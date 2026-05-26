//! Replica-level tests for the canonical publish event log.
//!
//! Spins up a memory-backed `Docs` + `MemStore` blobs setup so the tests
//! exercise the real iroh-docs read/write path without touching disk or
//! the network.

use anyhow::Result;
use iroh::Endpoint;
use iroh_blobs::store::mem::MemStore;
use iroh_docs::protocol::Docs;
use iroh_gossip::net::Gossip;
use tdf_iroh_s3::catalog::replica::{CatalogReplica, event_key};
use tdf_iroh_s3::catalog::{
    CatalogEntry, EventAuthorization, PublishEvent, PublishEventKind, Visibility,
};
use tempfile::TempDir;

async fn spawn_replica() -> Result<(CatalogReplica, TempDir)> {
    let tmp = TempDir::new()?;
    let endpoint = Endpoint::empty_builder().bind().await?;
    let blobs = MemStore::new();
    let blobs_store: iroh_blobs::api::Store = (*blobs).clone();
    let gossip = Gossip::builder().spawn(endpoint.clone());
    let docs = Docs::memory()
        .spawn(endpoint.clone(), blobs_store.clone(), gossip)
        .await?;
    let namespace_id_path = tmp.path().join("catalog.namespace_id");
    let replica = CatalogReplica::open_or_create(&docs, blobs_store, namespace_id_path).await?;
    Ok((replica, tmp))
}

fn sample_event(creator: &str, content_id: &str, published_at: &str) -> PublishEvent {
    PublishEvent {
        seq: 0,
        creator_id: creator.to_string(),
        content_id: content_id.to_string(),
        kind: PublishEventKind::Publish,
        published_at: published_at.to_string(),
        entry: CatalogEntry {
            content_id: content_id.to_string(),
            creator_id: creator.to_string(),
            title: format!("title-{content_id}"),
            visibility: Visibility::Public,
            required_tier_ids: vec![],
            tdf_ref: format!("iroh:{content_id}"),
            manifest_ref: format!("creators/{creator}/content/{content_id}/manifest.json"),
            published_at: published_at.to_string(),
        },
        authorization: EventAuthorization {
            cwt_b64: "stub-cwt".to_string(),
            issuer: "https://issuer.example".to_string(),
            cti: "00".to_string(),
        },
    }
}

#[tokio::test]
async fn append_then_list_roundtrips_in_seq_order() -> Result<()> {
    let (replica, _tmp) = spawn_replica().await?;

    let seq1 = replica
        .append_event(sample_event("alice", "c1", "2026-05-25T10:00:00Z"))
        .await?;
    let seq2 = replica
        .append_event(sample_event("alice", "c2", "2026-05-25T11:00:00Z"))
        .await?;
    let seq3 = replica
        .append_event(sample_event("alice", "c3", "2026-05-25T12:00:00Z"))
        .await?;
    assert_eq!((seq1, seq2, seq3), (1, 2, 3));

    let events = replica.list_events("alice").await?;
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[2].seq, 3);
    assert_eq!(events[0].content_id, "c1");
    assert_eq!(events[2].content_id, "c3");
    Ok(())
}

#[tokio::test]
async fn events_for_one_creator_do_not_bleed_into_another() -> Result<()> {
    let (replica, _tmp) = spawn_replica().await?;

    replica
        .append_event(sample_event("alice", "a1", "2026-05-25T10:00:00Z"))
        .await?;
    replica
        .append_event(sample_event("bob", "b1", "2026-05-25T10:00:00Z"))
        .await?;
    replica
        .append_event(sample_event("alice", "a2", "2026-05-25T10:00:00Z"))
        .await?;

    let alice = replica.list_events("alice").await?;
    let bob = replica.list_events("bob").await?;
    assert_eq!(alice.len(), 2);
    assert_eq!(bob.len(), 1);
    assert_eq!(alice[0].content_id, "a1");
    assert_eq!(alice[1].content_id, "a2");
    assert_eq!(bob[0].content_id, "b1");
    Ok(())
}

#[tokio::test]
async fn append_stamps_seq_into_event_body_matching_replica_key() -> Result<()> {
    let (replica, _tmp) = spawn_replica().await?;

    let seq = replica
        .append_event(sample_event("alice", "c1", "2026-05-25T10:00:00Z"))
        .await?;

    let events = replica.list_events("alice").await?;
    assert_eq!(events[0].seq, seq, "body seq must match assigned slot");

    // And the replica key for that seq is the conventional one — guards
    // against silent layout drift.
    let key = event_key("alice", seq);
    assert_eq!(key, format!("creators/alice/events/{seq:020}"));
    Ok(())
}

#[tokio::test]
async fn namespace_id_persists_via_id_file() -> Result<()> {
    let tmp = TempDir::new()?;
    let endpoint = Endpoint::empty_builder().bind().await?;
    let blobs = MemStore::new();
    let blobs_store: iroh_blobs::api::Store = (*blobs).clone();
    let gossip = Gossip::builder().spawn(endpoint.clone());
    let docs = Docs::memory()
        .spawn(endpoint.clone(), blobs_store.clone(), gossip)
        .await?;
    let namespace_id_path = tmp.path().join("catalog.namespace_id");

    let first =
        CatalogReplica::open_or_create(&docs, blobs_store.clone(), namespace_id_path.clone())
            .await?;
    let id_first = first.namespace_id();

    // Reopen: same Docs runtime, same id-file. Should return the same namespace.
    let second =
        CatalogReplica::open_or_create(&docs, blobs_store, namespace_id_path.clone()).await?;
    let id_second = second.namespace_id();
    assert_eq!(id_first, id_second);

    // And the file itself contains the 32-byte id.
    let bytes = tokio::fs::read(&namespace_id_path).await?;
    assert_eq!(bytes.len(), 32);
    assert_eq!(bytes.as_slice(), id_first.as_bytes());
    Ok(())
}
