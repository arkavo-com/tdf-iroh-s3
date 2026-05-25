use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    MembersOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishEventKind {
    Publish,
}

/// Metadata supplied by the creator at publish time. Drives catalog filtering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentMetadata {
    pub title: String,
    pub visibility: Visibility,
    #[serde(default)]
    pub required_tier_ids: Vec<String>,
}

/// Per-content manifest persisted alongside the payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentManifest {
    pub content_id: String,
    pub creator_id: String,
    pub title: String,
    pub visibility: Visibility,
    #[serde(default)]
    pub required_tier_ids: Vec<String>,
    pub payload_size: u64,
    pub tdf_ref: String,
    pub published_at: String,
}

/// Append-only publish event. The catalog is derived deterministically from
/// the ordered set of events for a creator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishEvent {
    pub seq: u64,
    pub creator_id: String,
    pub content_id: String,
    pub kind: PublishEventKind,
    pub published_at: String,
    pub entry: CatalogEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub content_id: String,
    pub creator_id: String,
    pub title: String,
    pub visibility: Visibility,
    #[serde(default)]
    pub required_tier_ids: Vec<String>,
    pub tdf_ref: String,
    pub manifest_ref: String,
    pub published_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub creator_id: String,
    pub version: u64,
    pub generated_at: String,
    pub entries: Vec<CatalogEntry>,
    pub signature: CatalogSignature,
}

/// Detached signature over the canonical-bytes serialization of the catalog
/// body (everything except the `signature` field itself).
///
/// `method = "blake3-unsigned-placeholder"` is the current scheme: a BLAKE3
/// digest of the canonical body. It is NOT cryptographically authenticated —
/// it only detects accidental tampering. The replacement is ed25519 once a
/// catalog signing key is wired through config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSignature {
    pub method: String,
    pub value: String,
}

/// Outcome of a successful publish.
#[derive(Debug, Clone)]
pub struct PublishOutcome {
    pub content_id: String,
    pub seq: u64,
    pub version: u64,
}
