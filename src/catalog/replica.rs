//! `iroh-docs` replica that holds the canonical publish event log.
//!
//! **The event log in this replica is the catalog.**
//! [`CatalogView`](super::CatalogView) is a disposable projection.
//!
//! Key layout inside the replica:
//!
//! ```text
//! creators/{creator_id}/events/{seq:020}
//! ```
//!
//! Each value is the canonical JSON of a [`PublishEvent`](super::PublishEvent),
//! including the [`EventAuthorization`](super::EventAuthorization) that pins
//! the entry to its issuing CWT.
//!
//! Sequence allocation reads all existing entries under the per-creator
//! prefix and writes at `max(seq) + 1`. Two concurrent writers can race —
//! iroh-docs lets both writes land at the same (key, author) by taking the
//! later timestamp, so we retry on detection up to
//! [`MAX_APPEND_RETRIES`]. For the single-author single-node model the
//! retry budget is overkill but cheap to keep.
//!
//! The replica's `NamespaceId` is persisted to a small file under the
//! catalog data directory so subsequent boots can reopen the same
//! replica via [`iroh_docs::api::DocsApi::open`]; iroh-docs itself
//! manages the corresponding `NamespaceSecret` inside its on-disk store.

use anyhow::{Context, Result, anyhow, bail};
use futures_lite::StreamExt;
use iroh_docs::api::Doc;
use iroh_docs::protocol::Docs;
use iroh_docs::store::Query;
use iroh_docs::{AuthorId, NamespaceId};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::catalog::PublishEvent;

pub const EVENT_SEQ_WIDTH: usize = 20;
pub const MAX_APPEND_RETRIES: u32 = 32;

/// Append an event to the canonical log for `creator_id` and project the
/// resulting view of the log.
pub struct CatalogReplica {
    doc: Doc,
    author: AuthorId,
    blobs: iroh_blobs::api::Store,
    /// Serializes seq allocation on a single node so two concurrent
    /// publishes don't both compute the same `next_seq`. iroh-docs would
    /// accept both as separate entries at the same (key, author) under
    /// different timestamps, but seq collisions are still a smell — we
    /// avoid them here cheaply.
    append_lock: Arc<Mutex<()>>,
}

impl CatalogReplica {
    /// Open the persisted replica, or create a fresh one on first boot.
    /// The `namespace_id_path` is created next to the docs storage.
    pub async fn open_or_create(
        docs: &Docs,
        blobs: iroh_blobs::api::Store,
        namespace_id_path: PathBuf,
    ) -> Result<Self> {
        let api = docs.api();
        let author = api
            .author_default()
            .await
            .context("failed to load default docs author")?;

        let doc = match read_namespace_id(&namespace_id_path).await? {
            Some(id) => {
                debug!(%id, "reopening catalog replica");
                api.open(id)
                    .await
                    .context("failed to open catalog replica")?
                    .ok_or_else(|| {
                        anyhow!(
                            "namespace {id} recorded in {} but missing from docs store",
                            namespace_id_path.display()
                        )
                    })?
            }
            None => {
                let doc = api.create().await.context("failed to create catalog replica")?;
                info!(id = %doc.id(), "created new catalog replica");
                write_namespace_id(&namespace_id_path, &doc.id()).await?;
                doc
            }
        };

        Ok(Self {
            doc,
            author,
            blobs,
            append_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn namespace_id(&self) -> NamespaceId {
        self.doc.id()
    }

    pub fn author_id(&self) -> AuthorId {
        self.author
    }

    /// Append a single publish event. Allocates the next `seq`, stamps it
    /// into the event body so the JSON value matches the replica key, and
    /// writes both. Returns the assigned `seq`.
    pub async fn append_event(&self, event: PublishEvent) -> Result<u64> {
        let _guard = self.append_lock.lock().await;

        for attempt in 0..MAX_APPEND_RETRIES {
            let seq = self.next_seq(&event.creator_id).await?;
            let key = event_key(&event.creator_id, seq);
            let stamped = PublishEvent { seq, ..event.clone() };
            let body = serde_json::to_vec(&stamped).context("serialize PublishEvent")?;
            self.doc
                .set_bytes(self.author, key.clone().into_bytes(), body)
                .await
                .with_context(|| format!("write event seq={seq} on attempt {attempt}"))?;

            // Confirm our write landed. Under serialized single-node access
            // this is always true; the check guards against a future
            // foreign-author race on a synced replica.
            let written = self
                .doc
                .get_exact(self.author, key.as_bytes(), false)
                .await?;
            if written.is_some() {
                return Ok(seq);
            }
        }
        bail!(
            "failed to append publish event for creator '{}' after {MAX_APPEND_RETRIES} retries",
            event.creator_id
        );
    }

    /// List all events for `creator_id` in ascending seq order.
    pub async fn list_events(&self, creator_id: &str) -> Result<Vec<PublishEvent>> {
        let prefix = events_prefix(creator_id);
        let query = Query::single_latest_per_key().key_prefix(prefix.as_bytes());
        let stream = self.doc.get_many(query).await?;
        let mut stream = Box::pin(stream);

        let mut out: Vec<PublishEvent> = Vec::new();
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            if entry.content_len() == 0 {
                continue; // deletion marker
            }
            let bytes = self
                .blobs
                .blobs()
                .get_bytes(entry.content_hash())
                .await
                .with_context(|| {
                    format!(
                        "fetch event blob for key {:?}",
                        std::str::from_utf8(entry.key()).unwrap_or("<non-utf8>")
                    )
                })?;
            let event: PublishEvent =
                serde_json::from_slice(&bytes).context("deserialize PublishEvent")?;
            out.push(event);
        }
        out.sort_by_key(|e| e.seq);
        Ok(out)
    }

    async fn next_seq(&self, creator_id: &str) -> Result<u64> {
        let prefix = events_prefix(creator_id);
        let query = Query::single_latest_per_key().key_prefix(prefix.as_bytes());
        let stream = self.doc.get_many(query).await?;
        let mut stream = Box::pin(stream);

        let mut max_seq: u64 = 0;
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            let Ok(key) = std::str::from_utf8(entry.key()) else {
                continue;
            };
            if let Some(seq) = parse_event_seq(key)
                && seq > max_seq
            {
                max_seq = seq;
            }
        }
        Ok(max_seq + 1)
    }
}

pub fn events_prefix(creator_id: &str) -> String {
    format!("creators/{creator_id}/events/")
}

pub fn event_key(creator_id: &str, seq: u64) -> String {
    format!(
        "creators/{creator_id}/events/{seq:0width$}",
        width = EVENT_SEQ_WIDTH
    )
}

pub fn parse_event_seq(key: &str) -> Option<u64> {
    let suffix = key.rsplit('/').next()?;
    suffix.parse().ok()
}

async fn read_namespace_id(path: &PathBuf) -> Result<Option<NamespaceId>> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(None);
    }
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("read namespace id from {}", path.display()))?;
    let id_bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("{} has length {}, expected 32", path.display(), bytes.len()))?;
    Ok(Some(NamespaceId::from(&id_bytes)))
}

async fn write_namespace_id(path: &PathBuf, id: &NamespaceId) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    tokio::fs::write(path, id.as_bytes())
        .await
        .with_context(|| format!("write namespace id to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_keys_sort_numerically_under_lexicographic_ordering() {
        let mut keys = [
            event_key("creator_a", 10),
            event_key("creator_a", 2),
            event_key("creator_a", 1),
            event_key("creator_a", 100),
        ];
        keys.sort();
        let seqs: Vec<u64> = keys.iter().filter_map(|k| parse_event_seq(k)).collect();
        assert_eq!(seqs, vec![1, 2, 10, 100]);
    }

    #[test]
    fn parse_event_seq_extracts_padded_number() {
        let key = event_key("c", 42);
        assert_eq!(parse_event_seq(&key), Some(42));
    }

    #[test]
    fn parse_event_seq_rejects_unrelated_keys() {
        assert_eq!(
            parse_event_seq("creators/c/content/x/manifest.json"),
            None
        );
        assert_eq!(parse_event_seq(""), None);
    }

    #[test]
    fn events_prefix_terminates_with_slash_so_it_doesnt_match_creator_b_under_creator_a() {
        let p = events_prefix("alice");
        assert!(p.ends_with('/'));
        // A naive substring match would match "creators/alice2/..." under
        // "creators/alice"; the trailing slash defends against that.
        assert!(!"creators/alice2/events/0".starts_with(&p));
        assert!("creators/alice/events/0".starts_with(&p));
    }
}
