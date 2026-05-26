//! COSE_Sign1 / CWT verification.
//!
//! The verifier is constructed with an issuer name, a JWKS cache, and a
//! permitted clock skew (seconds). [`Verifier::verify`] parses a
//! COSE_Sign1, looks the key up by `kid`, verifies the ES256 signature,
//! and walks the CWT claims set to enforce issuer, `exp`, `iat`, and the
//! `scope` / `sub` / `campaign_id` shape we require for catalog writes.

use bytes::Bytes;
use coset::iana::EnumI64;
use coset::{AsCborValue, CborSerializable, CoseSign1, iana, cwt::ClaimsSet, cwt::Timestamp};
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier as _};
use std::sync::Arc;
use thiserror::Error;

use super::SCOPE_CATALOG_WRITE;
use super::cose_keys::CoseKeyCache;

/// Claim key for the OAuth-style `scope` string in a CWT.
/// IANA registration: 9 (RFC 8693).
const CLAIM_SCOPE: i64 = 9;
/// Confirmation claim (`cnf`). IANA registration: 8 (RFC 8747).
const CLAIM_CNF: i64 = 8;
/// Custom text claim — campaign identifier the token is bound to.
const CLAIM_CAMPAIGN_ID: &str = "campaign_id";
/// Custom text claim inside `cnf` — iroh NodeId (hex) the token is bound to.
const CNF_IROH_NODE_ID: &str = "iroh_node_id";

#[derive(Debug, Clone)]
pub struct VerifiedClaims {
    pub creator_id: String,
    pub campaign_id: String,
    pub raw_cwt: Bytes,
    pub cti: String,
    pub exp: i64,
    pub issuer: String,
}

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("cwt: failed to parse COSE_Sign1: {0}")]
    Parse(String),
    #[error("cwt: unsupported alg (only ES256 / -7 accepted)")]
    UnsupportedAlg,
    #[error("cwt: missing kid in protected header")]
    MissingKid,
    #[error("cwt: unknown kid '{0}'")]
    UnknownKid(String), // hex-encoded for diagnostics
    #[error("cwt: signature verification failed")]
    BadSignature,
    #[error("cwt: failed to decode payload: {0}")]
    BadPayload(String),
    #[error("cwt: wrong issuer (expected '{expected}', got '{got:?}')")]
    WrongIssuer {
        expected: String,
        got: Option<String>,
    },
    #[error("cwt: expired (exp={exp}, now={now})")]
    Expired { exp: i64, now: i64 },
    #[error("cwt: issued in the future (iat={iat}, now={now})")]
    NotYetValid { iat: i64, now: i64 },
    #[error("cwt: missing or empty claim '{0}'")]
    MissingClaim(&'static str),
    #[error("cwt: scope missing required '{0}'")]
    MissingScope(&'static str),
    #[error("cwt: cnf.iroh_node_id mismatch (token='{token}', connection='{connection}')")]
    NodeIdMismatch { token: String, connection: String },
}

pub struct Verifier {
    keys: Arc<CoseKeyCache>,
    issuer: String,
    clock_skew_secs: i64,
}

impl Verifier {
    pub fn new(keys: Arc<CoseKeyCache>, issuer: String, clock_skew_secs: i64) -> Self {
        Self {
            keys,
            issuer,
            clock_skew_secs,
        }
    }

    pub async fn verify(
        &self,
        cwt: &[u8],
        bound_node_id: Option<&str>,
    ) -> Result<VerifiedClaims, VerifyError> {
        // Accept both a bare COSE_Sign1 and a tag(18) wrapper.
        let sign1 = parse_sign1(cwt)?;

        let kid = sign1.protected.header.key_id.clone();
        if kid.is_empty() {
            return Err(VerifyError::MissingKid);
        }

        match sign1.protected.header.alg {
            Some(coset::Algorithm::Assigned(iana::Algorithm::ES256)) => {}
            _ => return Err(VerifyError::UnsupportedAlg),
        }

        let key = match self.keys.get(&kid) {
            Some(k) => k,
            None => {
                self.keys.force_refresh().await;
                self.keys
                    .get(&kid)
                    .ok_or_else(|| VerifyError::UnknownKid(hex::encode(&kid)))?
            }
        };

        verify_signature(&sign1, &key)?;

        let payload = sign1
            .payload
            .as_ref()
            .ok_or_else(|| VerifyError::BadPayload("missing payload".into()))?;
        let claims =
            decode_claims(payload).map_err(|e| VerifyError::BadPayload(e.to_string()))?;

        let now = now_unix();
        let skew = self.clock_skew_secs;

        let got_issuer = claims.issuer.clone();
        if got_issuer.as_deref() != Some(self.issuer.as_str()) {
            return Err(VerifyError::WrongIssuer {
                expected: self.issuer.clone(),
                got: got_issuer,
            });
        }

        let exp = ts_to_secs(claims.expiration_time.as_ref()).ok_or(VerifyError::MissingClaim("exp"))?;
        if exp + skew < now {
            return Err(VerifyError::Expired { exp, now });
        }
        if let Some(iat) = ts_to_secs(claims.issued_at.as_ref())
            && iat - skew > now
        {
            return Err(VerifyError::NotYetValid { iat, now });
        }

        let sub = claims
            .subject
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or(VerifyError::MissingClaim("sub"))?;

        let mut campaign_id: Option<String> = None;
        let mut scope: Option<String> = None;
        let mut cnf_node_id: Option<String> = None;
        for (label, value) in &claims.rest {
            match label {
                coset::cwt::ClaimName::Assigned(a) if a.to_i64() == CLAIM_SCOPE => {
                    scope = match value {
                        ciborium::Value::Text(s) => Some(s.clone()),
                        _ => None,
                    };
                }
                coset::cwt::ClaimName::Assigned(a) if a.to_i64() == CLAIM_CNF => {
                    cnf_node_id = extract_cnf_node_id(value);
                }
                coset::cwt::ClaimName::Text(s) if s == CLAIM_CAMPAIGN_ID => {
                    campaign_id = match value {
                        ciborium::Value::Text(s) => Some(s.clone()),
                        _ => None,
                    };
                }
                _ => {}
            }
        }

        let campaign_id =
            campaign_id.ok_or(VerifyError::MissingClaim(CLAIM_CAMPAIGN_ID))?;
        if campaign_id.is_empty() {
            return Err(VerifyError::MissingClaim(CLAIM_CAMPAIGN_ID));
        }
        let scope = scope.ok_or(VerifyError::MissingScope(SCOPE_CATALOG_WRITE))?;
        if !scope.split(' ').any(|s| s == SCOPE_CATALOG_WRITE) {
            return Err(VerifyError::MissingScope(SCOPE_CATALOG_WRITE));
        }

        if let (Some(token_node_id), Some(connection_node_id)) = (cnf_node_id, bound_node_id) {
            if token_node_id != connection_node_id {
                return Err(VerifyError::NodeIdMismatch {
                    token: token_node_id,
                    connection: connection_node_id.to_string(),
                });
            }
        }

        let cti = claims
            .cwt_id
            .clone()
            .map(|b| hex::encode(b))
            .unwrap_or_default();

        Ok(VerifiedClaims {
            creator_id: sub,
            campaign_id,
            raw_cwt: Bytes::copy_from_slice(cwt),
            cti,
            exp,
            issuer: self.issuer.clone(),
        })
    }
}

fn parse_sign1(bytes: &[u8]) -> Result<CoseSign1, VerifyError> {
    if let Ok(s) = CoseSign1::from_slice(bytes) {
        return Ok(s);
    }
    // Try the CWT tag (61) wrapping a tagged COSE_Sign1.
    if let Ok(ciborium::Value::Tag(_, inner)) = ciborium::de::from_reader::<ciborium::Value, _>(bytes)
        && let Ok(s) = CoseSign1::from_cbor_value(*inner)
    {
        return Ok(s);
    }
    Err(VerifyError::Parse("not a COSE_Sign1".into()))
}

fn verify_signature(sign1: &CoseSign1, key: &VerifyingKey) -> Result<(), VerifyError> {
    sign1
        .verify_signature(&[], |sig, tbs| -> Result<(), VerifyError> {
            let signature = Signature::from_slice(sig).map_err(|_| VerifyError::BadSignature)?;
            key.verify(tbs, &signature)
                .map_err(|_| VerifyError::BadSignature)
        })
}

fn decode_claims(payload: &[u8]) -> Result<ClaimsSet, coset::CoseError> {
    let value: ciborium::Value =
        ciborium::de::from_reader(payload).map_err(|e| coset::CoseError::DecodeFailed(
            coset::cbor::de::Error::Semantic(None, e.to_string()),
        ))?;
    ClaimsSet::from_cbor_value(value)
}

fn ts_to_secs(t: Option<&Timestamp>) -> Option<i64> {
    match t? {
        Timestamp::WholeSeconds(s) => Some(*s),
        Timestamp::FractionalSeconds(f) => Some(*f as i64),
    }
}

fn extract_cnf_node_id(v: &ciborium::Value) -> Option<String> {
    let map = match v {
        ciborium::Value::Map(m) => m,
        _ => return None,
    };
    for (k, val) in map {
        if let ciborium::Value::Text(s) = k
            && s == CNF_IROH_NODE_ID
            && let ciborium::Value::Text(node_id) = val
        {
            return Some(node_id.clone());
        }
    }
    None
}

fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
