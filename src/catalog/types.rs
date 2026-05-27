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

/// Per-content manifest persisted alongside the payload in S3.
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

/// One publish event — the canonical unit in the catalog event log.
///
/// The log lives in an `iroh-docs` replica under
/// `creators/{creator_id}/events/{seq:020}`. Each event carries its own
/// CWT-derived [`EventAuthorization`] so a reader can verify the chain of
/// custody without trusting the authoring node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishEvent {
    pub seq: u64,
    pub creator_id: String,
    pub content_id: String,
    pub kind: PublishEventKind,
    pub published_at: String,
    pub entry: CatalogEntry,
    pub authorization: EventAuthorization,
}

/// Pinned to the CWT that authorized the publish. The token bytes are
/// preserved verbatim so a downstream verifier can re-run signature checks
/// against the issuer's key set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventAuthorization {
    /// Base64 (standard, padded) of the raw COSE_Sign1 CWT.
    pub cwt_b64: String,
    /// Copy of the CWT `iss` claim, for indexing without re-decoding.
    pub issuer: String,
    /// Hex of the CWT `cti` claim, for replay-analysis tooling.
    pub cti: String,
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

/// A reader-side projection of the event log for a single creator.
///
/// The event log is canonical; this struct is a disposable summary computed
/// on demand and is never written back to the replica. Two callers asking
/// for "the catalog" at the same replica state are free to compute
/// different projections (different sorts, filters) without disagreeing
/// about canonical state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogView {
    pub creator_id: String,
    pub entries: Vec<CatalogEntry>,
}

/// Outcome of a successful publish.
#[derive(Debug, Clone)]
pub struct PublishOutcome {
    pub content_id: String,
    pub seq: u64,
}

/// Node-authored event in the local log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentEvent {
    pub seq: u64,
    pub content_id: String,
    pub manifest_ref: String,
    pub attribute_value_fqns: Vec<String>,
    pub ingested_at: String,
}

/// Input to `EventStore::append`; seq is assigned by the store.
#[derive(Debug, Clone)]
pub struct NewContentEvent {
    pub content_id: String,
    pub manifest_ref: String,
    pub attribute_value_fqns: Vec<String>,
    pub ingested_at: String,
}
