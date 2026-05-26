//! S3 key layout for per-content payload and manifest objects.
//!
//! ```text
//! {prefix}creators/{creator_id}/content/{content_id}/payload.tdf
//! {prefix}creators/{creator_id}/content/{content_id}/manifest.json
//! ```
//!
//! Catalog events live in an `iroh-docs` replica, not in S3 — see
//! [`crate::catalog::replica`] for their key layout.

pub fn content_payload_key(prefix: &str, creator_id: &str, content_id: &str) -> String {
    format!("{prefix}creators/{creator_id}/content/{content_id}/payload.tdf")
}

pub fn content_manifest_key(prefix: &str, creator_id: &str, content_id: &str) -> String {
    format!("{prefix}creators/{creator_id}/content/{content_id}/manifest.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_is_respected() {
        assert_eq!(
            content_payload_key("env/prod/", "alice", "abcd"),
            "env/prod/creators/alice/content/abcd/payload.tdf"
        );
        assert_eq!(
            content_manifest_key("env/prod/", "alice", "abcd"),
            "env/prod/creators/alice/content/abcd/manifest.json"
        );
    }

    #[test]
    fn empty_prefix_omits_leading_slash_segment() {
        assert_eq!(
            content_payload_key("", "alice", "abcd"),
            "creators/alice/content/abcd/payload.tdf"
        );
    }
}
