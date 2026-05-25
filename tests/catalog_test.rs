//! Verifies the on-disk JSON shape of the catalog types is stable. The
//! consumer app and any external tooling read these documents, so changes
//! to field names or casing are breaking changes worth catching in tests.

use tdf_iroh_s3::catalog::{
    Catalog, CatalogEntry, ContentManifest, ContentMetadata, PublishEvent, PublishEventKind,
    Visibility, build_catalog,
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
fn full_catalog_roundtrips_with_stable_signature() {
    let events = vec![
        PublishEvent {
            seq: 1,
            creator_id: "creator_1".to_string(),
            content_id: "c1".to_string(),
            kind: PublishEventKind::Publish,
            published_at: "2026-05-25T10:00:00Z".to_string(),
            entry: sample_entry("c1", 1),
        },
        PublishEvent {
            seq: 2,
            creator_id: "creator_1".to_string(),
            content_id: "c2".to_string(),
            kind: PublishEventKind::Publish,
            published_at: "2026-05-25T11:00:00Z".to_string(),
            entry: sample_entry("c2", 2),
        },
    ];

    let draft = build_catalog("creator_1", events);
    let catalog = draft.finalize("2026-05-25T12:00:00Z".to_string());

    assert_eq!(catalog.version, 2);
    assert_eq!(catalog.entries.len(), 2);
    assert_eq!(catalog.signature.method, "blake3-unsigned-placeholder");
    assert_eq!(catalog.signature.value.len(), 64); // BLAKE3 hex

    let json = serde_json::to_string(&catalog).unwrap();
    let parsed: Catalog = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.signature.value, catalog.signature.value);
    assert_eq!(parsed.entries[0].content_id, catalog.entries[0].content_id);
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
