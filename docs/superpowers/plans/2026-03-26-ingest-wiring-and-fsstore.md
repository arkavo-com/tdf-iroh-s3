# Ingest Wiring and FsStore Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the ingest pipeline to incoming blob pushes and replace MemStore with FsStore so blobs survive restarts and can be served back to peers.

**Architecture:** Replace MemStore with FsStore for persistent blob storage. Create an EventSender channel to receive push notifications from BlobsProtocol. Spawn a background ingest task that waits for blobs to complete, then validates and uploads to S3.

**Tech Stack:** iroh-blobs (FsStore, EventSender, EventMask), tokio (spawn, CancellationToken via tokio-util)

**Spec:** `docs/superpowers/specs/2026-03-26-ingest-wiring-and-fsstore-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `Cargo.toml` | Modify | Add `tokio-util` dependency |
| `src/config.rs` | Modify | Add `data_dir` field to `IrohConfig` |
| `src/node.rs` | Modify | FsStore, EventSender, ingest task, cancellation |
| `src/ingest.rs` | Modify | Add `ingest_from_store()` that reads from store by hash |
| `packer/scripts/install.sh` | Modify | Create data directory |
| `tests/config_test.rs` | Modify | Cover `data_dir` field in config tests |
| `tests/node_ingest_test.rs` | Create | Integration test: push blob → verify ingest runs |

---

### Task 1: Add `data_dir` to Config

**Files:**
- Modify: `src/config.rs:13-28` (IrohConfig struct and Default impl)
- Modify: `tests/config_test.rs`

- [ ] **Step 1: Write failing test for data_dir default**

Add to `tests/config_test.rs`:

```rust
#[test]
fn test_config_default_data_dir() {
    let toml_str = r#"
[s3]
bucket = "test-bucket"
region = "us-east-1"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.iroh.data_dir, "/var/lib/tdf-iroh-s3/data");
}

#[test]
fn test_config_custom_data_dir() {
    let toml_str = r#"
[iroh]
data_dir = "/tmp/my-data"

[s3]
bucket = "test-bucket"
region = "us-east-1"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.iroh.data_dir, "/tmp/my-data");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test config_test`
Expected: FAIL — `IrohConfig` has no field `data_dir`

- [ ] **Step 3: Add `data_dir` to IrohConfig**

In `src/config.rs`, add the field and default:

```rust
fn default_data_dir() -> String {
    "/var/lib/tdf-iroh-s3/data".to_string()
}
```

Add to the `IrohConfig` struct:

```rust
#[derive(Debug, Deserialize)]
pub struct IrohConfig {
    #[serde(default = "default_bind_port")]
    pub bind_port: u16,
    #[serde(default = "default_secret_key_path")]
    pub secret_key_path: String,
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
}
```

Update the `Default` impl:

```rust
impl Default for IrohConfig {
    fn default() -> Self {
        Self {
            bind_port: default_bind_port(),
            secret_key_path: default_secret_key_path(),
            data_dir: default_data_dir(),
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test config_test`
Expected: All 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/config.rs tests/config_test.rs
git commit -m "feat: add data_dir to IrohConfig for FsStore path"
```

---

### Task 2: Add `tokio-util` dependency

**Files:**
- Modify: `Cargo.toml:20`

- [ ] **Step 1: Add tokio-util to Cargo.toml**

Add under the `# Async runtime` section, after the `tokio` line:

```toml
tokio-util = "0.7"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: Compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add tokio-util dependency for CancellationToken"
```

---

### Task 3: Replace MemStore with FsStore in node.rs

**Files:**
- Modify: `src/node.rs`

- [ ] **Step 1: Update imports and struct**

Replace the full content of `src/node.rs` with:

```rust
use anyhow::{Context, Result};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr};
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::BlobsProtocol;
use std::net::Ipv4Addr;
use std::sync::Arc;
use tracing::info;

use crate::config::Config;
use crate::store::s3::S3Client;

pub struct TdfIrohNode {
    router: Router,
    store: FsStore,
    endpoint: Endpoint,
    pub s3_client: Arc<S3Client>,
    pub config: Arc<Config>,
}

impl TdfIrohNode {
    pub async fn spawn(config: Config) -> Result<Self> {
        let s3_client = Arc::new(
            S3Client::new(&config.s3.bucket, &config.s3.region, &config.s3.prefix)
                .await
                .context("Failed to create S3 client")?,
        );

        let store = FsStore::load(&config.iroh.data_dir)
            .await
            .context("Failed to load FsStore")?;

        let endpoint = Endpoint::builder(presets::N0)
            .bind_addr((Ipv4Addr::UNSPECIFIED, config.iroh.bind_port))
            .context("Invalid bind address")?
            .bind()
            .await
            .context("Failed to bind Iroh endpoint")?;

        info!("Iroh endpoint bound on port {}", config.iroh.bind_port);
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
            s3_client,
            config: Arc::new(config),
        })
    }

    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    pub fn store(&self) -> &FsStore {
        &self.store
    }

    pub async fn shutdown(self) -> Result<()> {
        self.router
            .shutdown()
            .await
            .context("Failed to shutdown router")?;
        self.store
            .shutdown()
            .await;
        Ok(())
    }
}
```

Key changes from the original:
- `MemStore` → `FsStore` (import, struct field, constructor)
- `MemStore::new()` → `FsStore::load(&config.iroh.data_dir).await?`
- `store()` returns `&FsStore` instead of `&MemStore`
- `shutdown()` also calls `self.store.shutdown().await` for clean FsStore teardown

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: Compiles. If `FsStore::shutdown` returns something other than `()`, adjust the shutdown call.

- [ ] **Step 3: Run existing tests**

Run: `cargo test`
Expected: All existing tests pass (no tests directly instantiate `TdfIrohNode` — they test config, validation, and the test CLI separately)

- [ ] **Step 4: Commit**

```bash
git add src/node.rs
git commit -m "feat: replace MemStore with FsStore for persistent blob storage"
```

---

### Task 4: Add `ingest_from_store()` function

**Files:**
- Modify: `src/ingest.rs`
- Create: `tests/node_ingest_test.rs`

- [ ] **Step 1: Write the new function**

Add to `src/ingest.rs`, below the existing `ingest_blob()`:

```rust
use iroh_blobs::Hash;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::api::blobs::BlobStatus;

/// Read a blob from the FsStore by hash, validate it, and upload to S3.
/// Returns Ok(Some(result)) on success, Ok(None) if the blob is not yet complete,
/// or Err if validation/upload fails.
pub async fn ingest_from_store(
    hash: Hash,
    store: &FsStore,
    validation_config: &crate::config::ValidationConfig,
    s3_client: &S3Client,
) -> Result<Option<IngestResult>> {
    // Check if blob is complete in the store
    let status = store
        .status(hash)
        .await
        .context("Failed to check blob status")?;

    match status {
        BlobStatus::Complete { .. } => {}
        _ => return Ok(None),
    }

    // Read blob bytes
    let data = store
        .get_bytes(hash)
        .await
        .context("Failed to read blob from store")?;

    // Delegate to the existing ingest pipeline
    ingest_blob(&data, validation_config, s3_client).await.map(Some)
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: Compiles. The `BlobStatus` import path may need adjustment — check if it's `iroh_blobs::api::blobs::BlobStatus` or `iroh_blobs::store::BlobStatus`. Use the path that compiles.

- [ ] **Step 3: Commit**

```bash
git add src/ingest.rs
git commit -m "feat: add ingest_from_store to read blobs by hash and ingest"
```

---

### Task 5: Wire EventSender and spawn ingest task

**Files:**
- Modify: `src/node.rs`

- [ ] **Step 1: Add event handling imports**

Add these imports to the top of `src/node.rs`:

```rust
use iroh_blobs::provider::events::{EventSender, EventMask, RequestMode};
use iroh_blobs::provider::events::ProviderMessage;
use tokio_util::sync::CancellationToken;
use tracing::warn;
```

- [ ] **Step 2: Add CancellationToken to the struct**

Add a field to `TdfIrohNode`:

```rust
pub struct TdfIrohNode {
    router: Router,
    store: FsStore,
    endpoint: Endpoint,
    pub s3_client: Arc<S3Client>,
    pub config: Arc<Config>,
    cancel: CancellationToken,
}
```

- [ ] **Step 3: Create EventSender and spawn ingest task in `spawn()`**

Replace the `BlobsProtocol::new` call and everything after it (up to the `Ok(Self {...})`) in `spawn()` with:

```rust
        let cancel = CancellationToken::new();

        // Set up event channel to receive push notifications
        let (event_sender, event_receiver) = EventSender::channel(
            32,
            EventMask {
                push: RequestMode::Notify,
                ..Default::default()
            },
        );

        let blobs = BlobsProtocol::new(&store, Some(event_sender));

        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs)
            .spawn();

        let addr = endpoint.addr();
        info!("Node ID: {}", addr.id);

        // Spawn background ingest task
        {
            let store = store.clone();
            let s3_client = Arc::clone(&s3_client);
            let config = config.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                run_ingest_loop(event_receiver, store, s3_client, config, cancel).await;
            });
        }

        Ok(Self {
            router,
            store,
            endpoint,
            s3_client,
            config: Arc::new(config),
            cancel,
        })
```

Note: `config` is used before being wrapped in `Arc` here, so clone it for the ingest task, then wrap the original. Alternatively, wrap in `Arc` earlier. Adjust to:

```rust
        let config = Arc::new(config);

        // ... (event_sender, blobs, router setup as above) ...

        // Spawn background ingest task
        {
            let store = store.clone();
            let s3_client = Arc::clone(&s3_client);
            let config = Arc::clone(&config);
            let cancel = cancel.clone();
            tokio::spawn(async move {
                run_ingest_loop(event_receiver, store, s3_client, config, cancel).await;
            });
        }

        Ok(Self {
            router,
            store,
            endpoint,
            s3_client,
            config,
            cancel,
        })
```

- [ ] **Step 4: Implement `run_ingest_loop`**

Add this function below the `TdfIrohNode` impl block in `src/node.rs`:

```rust
async fn run_ingest_loop(
    mut receiver: tokio::sync::mpsc::Receiver<ProviderMessage>,
    store: FsStore,
    s3_client: Arc<S3Client>,
    config: Arc<Config>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Ingest loop shutting down");
                break;
            }
            msg = receiver.recv() => {
                let Some(msg) = msg else {
                    info!("Event channel closed, ingest loop exiting");
                    break;
                };
                handle_provider_message(msg, &store, &s3_client, &config).await;
            }
        }
    }
}

async fn handle_provider_message(
    msg: ProviderMessage,
    store: &FsStore,
    s3_client: &S3Client,
    config: &Config,
) {
    // Extract hash from push notification
    let hash = match msg {
        ProviderMessage::PushRequestReceivedNotify(msg) => msg.inner.0.request.0.hash,
        _ => return, // Ignore non-push events
    };

    info!(hash = %hash.to_hex(), "Push received, waiting for transfer to complete");

    // Poll until blob is complete (transfer is in progress)
    let mut attempts = 0;
    loop {
        match crate::ingest::ingest_from_store(
            hash,
            store,
            &config.validation,
            s3_client,
        )
        .await
        {
            Ok(Some(result)) => {
                info!(
                    hash = %result.hash_hex,
                    size = result.size,
                    "Blob ingested successfully"
                );
                return;
            }
            Ok(None) => {
                // Blob not complete yet, wait and retry
                attempts += 1;
                if attempts > 300 {
                    // 30 seconds timeout (300 * 100ms)
                    warn!(hash = %hash.to_hex(), "Blob transfer timed out after 30s");
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => {
                warn!(hash = %hash.to_hex(), error = %e, "Blob ingest failed");
                return;
            }
        }
    }
}
```

- [ ] **Step 5: Update shutdown to cancel the ingest task**

Replace the `shutdown` method:

```rust
    pub async fn shutdown(self) -> Result<()> {
        self.cancel.cancel();
        self.router
            .shutdown()
            .await
            .context("Failed to shutdown router")?;
        self.store
            .shutdown()
            .await;
        Ok(())
    }
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo check`
Expected: Compiles. Fix any import paths. The `ProviderMessage::PushRequestReceivedNotify` variant access pattern (`msg.inner.0.request.0.hash`) may need adjustment — check the actual field structure. The `PushRequest` wraps `GetRequest` via a newtype, so `.0` unwraps it. The `Notify` wraps `RequestReceived`, so `.inner.0` unwraps `Notify` then accesses `RequestReceived`.

- [ ] **Step 7: Run existing tests**

Run: `cargo test`
Expected: All existing tests pass

- [ ] **Step 8: Commit**

```bash
git add src/node.rs
git commit -m "feat: wire EventSender and spawn ingest task for incoming pushes"
```

---

### Task 6: Update deployment scripts

**Files:**
- Modify: `packer/scripts/install.sh`

- [ ] **Step 1: Add data directory creation to install.sh**

Add after the existing `install` commands, before `systemctl daemon-reload`:

```bash
# Create data directory for FsStore
install -d -o tdf-iroh-s3 -g tdf-iroh-s3 -m 750 /var/lib/tdf-iroh-s3/data
```

- [ ] **Step 2: Commit**

```bash
git add packer/scripts/install.sh
git commit -m "chore: create FsStore data directory in install script"
```

---

### Task 7: Integration test — push blob triggers ingest

**Files:**
- Create: `tests/node_ingest_test.rs`

This test verifies the full flow: start a node with FsStore, push a TDF blob via the test CLI client, and confirm the blob is stored and accessible.

- [ ] **Step 1: Write the integration test**

Create `tests/node_ingest_test.rs`:

```rust
//! Integration test: push a TDF blob to a node, verify it arrives in the FsStore.
//!
//! Note: This test does NOT verify S3 upload (requires LocalStack/MinIO).
//! It verifies the event-driven flow: push → FsStore storage → blob accessible.

use iroh_blobs::api::blobs::BlobStatus;
use tdf_iroh_s3::config::{Config, IrohConfig, S3Config, ValidationConfig};
use tdf_iroh_s3::node::TdfIrohNode;
use tdf_iroh_s3::test_cli::iroh_client::IrohTestClient;

fn test_config(data_dir: &str) -> Config {
    Config {
        iroh: IrohConfig {
            bind_port: 0, // Random port
            secret_key_path: String::new(),
            data_dir: data_dir.to_string(),
        },
        s3: S3Config {
            bucket: "test-bucket".to_string(),
            region: "us-east-1".to_string(),
            prefix: String::new(),
        },
        validation: ValidationConfig::default(),
    }
}

#[tokio::test]
async fn test_push_blob_stored_in_fsstore() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config = test_config(tmp_dir.path().to_str().unwrap());

    let node = TdfIrohNode::spawn(config).await.unwrap();
    let node_id = node.addr().id;

    // Create a valid TDF blob
    let tdf_bytes = create_test_tdf();

    // Push it to the node
    let client = IrohTestClient::new().await.unwrap();
    let hash = client.push_to_node(node_id, &tdf_bytes).await.unwrap();

    // Wait for the blob to appear in the node's store
    let mut attempts = 0;
    loop {
        let status = node.store().status(hash).await.unwrap();
        if matches!(status, BlobStatus::Complete { .. }) {
            break;
        }
        attempts += 1;
        assert!(attempts < 50, "Blob did not complete within 5 seconds");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Verify we can read the blob back
    let stored = node.store().get_bytes(hash).await.unwrap();
    assert_eq!(stored.as_ref(), tdf_bytes.as_slice());

    client.shutdown().await.unwrap();
    node.shutdown().await.unwrap();
}

fn create_test_tdf() -> Vec<u8> {
    use opentdf::prelude::*;

    let policy = PolicyBuilder::new()
        .id_auto()
        .dissemination(["test@example.com"])
        .attribute_fqn("https://example.com/attr/test/value/integration")
        .unwrap()
        .build()
        .unwrap();

    Tdf::encrypt(b"integration test payload")
        .kas_url("https://kas.example.com")
        .policy(policy)
        .to_bytes()
        .unwrap()
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test --test node_ingest_test -- --nocapture`
Expected: PASS — blob is pushed to the node, stored in FsStore, and readable.

If it fails due to import paths or API differences, fix them. The key things to verify:
- `TdfIrohNode::spawn` works with a tempdir for `data_dir`
- `IrohTestClient::push_to_node` sends the blob over QUIC
- The blob appears in `node.store()` as `BlobStatus::Complete`
- `Config` struct can be constructed directly (it derives `Deserialize` but we're building it manually — may need to add `#[derive(Clone)]` on `Config` or make fields `pub`)

- [ ] **Step 3: Commit**

```bash
git add tests/node_ingest_test.rs
git commit -m "test: add integration test for push-to-FsStore flow"
```

---

### Task 8: Verify full build and all tests

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Final commit if any fixups needed**

```bash
git add -A
git commit -m "chore: fix clippy warnings"
```
