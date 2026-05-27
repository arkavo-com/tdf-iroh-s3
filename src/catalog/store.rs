//! Local redb-backed event log. Single-author (the node itself).

use anyhow::{Context, Result};
use redb::{Database, ReadableTable, TableDefinition};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::catalog::types::{ContentEvent, NewContentEvent};

const EVENTS_TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("events_v1");
const META_TABLE: TableDefinition<&str, u64> = TableDefinition::new("meta_v1");
const META_LAST_SEQ: &str = "last_seq";
const BROADCAST_CAPACITY: usize = 1024;

pub struct EventStore {
    db: Arc<Database>,
    tx: broadcast::Sender<ContentEvent>,
}

impl EventStore {
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create catalog dir {}", parent.display()))?;
        }
        let path = path.to_path_buf();
        let db = tokio::task::spawn_blocking(move || -> Result<Database> {
            let db = Database::create(&path)
                .with_context(|| format!("open redb at {}", path.display()))?;
            // Ensure both tables exist so subsequent read transactions don't ENOENT.
            let w = db.begin_write()?;
            {
                let _ = w.open_table(EVENTS_TABLE)?;
                let _ = w.open_table(META_TABLE)?;
            }
            w.commit()?;
            Ok(db)
        })
        .await??;
        let (tx, _rx) = broadcast::channel(BROADCAST_CAPACITY);
        Ok(Self {
            db: Arc::new(db),
            tx,
        })
    }

    pub fn current_tail(&self) -> u64 {
        let db = Arc::clone(&self.db);
        let r = match db.begin_read() {
            Ok(r) => r,
            Err(_) => return 0,
        };
        let table = match r.open_table(META_TABLE) {
            Ok(t) => t,
            Err(_) => return 0,
        };
        table
            .get(META_LAST_SEQ)
            .ok()
            .flatten()
            .map(|v| v.value())
            .unwrap_or(0)
    }

    pub async fn append(&self, new: NewContentEvent) -> Result<ContentEvent> {
        let db = Arc::clone(&self.db);
        let event = tokio::task::spawn_blocking(move || -> Result<ContentEvent> {
            let w = db.begin_write()?;
            let next_seq = {
                let meta = w.open_table(META_TABLE)?;
                meta.get(META_LAST_SEQ)?
                    .map(|v| v.value())
                    .unwrap_or(0)
                    + 1
            };
            let event = ContentEvent {
                seq: next_seq,
                content_id: new.content_id,
                manifest_ref: new.manifest_ref,
                attribute_value_fqns: new.attribute_value_fqns,
                ingested_at: new.ingested_at,
            };
            let mut buf = Vec::with_capacity(256);
            ciborium::ser::into_writer(&event, &mut buf)
                .context("encode ContentEvent as CBOR")?;
            {
                let mut events = w.open_table(EVENTS_TABLE)?;
                events.insert(next_seq, buf.as_slice())?;
            }
            {
                let mut meta = w.open_table(META_TABLE)?;
                meta.insert(META_LAST_SEQ, next_seq)?;
            }
            w.commit()?;
            Ok(event)
        })
        .await??;
        let _ = self.tx.send(event.clone());
        Ok(event)
    }
}
