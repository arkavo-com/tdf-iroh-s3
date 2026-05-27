use bytes::Bytes;
use tdf_iroh_s3::auth::cwt::VerifiedClaims;
use tdf_iroh_s3::auth::{cwt_to_entitlements, Grant};

fn vc_with_grants(grants: Vec<Grant>) -> VerifiedClaims {
    VerifiedClaims {
        subject: "alice".into(),
        raw_cwt: Bytes::new(),
        cti: String::new(),
        exp: 0, iat: 0,
        issuer: "iss".into(),
        grants,
    }
}

fn grant(fqn: &str, actions: &[&str]) -> Grant {
    Grant {
        fqn: fqn.to_string(),
        actions: actions.iter().map(|s| s.to_string()).collect(),
        locations: Vec::new(),
        obligations: Vec::new(),
    }
}

#[test]
fn collapses_grants_to_fqn_to_actions_map() {
    let grants = vec![
        grant("https://x/attr/a/value/1", &["read"]),
        grant("https://x/attr/b/value/2", &["read"]),
    ];
    let ents = cwt_to_entitlements(&vc_with_grants(grants));
    assert_eq!(ents.len(), 2);
    assert_eq!(ents["https://x/attr/a/value/1"], vec!["read".to_string()]);
    assert_eq!(ents["https://x/attr/b/value/2"], vec!["read".to_string()]);
}

#[test]
fn merges_duplicate_fqn_entries() {
    // Issuer mints two grants with the same FQN — actions should union.
    let grants = vec![
        grant("https://x/attr/a/value/1", &["read"]),
        grant("https://x/attr/a/value/1", &["read"]), // duplicate
    ];
    let ents = cwt_to_entitlements(&vc_with_grants(grants));
    assert_eq!(ents.len(), 1);
    // De-duped within a key — exactly one "read".
    assert_eq!(ents["https://x/attr/a/value/1"], vec!["read".to_string()]);
}

#[test]
fn empty_grants_yields_empty_entitlements() {
    let ents = cwt_to_entitlements(&vc_with_grants(Vec::new()));
    assert!(ents.is_empty());
}

#[test]
fn skips_grants_with_malformed_fqn() {
    let grants = vec![
        grant("https://x/attr/a/value/1", &["read"]),
        grant("not-a-url",                 &["read"]),
        grant("",                          &["read"]),
    ];
    let ents = cwt_to_entitlements(&vc_with_grants(grants));
    assert_eq!(ents.len(), 1);
    assert!(ents.contains_key("https://x/attr/a/value/1"));
}
