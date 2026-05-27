//! In-process CWT issuer used by unit and integration tests.
//!
//! Mints Arkavo CWT v1 contract-conforming tokens: ES256 COSE_Sign1 with
//! `iss`, `sub`, `iat`, `exp` (windowed to <=3600s), `scope: catalog.read`,
//! `cnf.iroh_node_id` as a 32-byte byte string, and a non-empty
//! `authorization_details` array of `tdf_attribute` grants with `read` actions.

use ciborium::Value;
use coset::cwt::{ClaimsSetBuilder, Timestamp};
use coset::{CborSerializable, CoseKeyBuilder, CoseKeySet, CoseSign1Builder, HeaderBuilder, iana};
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use p256::elliptic_curve::rand_core::OsRng;
use std::collections::HashMap;
use std::sync::Arc;

use super::CoseKeyCache;

pub struct TestSigner {
    signing_key: SigningKey,
    verifying_key: p256::ecdsa::VerifyingKey,
    pub kid: Vec<u8>,
    pub issuer: String,
}

impl TestSigner {
    pub fn new(issuer: impl Into<String>) -> Self {
        let signing_key = SigningKey::random(&mut OsRng);
        let verifying_key = *signing_key.verifying_key();
        Self {
            signing_key,
            verifying_key,
            kid: b"test-kid-1".to_vec(),
            issuer: issuer.into(),
        }
    }

    /// Encode this signer's public key as a CBOR COSE_KeySet, the format
    /// served by the production identity endpoint.
    pub fn cose_key_set(&self) -> Vec<u8> {
        let point = self.verifying_key.to_encoded_point(false);
        let x = point.x().expect("uncompressed point has x").to_vec();
        let y = point.y().expect("uncompressed point has y").to_vec();
        let key = CoseKeyBuilder::new_ec2_pub_key(iana::EllipticCurve::P_256, x, y)
            .algorithm(iana::Algorithm::ES256)
            .key_id(self.kid.clone())
            .build();
        CoseKeySet(vec![key])
            .to_vec()
            .expect("CoseKeySet serializes")
    }

    /// Build an in-memory `CoseKeyCache` populated with this signer's public
    /// key. Skips the HTTP path entirely. The raw CBOR keyset is seeded
    /// alongside the parsed map so [`crate::auth::Verifier`] (which calls
    /// `pep_check::verify_cose_sign1` with the raw bytes) can verify
    /// signatures without an HTTP fetch.
    pub fn cose_key_cache(&self) -> Arc<CoseKeyCache> {
        let mut map = HashMap::new();
        map.insert(self.kid.clone(), self.verifying_key);
        CoseKeyCache::new_static(map, bytes::Bytes::from(self.cose_key_set()))
    }

    /// Mint a v1-contract CWT.
    pub fn mint(&self, claims: TestClaims) -> Vec<u8> {
        // Build authorization_details array
        let auth_details = Value::Array(
            claims
                .grants
                .into_iter()
                .map(|g| {
                    Value::Map(vec![
                        (Value::Text("type".into()), Value::Text(g.grant_type)),
                        (Value::Text("fqn".into()), Value::Text(g.fqn)),
                        (
                            Value::Text("actions".into()),
                            Value::Array(g.actions.into_iter().map(Value::Text).collect()),
                        ),
                    ])
                })
                .collect(),
        );

        let mut builder = ClaimsSetBuilder::new()
            .issuer(claims.issuer.unwrap_or_else(|| self.issuer.clone()))
            .subject(claims.subject)
            .issued_at(Timestamp::WholeSeconds(claims.iat))
            .expiration_time(Timestamp::WholeSeconds(claims.exp));

        if let Some(cti) = claims.cti {
            builder = builder.cwt_id(cti);
        }

        // scope (integer-9 IANA key)
        builder = builder.claim(iana::CwtClaimName::Scope, Value::Text(claims.scope));

        // authorization_details (text key per contract A.2)
        builder = builder.text_claim("authorization_details".into(), auth_details);

        // cnf.iroh_node_id as BYTE STRING (32 bytes), inside cnf (integer 8) map
        if let Some(node_id_hex) = claims.cnf_iroh_node_id {
            // Caller passes a hex string; decode to bytes.
            let node_id_bytes = hex::decode(&node_id_hex)
                .expect("test cnf.iroh_node_id must be valid hex");
            assert_eq!(node_id_bytes.len(), 32, "iroh_node_id must be 32 bytes");
            let cnf = Value::Map(vec![(
                Value::Text("iroh_node_id".into()),
                Value::Bytes(node_id_bytes),
            )]);
            builder = builder.claim(iana::CwtClaimName::Cnf, cnf);
        }

        let claims_set = builder.build();
        let payload = claims_set
            .to_vec()
            .expect("ClaimsSet serializes to CBOR");

        let protected = HeaderBuilder::new()
            .algorithm(iana::Algorithm::ES256)
            .key_id(self.kid.clone())
            .build();

        let sk = self.signing_key.clone();
        let sign1 = CoseSign1Builder::new()
            .protected(protected)
            .payload(payload)
            .create_signature(&[], move |tbs| {
                let sig: Signature = sk.sign(tbs);
                sig.to_bytes().to_vec()
            })
            .build();

        sign1.to_vec().expect("COSE_Sign1 serializes")
    }
}

/// One `authorization_details` entry to be minted into the token.
pub struct TestGrant {
    pub grant_type: String,
    pub fqn: String,
    pub actions: Vec<String>,
}

impl TestGrant {
    pub fn read(fqn: impl Into<String>) -> Self {
        Self {
            grant_type: "tdf_attribute".into(),
            fqn: fqn.into(),
            actions: vec!["read".into()],
        }
    }
}

pub struct TestClaims {
    pub subject: String,
    pub scope: String,
    pub iat: i64,
    pub exp: i64,
    pub cti: Option<Vec<u8>>,
    /// Hex-encoded 32-byte iroh NodeId. The signer asserts validity on mint.
    pub cnf_iroh_node_id: Option<String>,
    pub issuer: Option<String>,
    pub grants: Vec<TestGrant>,
}

impl TestClaims {
    /// Defaults: `catalog.read` scope, valid for 5 minutes, one grant.
    pub fn defaults(subject: impl Into<String>, fqn: impl Into<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Self {
            subject: subject.into(),
            scope: "catalog.read".into(),
            iat: now,
            exp: now + 300,
            cti: Some(b"test-cti-001".to_vec()),
            cnf_iroh_node_id: None,
            issuer: None,
            grants: vec![TestGrant::read(fqn.into())],
        }
    }
}
