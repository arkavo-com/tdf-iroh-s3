//! Orchestrates one creator publish: write payload, write manifest, append
//! a publish event with a conditional write, regenerate the catalog.
//!
//! This module is the S3-integration side of the catalog system; the pure
//! catalog-construction logic lives in [`crate::catalog`] and is tested
//! without S3.

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::catalog::keys;
use crate::catalog::{
    Catalog, CatalogEntry, ContentManifest, ContentMetadata, PublishEvent, PublishEventKind,
    PublishOutcome, build_catalog,
};
use crate::store::s3::{ConditionalPut, S3Client};

const MAX_EVENT_APPEND_RETRIES: u32 = 32;

/// Publish a validated TDF blob into a creator's namespace.
///
/// Caller must have already passed the blob through the validation pipeline
/// (`crate::validation::validate_blob`). This function does not re-validate;
/// it only assigns identity, persists, sequences, and rebuilds the catalog.
pub async fn publish_content(
    creator_id: &str,
    metadata: ContentMetadata,
    payload: Bytes,
    s3: &S3Client,
) -> Result<PublishOutcome> {
    if creator_id.is_empty() {
        bail!("creator_id must not be empty");
    }
    if metadata.title.trim().is_empty() {
        bail!("content metadata title must not be empty");
    }

    let content_id = blake3::hash(&payload).to_hex().to_string();
    let payload_size = payload.len() as u64;
    let published_at = now_rfc3339()?;
    let prefix = s3.prefix();

    // 1. Payload — idempotent: skip if a prior publish already wrote the bytes.
    let payload_key = keys::content_payload_key(prefix, creator_id, &content_id);
    if !s3.head_object(&payload_key).await? {
        s3.put_object_bytes(&payload_key, payload).await?;
    }

    let tdf_ref = format!("iroh:{content_id}");
    let manifest_key = keys::content_manifest_key(prefix, creator_id, &content_id);

    // 2. Per-content manifest. Overwrites on republish so the most recent
    //    creator-supplied metadata wins.
    let content_manifest = ContentManifest {
        content_id: content_id.clone(),
        creator_id: creator_id.to_string(),
        title: metadata.title.clone(),
        visibility: metadata.visibility,
        required_tier_ids: metadata.required_tier_ids.clone(),
        payload_size,
        tdf_ref: tdf_ref.clone(),
        published_at: published_at.clone(),
    };
    s3.put_json(&manifest_key, &content_manifest).await?;

    let entry = CatalogEntry {
        content_id: content_id.clone(),
        creator_id: creator_id.to_string(),
        title: metadata.title,
        visibility: metadata.visibility,
        required_tier_ids: metadata.required_tier_ids,
        tdf_ref,
        manifest_ref: manifest_key,
        published_at: published_at.clone(),
    };

    // 3. Append the publish event atomically.
    let seq = append_publish_event(s3, creator_id, entry, published_at.clone()).await?;

    // 4. Regenerate the catalog from the full event log.
    let catalog = regenerate_catalog(s3, creator_id).await?;

    // 5. Write a versioned snapshot, then update `latest.json` to point at it.
    //    Ordering matters: a reader that sees the new `latest.json` must be
    //    able to follow `version` back to a snapshot that exists.
    let snapshot_key = keys::catalog_snapshot_key(prefix, creator_id, catalog.version);
    s3.put_json(&snapshot_key, &catalog).await?;
    s3.put_json(&keys::catalog_latest_key(prefix, creator_id), &catalog)
        .await?;

    Ok(PublishOutcome {
        content_id,
        seq,
        version: catalog.version,
    })
}

/// Read every event for a creator and rebuild the catalog deterministically.
///
/// Exposed for catalog rebuild tooling (see plan §10).
pub async fn regenerate_catalog(s3: &S3Client, creator_id: &str) -> Result<Catalog> {
    let events = load_events(s3, creator_id).await?;
    let draft = build_catalog(creator_id, events);
    let generated_at = now_rfc3339()?;
    Ok(draft.finalize(generated_at))
}

async fn load_events(s3: &S3Client, creator_id: &str) -> Result<Vec<PublishEvent>> {
    let prefix = keys::events_prefix(s3.prefix(), creator_id);
    let keys = s3.list_keys(&prefix).await?;

    let mut events = Vec::with_capacity(keys.len());
    for key in keys {
        if keys::parse_event_seq(&key).is_none() {
            // Skip files that don't match the event naming convention rather
            // than failing — keeps the system robust to stray writes.
            continue;
        }
        match s3.get_json::<PublishEvent>(&key).await? {
            Some(event) => events.push(event),
            None => {
                // Listed but disappeared mid-read — treat as transient.
                continue;
            }
        }
    }
    Ok(events)
}

/// Append a publish event with a conditional write so two concurrent
/// publishers cannot land on the same seq. On 412, list again and retry
/// with the next candidate.
async fn append_publish_event(
    s3: &S3Client,
    creator_id: &str,
    entry: CatalogEntry,
    published_at: String,
) -> Result<u64> {
    for _ in 0..MAX_EVENT_APPEND_RETRIES {
        let next = next_event_seq(s3, creator_id).await?;
        let event = PublishEvent {
            seq: next,
            creator_id: creator_id.to_string(),
            content_id: entry.content_id.clone(),
            kind: PublishEventKind::Publish,
            published_at: published_at.clone(),
            entry: entry.clone(),
        };
        let key = keys::event_key(s3.prefix(), creator_id, next);
        let body = serde_json::to_vec_pretty(&event)
            .context("Failed to serialize publish event")?;
        match s3
            .put_object_bytes_if_none_match(&key, Bytes::from(body))
            .await?
        {
            ConditionalPut::Wrote => return Ok(next),
            ConditionalPut::PreconditionFailed => continue,
        }
    }
    bail!(
        "Failed to append publish event for creator '{creator_id}' after {MAX_EVENT_APPEND_RETRIES} retries"
    );
}

async fn next_event_seq(s3: &S3Client, creator_id: &str) -> Result<u64> {
    let prefix = keys::events_prefix(s3.prefix(), creator_id);
    let mut max_seq: u64 = 0;
    for key in s3.list_keys(&prefix).await? {
        if let Some(seq) = keys::parse_event_seq(&key)
            && seq > max_seq
        {
            max_seq = seq;
        }
    }
    Ok(max_seq + 1)
}

fn now_rfc3339() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .context("Failed to format current time as RFC3339")
}
