use anyhow::{Context, Result};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr};
use iroh_blobs::provider::events::{
    EventMask, EventSender, ProviderMessage, RequestMode, RequestUpdate,
};
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::BlobsProtocol;
use iroh_docs::protocol::Docs;
use iroh_gossip::net::Gossip;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::auth::{CoseKeyCache, Verifier};
use crate::catalog::replica::CatalogReplica;
use crate::config::Config;
use crate::ingest::ingest_from_store;
use crate::secret_key;
use crate::store::s3::S3Client;

pub struct TdfIrohNode {
    router: Router,
    store: FsStore,
    endpoint: Endpoint,
    pub s3_client: Arc<S3Client>,
    pub config: Arc<Config>,
    pub catalog: Arc<CatalogReplica>,
    pub verifier: Arc<Verifier>,
    cancel: CancellationToken,
}

impl TdfIrohNode {
    pub async fn spawn(config: Config) -> Result<Self> {
        let config = Arc::new(config);

        let s3_client = Arc::new(
            S3Client::new(&config.s3.bucket, &config.s3.region, &config.s3.prefix)
                .await
                .context("Failed to create S3 client")?,
        );

        let store = FsStore::load(&config.iroh.data_dir)
            .await
            .context("Failed to load FsStore")?;

        let mut builder = Endpoint::builder(presets::N0);
        if !config.iroh.secret_key_param.is_empty() {
            let secret_key =
                secret_key::load_or_create(&config.iroh.secret_key_param, &config.s3.region)
                    .await
                    .context("Failed to load or create node secret key")?;
            builder = builder.secret_key(secret_key);
        }

        let endpoint = builder
            .bind_addr((Ipv4Addr::UNSPECIFIED, config.iroh.bind_port))
            .context("Invalid bind address")?
            .bind()
            .await
            .context("Failed to bind Iroh endpoint")?;

        info!("Iroh endpoint bound on port {}", config.iroh.bind_port);
        endpoint.online().await;
        info!("Iroh endpoint online");

        let cancel = CancellationToken::new();

        // NotifyLog on `get` enables event delivery for ALL request types (get, push, etc.)
        // and provides a RequestUpdate stream to track transfer completion.
        // Note: EventSender::request() checks only mask.get, not mask.push.
        // Notify on `get` enables event delivery for ALL request types (get, push, etc.)
        // Note: EventSender::request() checks only mask.get, not mask.push.
        let mask = EventMask {
            get: RequestMode::Notify,
            ..EventMask::DEFAULT
        };
        let (event_sender, event_rx) = EventSender::channel(64, mask);

        let blobs = BlobsProtocol::new(&store, Some(event_sender));
        let blobs_store: iroh_blobs::api::Store = (*store).clone();

        // Gossip + Docs runtime — needed to host the catalog replica that
        // holds the publish event log.
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let docs = Docs::persistent(std::path::PathBuf::from(&config.catalog.data_dir))
            .spawn(endpoint.clone(), blobs_store.clone(), gossip.clone())
            .await
            .context("Failed to spawn iroh-docs runtime")?;

        let namespace_id_path =
            std::path::PathBuf::from(&config.catalog.data_dir).join("catalog.namespace_id");
        let catalog = Arc::new(
            CatalogReplica::open_or_create(&docs, blobs_store.clone(), namespace_id_path)
                .await
                .context("Failed to open catalog replica")?,
        );
        info!(
            namespace_id = %catalog.namespace_id(),
            "catalog replica ready"
        );

        // CWT verifier (COSE keys fetched from config-supplied endpoint).
        let http_client = reqwest::Client::builder()
            .build()
            .context("Failed to build reqwest client")?;
        let keys = CoseKeyCache::spawn(
            config.auth.cose_keys_url.clone(),
            Duration::from_secs(config.auth.refresh_interval_secs),
            http_client,
        )
        .await
        .context("Failed to spawn COSE key cache")?;
        let verifier = Arc::new(Verifier::new(
            keys,
            config.auth.issuer.clone(),
            config.auth.clock_skew_secs,
        ));

        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs)
            .accept(iroh_docs::ALPN, docs.clone())
            .accept(iroh_gossip::ALPN, gossip.clone())
            .spawn();

        let addr = endpoint.addr();
        info!("Node ID: {}", addr.id);

        // Spawn the ingest background task
        {
            let store = store.clone();
            let s3_client = Arc::clone(&s3_client);
            let config = Arc::clone(&config);
            let cancel = cancel.clone();
            tokio::spawn(async move {
                run_ingest_loop(event_rx, store, s3_client, config, cancel).await;
            });
        }

        Ok(Self {
            router,
            store,
            endpoint,
            s3_client,
            config,
            catalog,
            verifier,
            cancel,
        })
    }

    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    pub fn store(&self) -> &FsStore {
        &self.store
    }

    pub async fn shutdown(self) -> Result<()> {
        self.cancel.cancel();
        self.router
            .shutdown()
            .await
            .context("Failed to shutdown router")?;
        let _ = self.store.shutdown().await;
        Ok(())
    }
}

async fn run_ingest_loop(
    mut rx: tokio::sync::mpsc::Receiver<ProviderMessage>,
    store: FsStore,
    s3_client: Arc<S3Client>,
    config: Arc<Config>,
    cancel: CancellationToken,
) {
    info!("Ingest loop started");
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Ingest loop cancelled");
                break;
            }
            msg = rx.recv() => {
                if let Some(ref m) = msg {
                    info!("Received provider message: {:?}", m);
                }
                match msg {
                    Some(ProviderMessage::PushRequestReceivedNotify(msg)) => {
                        let hash = msg.inner.request.hash;
                        info!(%hash, "Push request received (notify)");
                        let store = store.clone();
                        let s3_client = Arc::clone(&s3_client);
                        let config = Arc::clone(&config);
                        tokio::spawn(async move {
                            wait_and_ingest(hash, msg.rx, &store, &s3_client, &config).await;
                        });
                    }
                    Some(ProviderMessage::PushRequestReceived(msg)) => {
                        let hash = msg.inner.request.hash;
                        info!(%hash, "Push request received (intercept)");
                        msg.tx.send(Ok(())).await.ok();
                        let store = store.clone();
                        let s3_client = Arc::clone(&s3_client);
                        let config = Arc::clone(&config);
                        tokio::spawn(async move {
                            wait_and_ingest(hash, msg.rx, &store, &s3_client, &config).await;
                        });
                    }
                    Some(ProviderMessage::GetRequestReceivedNotify(_)) => {
                        debug!("Get request received (notify)");
                    }
                    Some(ProviderMessage::GetRequestReceived(msg)) => {
                        debug!("Get request received (intercept)");
                        msg.tx.send(Ok(())).await.ok();
                    }
                    Some(ProviderMessage::ClientConnected(msg)) => {
                        debug!("Client connected, accepting");
                        msg.tx.send(Ok(())).await.ok();
                    }
                    Some(other) => {
                        debug!("Other event received: {:?}", std::mem::discriminant(&other));
                    }
                    None => {
                        info!("Event channel closed, ingest loop exiting");
                        break;
                    }
                }
            }
        }
    }
}

async fn wait_and_ingest(
    hash: iroh_blobs::Hash,
    mut rx: irpc::channel::mpsc::Receiver<RequestUpdate>,
    store: &FsStore,
    s3_client: &S3Client,
    config: &Config,
) {
    // Wait for the push transfer to complete
    let mut completed = false;
    while let Ok(Some(update)) = rx.recv().await {
        match update {
            RequestUpdate::Started(s) => {
                info!(%hash, size = s.size, "Push transfer started");
            }
            RequestUpdate::Progress(_) => {}
            RequestUpdate::Completed(_) => {
                info!(%hash, "Push transfer completed");
                completed = true;
                break;
            }
            RequestUpdate::Aborted(_) => {
                warn!(%hash, "Push transfer aborted");
                return;
            }
        }
    }
    if !completed {
        // Notify mode doesn't provide RequestUpdate events — the stream closes
        // immediately. The blob should already be in the store by this point.
        info!(%hash, "Push notification received, checking store");
    }

    // Blob is written — ingest with small retry for FsStore async DB propagation
    for attempt in 0..10 {
        match ingest_from_store(hash, store, &config.validation, s3_client).await {
            Ok(Some(result)) => {
                info!(
                    hash = %result.hash_hex,
                    size = result.size,
                    "Blob ingested successfully"
                );
                return;
            }
            Ok(None) => {
                debug!(%hash, attempt, "Blob not yet readable, retrying");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => {
                error!(%hash, error = %e, "Ingest failed");
                return;
            }
        }
    }
    error!(%hash, "Blob not readable after transfer completed");
}
