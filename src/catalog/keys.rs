//! S3 key layout for the per-creator catalog system.
//!
//! ```text
//! {prefix}creators/{creator_id}/content/{content_id}/payload.tdf
//! {prefix}creators/{creator_id}/content/{content_id}/manifest.json
//! {prefix}creators/{creator_id}/events/{seq}.publish.json
//! {prefix}creators/{creator_id}/catalog/latest.json
//! {prefix}creators/{creator_id}/catalog/snapshots/{version}.json
//! ```
//!
//! Event and snapshot filenames are zero-padded to 20 digits so listing the
//! prefix returns keys in ascending sequence order.

pub const EVENT_SEQ_WIDTH: usize = 20;

pub fn creator_root(prefix: &str, creator_id: &str) -> String {
    format!("{prefix}creators/{creator_id}/")
}

pub fn content_payload_key(prefix: &str, creator_id: &str, content_id: &str) -> String {
    format!("{prefix}creators/{creator_id}/content/{content_id}/payload.tdf")
}

pub fn content_manifest_key(prefix: &str, creator_id: &str, content_id: &str) -> String {
    format!("{prefix}creators/{creator_id}/content/{content_id}/manifest.json")
}

pub fn events_prefix(prefix: &str, creator_id: &str) -> String {
    format!("{prefix}creators/{creator_id}/events/")
}

pub fn event_key(prefix: &str, creator_id: &str, seq: u64) -> String {
    format!(
        "{prefix}creators/{creator_id}/events/{seq:0width$}.publish.json",
        width = EVENT_SEQ_WIDTH
    )
}

pub fn catalog_latest_key(prefix: &str, creator_id: &str) -> String {
    format!("{prefix}creators/{creator_id}/catalog/latest.json")
}

pub fn catalog_snapshot_key(prefix: &str, creator_id: &str, version: u64) -> String {
    format!(
        "{prefix}creators/{creator_id}/catalog/snapshots/{version:0width$}.json",
        width = EVENT_SEQ_WIDTH
    )
}

/// Parse the seq number out of an event object key.
pub fn parse_event_seq(key: &str) -> Option<u64> {
    let filename = key.rsplit('/').next()?;
    let digits = filename.strip_suffix(".publish.json")?;
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_keys_sort_numerically_under_lexicographic_listing() {
        let mut keys = [
            event_key("", "creator_a", 10),
            event_key("", "creator_a", 2),
            event_key("", "creator_a", 1),
            event_key("", "creator_a", 100),
        ];
        keys.sort();
        let seqs: Vec<u64> = keys.iter().filter_map(|k| parse_event_seq(k)).collect();
        assert_eq!(seqs, vec![1, 2, 10, 100]);
    }

    #[test]
    fn prefix_is_respected() {
        assert_eq!(
            content_payload_key("env/prod/", "alice", "abcd"),
            "env/prod/creators/alice/content/abcd/payload.tdf"
        );
        assert_eq!(
            catalog_latest_key("env/prod/", "alice"),
            "env/prod/creators/alice/catalog/latest.json"
        );
    }

    #[test]
    fn parse_event_seq_extracts_padded_number() {
        let key = event_key("p/", "c", 42);
        assert_eq!(parse_event_seq(&key), Some(42));
    }

    #[test]
    fn parse_event_seq_rejects_unrelated_keys() {
        assert_eq!(parse_event_seq("p/creators/c/content/x/manifest.json"), None);
        assert_eq!(parse_event_seq(""), None);
    }
}
