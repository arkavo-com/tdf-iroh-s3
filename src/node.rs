use anyhow::{Context, Result};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr};
use iroh_blobs::store::mem::MemStore;
use iroh_blobs::BlobsProtocol;
use std::net::Ipv4Addr;
use tracing::info;

pub struct TdfIrohNode {
    router: Router,
    store: MemStore,
    endpoint: Endpoint,
}

impl TdfIrohNode {
    pub async fn spawn(bind_port: u16) -> Result<Self> {
        let store = MemStore::new();

        let endpoint = Endpoint::builder(presets::N0)
            .bind_addr((Ipv4Addr::UNSPECIFIED, bind_port))
            .context("Invalid bind address")?
            .bind()
            .await
            .context("Failed to bind Iroh endpoint")?;

        info!("Iroh endpoint bound on port {}", bind_port);
        endpoint.online().await;
        info!("Iroh endpoint online");

        let blobs = BlobsProtocol::new(&store, None);

        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs)
            .spawn();

        let addr = endpoint.addr();
        info!("Node ID: {}", addr.id);

        Ok(Self {
            router,
            store,
            endpoint,
        })
    }

    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    pub fn store(&self) -> &MemStore {
        &self.store
    }

    pub async fn shutdown(self) -> Result<()> {
        self.router
            .shutdown()
            .await
            .context("Failed to shutdown router")?;
        Ok(())
    }
}
