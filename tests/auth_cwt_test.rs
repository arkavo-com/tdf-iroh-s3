//! Verifier acceptance/rejection tests against the Arkavo CWT v1 contract.

use tdf_iroh_s3::auth::test_signer::{TestClaims, TestSigner};
use tdf_iroh_s3::auth::{Verifier, VerifyError};

const ISSUER: &str = "https://issuer.example";
const FQN_A: &str = "https://example/attr/dept/value/eng";
// 32-byte iroh NodeId encoded as 64 hex chars (the verifier hex-decodes
// and expects exactly 32 bytes).
const NODE_ID_A: &str =
    "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234";

fn verifier(signer: &TestSigner) -> Verifier {
    Verifier::new(signer.cose_key_cache(), ISSUER.to_string(), 60)
}

#[tokio::test]
async fn verifies_a_freshly_minted_cwt() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);
    let mut claims = TestClaims::defaults("alice", FQN_A);
    claims.cnf_iroh_node_id = Some(NODE_ID_A.into());
    let cwt = signer.mint(claims);

    let vc = v.verify(&cwt, NODE_ID_A).await.expect("valid CWT must verify");
    assert_eq!(vc.subject, "alice");
    assert_eq!(vc.issuer, ISSUER);
    assert!(vc.exp > 0);
    assert_eq!(vc.grants.len(), 1);
    assert_eq!(vc.grants[0].fqn, FQN_A);
    assert!(vc.grants[0].actions.iter().any(|a| a == "read"));
}

#[tokio::test]
async fn rejects_expired_cwt() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);
    let mut claims = TestClaims::defaults("alice", FQN_A);
    claims.cnf_iroh_node_id = Some(NODE_ID_A.into());
    claims.iat -= 7200;
    claims.exp -= 3600;
    let cwt = signer.mint(claims);
    match v.verify(&cwt, NODE_ID_A).await {
        Err(VerifyError::Expired { .. }) => {}
        other => panic!("expected Expired, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_wrong_issuer() {
    let signer = TestSigner::new("https://attacker.example");
    // Verifier expects ISSUER; signer mints with a different `iss`.
    let v = Verifier::new(signer.cose_key_cache(), ISSUER.to_string(), 60);
    let mut claims = TestClaims::defaults("alice", FQN_A);
    claims.cnf_iroh_node_id = Some(NODE_ID_A.into());
    let cwt = signer.mint(claims);
    match v.verify(&cwt, NODE_ID_A).await {
        Err(VerifyError::WrongIssuer { .. }) => {}
        other => panic!("expected WrongIssuer, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_missing_scope_catalog_read() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);
    let mut claims = TestClaims::defaults("alice", FQN_A);
    claims.cnf_iroh_node_id = Some(NODE_ID_A.into());
    claims.scope = "openid profile".into();
    let cwt = signer.mint(claims);
    match v.verify(&cwt, NODE_ID_A).await {
        Err(VerifyError::MissingScope(_)) => {}
        other => panic!("expected MissingScope, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_node_id_mismatch() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);
    let mut claims = TestClaims::defaults("alice", FQN_A);
    claims.cnf_iroh_node_id = Some(NODE_ID_A.into());
    let cwt = signer.mint(claims);
    // Verify against a DIFFERENT 32-byte hex id.
    let other_id = "ffff".repeat(16);
    match v.verify(&cwt, &other_id).await {
        Err(VerifyError::NodeIdMismatch { .. }) => {}
        other => panic!("expected NodeIdMismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_missing_cnf_when_iroh_bound() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);
    let claims = TestClaims::defaults("alice", FQN_A); // no cnf_iroh_node_id
    let cwt = signer.mint(claims);
    match v.verify(&cwt, NODE_ID_A).await {
        Err(VerifyError::MissingNodeIdBinding) => {}
        other => panic!("expected MissingNodeIdBinding, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_malformed_connection_binding() {
    // Verifier is handed a bound_node_id that's not 32-byte hex — that's a
    // programmer bug at the ALPN handler. Should error distinctly, not as
    // NodeIdMismatch.
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);
    let mut claims = TestClaims::defaults("alice", FQN_A);
    claims.cnf_iroh_node_id = Some(NODE_ID_A.into());
    let cwt = signer.mint(claims);
    match v.verify(&cwt, "not-hex").await {
        Err(VerifyError::MalformedConnectionBinding) => {}
        other => panic!("expected MalformedConnectionBinding, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_empty_authorization_details() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);
    let mut claims = TestClaims::defaults("alice", FQN_A);
    claims.cnf_iroh_node_id = Some(NODE_ID_A.into());
    claims.grants.clear();
    let cwt = signer.mint(claims);
    match v.verify(&cwt, NODE_ID_A).await {
        Err(VerifyError::MissingAuthDetails) => {}
        other => panic!("expected MissingAuthDetails, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_unknown_action_in_grants() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);
    let mut claims = TestClaims::defaults("alice", FQN_A);
    claims.cnf_iroh_node_id = Some(NODE_ID_A.into());
    claims.grants[0].actions.push("write".into());
    let cwt = signer.mint(claims);
    match v.verify(&cwt, NODE_ID_A).await {
        Err(VerifyError::UnknownAction) => {}
        other => panic!("expected UnknownAction, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_iat_too_far_in_future() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);
    let mut claims = TestClaims::defaults("alice", FQN_A);
    claims.cnf_iroh_node_id = Some(NODE_ID_A.into());
    claims.iat += 3600;
    claims.exp += 3600;
    let cwt = signer.mint(claims);
    match v.verify(&cwt, NODE_ID_A).await {
        Err(VerifyError::NotYetValid { .. }) => {}
        other => panic!("expected NotYetValid, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_window_too_wide() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);
    let mut claims = TestClaims::defaults("alice", FQN_A);
    claims.cnf_iroh_node_id = Some(NODE_ID_A.into());
    // Open a window > 3600s
    claims.exp = claims.iat + 7200;
    let cwt = signer.mint(claims);
    match v.verify(&cwt, NODE_ID_A).await {
        Err(VerifyError::WindowTooWide { .. }) => {}
        other => panic!("expected WindowTooWide, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_bad_signature() {
    // Sign with signer A's key, verify against signer B's key set.
    let signer_a = TestSigner::new(ISSUER);
    let signer_b = TestSigner::new(ISSUER);
    let v = Verifier::new(signer_b.cose_key_cache(), ISSUER.to_string(), 60);
    let mut claims = TestClaims::defaults("alice", FQN_A);
    claims.cnf_iroh_node_id = Some(NODE_ID_A.into());
    let cwt = signer_a.mint(claims);
    match v.verify(&cwt, NODE_ID_A).await {
        Err(VerifyError::BadSignature) => {}
        other => panic!("expected BadSignature, got {other:?}"),
    }
}
