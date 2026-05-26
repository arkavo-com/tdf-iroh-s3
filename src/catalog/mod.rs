//! Per-creator catalog system.
//!
//! **The event log is the catalog.** Each event in the `iroh-docs` replica
//! (see [`replica`]) is the authoritative record of one publish,
//! authenticated by the CWT embedded in its
//! [`EventAuthorization`](types::EventAuthorization). The
//! [`CatalogView`](types::CatalogView) returned by [`build_catalog`] is a
//! reader-side **projection**: derived on demand, disposable, never
//! written back, and not required to be byte-identical between callers.
//!
//! Design rules:
//! - No database — iroh-docs is the substrate.
//! - No continuous builder — projections are computed on read.
//! - No signed snapshot — per-event authenticity is the CWT in the event.

pub mod keys;
pub mod publish;
pub mod replica;
pub mod types;

pub use types::*;

/// Build the canonical projection from an ordered list of publish events.
///
/// "Canonical" here means: the same set of events produces the same
/// `CatalogView` regardless of input order. Specifically:
///
/// - Dedupe by `content_id`: keep the highest-`seq` event per content_id.
///   Re-publishing the same content_id replaces its catalog entry.
/// - Sort entries by `published_at` descending (ties broken by seq desc).
///
/// This is a pure function of the event set — the projection is disposable.
pub fn build_catalog(creator_id: &str, mut events: Vec<PublishEvent>) -> CatalogView {
    events.sort_by_key(|e| e.seq);

    let mut latest_per_content: std::collections::HashMap<String, PublishEvent> =
        std::collections::HashMap::new();
    for event in events {
        latest_per_content.insert(event.content_id.clone(), event);
    }

    let mut entries: Vec<(u64, CatalogEntry)> = latest_per_content
        .into_values()
        .map(|e| (e.seq, e.entry))
        .collect();

    entries.sort_by(|a, b| {
        b.1.published_at
            .cmp(&a.1.published_at)
            .then_with(|| b.0.cmp(&a.0))
    });

    CatalogView {
        creator_id: creator_id.to_string(),
        entries: entries.into_iter().map(|(_, e)| e).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(seq: u64, content_id: &str, published_at: &str) -> PublishEvent {
        PublishEvent {
            seq,
            creator_id: "creator_1".to_string(),
            content_id: content_id.to_string(),
            kind: PublishEventKind::Publish,
            published_at: published_at.to_string(),
            entry: CatalogEntry {
                content_id: content_id.to_string(),
                creator_id: "creator_1".to_string(),
                title: format!("title-{seq}"),
                visibility: Visibility::Public,
                required_tier_ids: vec![],
                tdf_ref: format!("iroh:{content_id}"),
                manifest_ref: format!(
                    "creators/creator_1/content/{content_id}/manifest.json"
                ),
                published_at: published_at.to_string(),
            },
            authorization: EventAuthorization {
                cwt_b64: "test-cwt".to_string(),
                issuer: "https://issuer.example".to_string(),
                cti: format!("{seq:032x}"),
            },
        }
    }

    #[test]
    fn empty_event_log_yields_empty_projection() {
        let view = build_catalog("creator_1", vec![]);
        assert!(view.entries.is_empty());
        assert_eq!(view.creator_id, "creator_1");
    }

    #[test]
    fn republishing_content_id_replaces_entry_keeps_latest_seq() {
        let events = vec![
            make_event(1, "content_a", "2026-05-25T10:00:00Z"),
            make_event(2, "content_b", "2026-05-25T11:00:00Z"),
            make_event(3, "content_a", "2026-05-25T12:00:00Z"),
        ];
        let view = build_catalog("creator_1", events);
        assert_eq!(view.entries.len(), 2);
        let a = view
            .entries
            .iter()
            .find(|e| e.content_id == "content_a")
            .unwrap();
        assert_eq!(a.title, "title-3");
    }

    #[test]
    fn entries_sorted_by_published_at_descending() {
        let events = vec![
            make_event(1, "content_a", "2026-05-25T10:00:00Z"),
            make_event(2, "content_b", "2026-05-26T10:00:00Z"),
            make_event(3, "content_c", "2026-05-24T10:00:00Z"),
        ];
        let view = build_catalog("creator_1", events);
        let ids: Vec<&str> = view.entries.iter().map(|e| e.content_id.as_str()).collect();
        assert_eq!(ids, vec!["content_b", "content_a", "content_c"]);
    }

    #[test]
    fn build_catalog_is_input_order_independent() {
        let a = make_event(1, "content_a", "2026-05-25T10:00:00Z");
        let b = make_event(2, "content_b", "2026-05-26T10:00:00Z");
        let forward = build_catalog("c", vec![a.clone(), b.clone()]);
        let reverse = build_catalog("c", vec![b, a]);
        let forward_ids: Vec<&str> = forward.entries.iter().map(|e| e.content_id.as_str()).collect();
        let reverse_ids: Vec<&str> = reverse.entries.iter().map(|e| e.content_id.as_str()).collect();
        assert_eq!(forward_ids, reverse_ids);
    }
}
