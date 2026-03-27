# Ingest Wiring and FsStore Migration

## Problem

Two issues prevent the node from functioning as intended:

1. **Ingest pipeline is disconnected.** `BlobsProtocol` accepts blobs into the store, but nothing calls `ingest_blob()` to validate and persist them to S3. The ingest module is dead code at runtime.

2. **MemStore loses blobs on restart.** Blobs live only in memory. After a restart, previously ingested blobs cannot be served back to peers via their tickets, even though the data exists in S3.

## Design

### FsStore Migration

Replace `MemStore` with `FsStore` in `node.rs`.

`FsStore::load(path)` takes a directory path and creates a persistent `redb`-backed store. It implements the same `Store` interface as `MemStore` — drop-in replacement.

**Config change:** Add `data_dir` to `IrohConfig`:

```toml
[iroh]
bind_port = 11204
data_dir = "/var/lib/tdf-iroh-s3/data"
```

Default: `/var/lib/tdf-iroh-s3/data`. The FsStore creates subdirectories (`blobs.db`, `data/`, `temp/`) under this path.

**node.rs change:** Replace `MemStore::new()` with `FsStore::load(&config.iroh.data_dir).await?`. Update the `TdfIrohNode` struct to hold `FsStore` instead of `MemStore`.

**Cargo.toml:** The `fs-store` feature is enabled by default in iroh-blobs, so no feature flag change needed.

### Event-Driven Ingest

Use iroh-blobs' `EventSender` to get notified when a push request arrives, then ingest the blob after transfer completes.

**Event setup in `node.rs`:**

```rust
let (event_sender, event_receiver) = EventSender::channel(32, EventMask {
    push: RequestMode::Notify,
    ..Default::default()
});

let blobs = BlobsProtocol::new(&store, Some(event_sender));
```

`RequestMode::Notify` fires `PushRequestReceivedNotify` when a peer pushes a blob. The notification includes the blob's `Hash`.

**Ingest task:** Spawn a background `tokio::spawn` task that:

1. Receives `PushRequestReceivedNotify` from the event channel — extracts the blob `Hash`.
2. Polls `store.status(hash)` until `BlobStatus::Complete` (the push notification arrives before transfer finishes).
3. Reads the blob bytes via `store.get_bytes(hash)`.
4. Calls `ingest_blob()` to validate and upload to S3.
5. Logs success or rejection. Invalid blobs are logged but remain in the FsStore (they'll be garbage collected if untagged).

**Why poll status instead of `InterceptLog`?** Intercept mode requires responding to accept/reject the push before we have the full blob — and TDF validation needs the complete blob. `Notify` mode lets the transfer proceed automatically. Polling `status()` is simple and reliable; the blob is local so the poll resolves in microseconds.

### Shutdown

The ingest task must stop when the node shuts down. Pass a `tokio::sync::watch` or `CancellationToken` to the ingest task. On `ctrl_c`, cancel the token before calling `router.shutdown()`.

### Error Handling

- FsStore load failure: fatal, node won't start.
- Event channel disconnected: ingest task logs error and exits (node continues serving existing blobs).
- Ingest failure (validation rejection): log warning, blob stays in FsStore but is not uploaded to S3.
- S3 upload failure: log error, can be retried on next push of the same blob (dedup check in `ingest_blob` will re-attempt upload since `has_blob` returns false).

## Files Changed

| File | Change |
|------|--------|
| `Cargo.toml` | Add `tokio-util` for `CancellationToken` |
| `src/config.rs` | Add `data_dir: String` to `IrohConfig` with default |
| `src/node.rs` | Replace MemStore with FsStore, create EventSender, spawn ingest task, cancellation on shutdown |
| `src/ingest.rs` | No changes — existing API is correct |
| `src/lib.rs` | No changes expected |
| `packer/files/bootstrap.sh` | Ensure data dir exists |
| `tests/*` | Update tests that reference `MemStore` or `node.store()` |

## Out of Scope

- S3-backed custom store (YAGNI — FsStore handles the serving-blobs-to-peers requirement)
- Reloading blobs from S3 into FsStore (FsStore is persistent, so this is only needed if the disk is lost — same as any stateful service)
- Issues 3 and 4 (upstream: arkavo-org/opentdf-rs#76, arkavo-org/opentdf-rs#77)
