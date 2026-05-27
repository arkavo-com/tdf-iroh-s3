use serde::{Deserialize, Serialize};

/// Node-authored event in the local log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentEvent {
    pub seq: u64,
    pub content_id: String,
    pub manifest_ref: String,
    pub attribute_value_fqns: Vec<String>,
    pub ingested_at: String,
}

#[derive(Debug, Clone)]
pub struct NewContentEvent {
    pub content_id: String,
    pub manifest_ref: String,
    pub attribute_value_fqns: Vec<String>,
    pub ingested_at: String,
}
