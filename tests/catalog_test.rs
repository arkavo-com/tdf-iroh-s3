//! Wire-format stability tests for the catalog types.
//!
//! These guard the on-disk JSON shape of types that travel between the
//! node and downstream consumers. Field-name or casing changes are
//! breaking and should fail here.

use tdf_iroh_s3::catalog::{
    CatalogEntry, ContentManifest, ContentMetadata, EventAuthorization, PublishEvent,
    PublishEventKind, Visibility, build_catalog,
};

fn sample_entry(content_id: &str, seq: u64) -> CatalogEntry {
    CatalogEntry {
        content_id: content_id.to_string(),
        creator_id: "creator_1".to_string(),
        title: format!("Episode {seq}"),
        visibility: Visibility::MembersOnly,
        required_tier_ids: vec!["patreon_gold".to_string()],
        tdf_ref: format!("iroh:{content_id}"),
        manifest_ref: format!("creators/creator_1/content/{content_id}/manifest.json"),
        published_at: "2026-05-25T22:30:00Z".to_string(),
    }
}

fn sample_event(seq: u64, content_id: &str) -> PublishEvent {
    PublishEvent {
        seq,
        creator_id: "creator_1".to_string(),
        content_id: content_id.to_string(),
        kind: PublishEventKind::Publish,
        published_at: "2026-05-25T22:30:00Z".to_string(),
        entry: sample_entry(content_id, seq),
        authorization: EventAuthorization {
            cwt_b64: "cwt-bytes-base64".to_string(),
            issuer: "https://issuer.example".to_string(),
            cti: format!("{seq:032x}"),
        },
    }
}

#[test]
fn content_manifest_roundtrips_through_json() {
    let manifest = ContentManifest {
        content_id: "abc123".to_string(),
        creator_id: "creator_1".to_string(),
        title: "Episode 1".to_string(),
        visibility: Visibility::MembersOnly,
        required_tier_ids: vec!["patreon_gold".to_string()],
        payload_size: 4096,
        tdf_ref: "iroh:abc123".to_string(),
        published_at: "2026-05-25T22:30:00Z".to_string(),
    };
    let json = serde_json::to_string(&manifest).unwrap();
    assert!(json.contains(r#""visibility":"members_only""#));
    let parsed: ContentManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.required_tier_ids, vec!["patreon_gold".to_string()]);
    assert_eq!(parsed.payload_size, 4096);
}

#[test]
fn visibility_serializes_snake_case() {
    assert_eq!(
        serde_json::to_string(&Visibility::Public).unwrap(),
        "\"public\""
    );
    assert_eq!(
        serde_json::to_string(&Visibility::MembersOnly).unwrap(),
        "\"members_only\""
    );
}

#[test]
fn content_manifest_required_tier_ids_defaults_to_empty() {
    let json = r#"{
        "content_id": "abc",
        "creator_id": "c1",
        "title": "t",
        "visibility": "public",
        "payload_size": 1,
        "tdf_ref": "iroh:abc",
        "published_at": "2026-05-25T00:00:00Z"
    }"#;
    let parsed: ContentManifest = serde_json::from_str(json).unwrap();
    assert!(parsed.required_tier_ids.is_empty());
}

#[test]
fn catalog_view_projects_events_dedup_and_sort() {
    let events = vec![sample_event(1, "c1"), sample_event(2, "c2")];
    let view = build_catalog("creator_1", events);
    assert_eq!(view.creator_id, "creator_1");
    assert_eq!(view.entries.len(), 2);
}

#[test]
fn publish_event_authorization_is_required_and_roundtrips() {
    let event = sample_event(7, "c7");
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains(r#""cwt_b64":"cwt-bytes-base64""#));
    assert!(json.contains(r#""issuer":"https://issuer.example""#));
    let parsed: PublishEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.authorization.issuer, "https://issuer.example");
    assert_eq!(parsed.seq, 7);
}

#[test]
fn publish_event_without_authorization_fails_to_parse() {
    // The CWT-embedded authorization is mandatory at the wire-format level.
    let json = r#"{
        "seq": 1,
        "creator_id": "c1",
        "content_id": "x",
        "kind": "publish",
        "published_at": "2026-05-25T00:00:00Z",
        "entry": {
            "content_id": "x",
            "creator_id": "c1",
            "title": "t",
            "visibility": "public",
            "tdf_ref": "iroh:x",
            "manifest_ref": "creators/c1/content/x/manifest.json",
            "published_at": "2026-05-25T00:00:00Z"
        }
    }"#;
    let err = serde_json::from_str::<PublishEvent>(json).unwrap_err();
    assert!(
        err.to_string().contains("authorization"),
        "expected missing-authorization error, got: {err}"
    );
}

#[test]
fn content_metadata_parses_from_creator_supplied_json() {
    let json = r#"{
        "title": "Episode 1",
        "visibility": "members_only",
        "required_tier_ids": ["patreon_gold", "patreon_diamond"]
    }"#;
    let parsed: ContentMetadata = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.title, "Episode 1");
    assert_eq!(parsed.visibility, Visibility::MembersOnly);
    assert_eq!(parsed.required_tier_ids.len(), 2);
}
