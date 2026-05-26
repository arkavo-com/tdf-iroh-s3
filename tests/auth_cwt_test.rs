//! End-to-end CWT verification tests.
//!
//! Uses the in-process `TestSigner` to mint CWTs. Some tests bypass HTTP by
//! constructing a `CoseKeyCache` directly; one test exercises the real
//! HTTP fetch path against a local server serving `application/cose-key-
//! set+cbor`.

use std::sync::Arc;
use std::time::Duration;
use tdf_iroh_s3::auth::test_signer::{TestClaims, TestSigner};
use tdf_iroh_s3::auth::{CoseKeyCache, Verifier, VerifyError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const ISSUER: &str = "https://issuer.example";

fn verifier(signer: &TestSigner) -> Verifier {
    Verifier::new(signer.cose_key_cache(), ISSUER.to_string(), 60)
}

async fn serve_cose_keys_once(body: Vec<u8>) -> String {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/cose-key-set+cbor\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn verifies_a_freshly_minted_cwt() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);

    let cwt = signer.mint(TestClaims::defaults("creator_1", "campaign_42"));

    let claims = v.verify(&cwt, None).await.expect("valid CWT must verify");
    assert_eq!(claims.creator_id, "creator_1");
    assert_eq!(claims.campaign_id, "campaign_42");
    assert_eq!(claims.issuer, ISSUER);
    assert_eq!(claims.raw_cwt.as_ref(), cwt.as_slice());
    assert!(!claims.cti.is_empty());
    assert!(claims.exp > 0);
}

#[tokio::test]
async fn rejects_wrong_issuer() {
    let signer = TestSigner::new("https://attacker.example");
    let v = verifier(&signer);

    let cwt = signer.mint(TestClaims::defaults("creator_1", "campaign_42"));

    match v.verify(&cwt, None).await {
        Err(VerifyError::WrongIssuer { expected, got }) => {
            assert_eq!(expected, ISSUER);
            assert_eq!(got.as_deref(), Some("https://attacker.example"));
        }
        other => panic!("expected WrongIssuer, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_expired_cwt() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);

    let mut claims = TestClaims::defaults("creator_1", "campaign_42");
    let long_ago = claims.iat - 10_000;
    claims.iat = long_ago;
    claims.exp = long_ago + 60; // expired well past the 60s skew window

    let cwt = signer.mint(claims);

    match v.verify(&cwt, None).await {
        Err(VerifyError::Expired { .. }) => {}
        other => panic!("expected Expired, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_future_iat() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);

    let mut claims = TestClaims::defaults("creator_1", "campaign_42");
    claims.iat += 10_000;
    claims.exp = claims.iat + 300;

    let cwt = signer.mint(claims);

    match v.verify(&cwt, None).await {
        Err(VerifyError::NotYetValid { .. }) => {}
        other => panic!("expected NotYetValid, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_tampered_signature() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);

    let mut cwt = signer.mint(TestClaims::defaults("creator_1", "campaign_42"));
    let last = cwt.len() - 5;
    cwt[last] ^= 0x01;

    match v.verify(&cwt, None).await {
        Err(VerifyError::BadSignature) | Err(VerifyError::Parse(_)) => {}
        other => panic!("expected BadSignature, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_missing_scope() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);

    let mut claims = TestClaims::defaults("creator_1", "campaign_42");
    claims.scope = "read.only".to_string();
    let cwt = signer.mint(claims);

    match v.verify(&cwt, None).await {
        Err(VerifyError::MissingScope("catalog.write")) => {}
        other => panic!("expected MissingScope, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_empty_subject() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);

    let mut claims = TestClaims::defaults("creator_1", "campaign_42");
    claims.subject = String::new();
    let cwt = signer.mint(claims);

    match v.verify(&cwt, None).await {
        Err(VerifyError::MissingClaim("sub")) => {}
        other => panic!("expected MissingClaim(sub), got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_unknown_kid() {
    let mut signer_a = TestSigner::new(ISSUER);
    signer_a.kid = b"kid-not-in-keyset".to_vec();
    let signer_b = TestSigner::new(ISSUER); // cache only carries test-kid-1

    let v = Verifier::new(signer_b.cose_key_cache(), ISSUER.to_string(), 60);
    let cwt = signer_a.mint(TestClaims::defaults("creator_1", "campaign_42"));

    match v.verify(&cwt, None).await {
        Err(VerifyError::UnknownKid(kid)) => {
            assert_eq!(kid, hex::encode(b"kid-not-in-keyset"));
        }
        other => panic!("expected UnknownKid, got {other:?}"),
    }
}

#[tokio::test]
async fn verifies_via_real_cose_keys_http_fetch() {
    let signer = TestSigner::new(ISSUER);
    let url = serve_cose_keys_once(signer.cose_key_set()).await;
    let cache = CoseKeyCache::spawn(url, Duration::from_secs(3600), reqwest::Client::new())
        .await
        .expect("initial COSE_KeySet fetch");
    let v = Verifier::new(Arc::clone(&cache), ISSUER.to_string(), 60);

    let cwt = signer.mint(TestClaims::defaults("creator_1", "campaign_42"));
    let claims = v.verify(&cwt, None).await.expect("real-HTTP CWT verifies");
    assert_eq!(claims.creator_id, "creator_1");
    assert_eq!(claims.campaign_id, "campaign_42");
}

#[tokio::test]
async fn enforces_cnf_node_id_when_present() {
    let signer = TestSigner::new(ISSUER);
    let v = verifier(&signer);

    let mut claims = TestClaims::defaults("creator_1", "campaign_42");
    claims.cnf_iroh_node_id = Some("aaaa".to_string());
    let cwt = signer.mint(claims);

    match v.verify(&cwt, Some("bbbb")).await {
        Err(VerifyError::NodeIdMismatch { .. }) => {}
        other => panic!("expected NodeIdMismatch, got {other:?}"),
    }
    v.verify(&cwt, Some("aaaa")).await.expect("matching node id");
    v.verify(&cwt, None).await.expect("no bound node id supplied");
}
