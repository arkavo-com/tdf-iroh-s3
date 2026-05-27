//! Vendored from arkavo-org/opentdf-rs `examples/pep_check.rs` at tag 0.12.0.
//! Reproduced under the upstream MIT license. See LICENSE-OPENTDF at the
//! repo root for the original notice.
//!
//! Local modifications: removed `fn main` / CLI scaffolding (the `Args`
//! struct, `TokenFormat` enum, `run`/`exit_with` orchestration, the
//! `default_policy` fixture, the std-only `ureq_get` / `fetch_or_read`
//! helpers, and the JWT debugging path — this crate only consumes the
//! CWT path the contract specifies). The remaining items
//! (`Grant`, `verify_cose_sign1`, `parse_authorization_details`,
//! `entitlements_from_grants`, `ec2_to_p256`, `pad_p256_coord`, ...) are
//! made `pub` so `src/auth/cwt.rs` can call them; their signatures and
//! bodies are otherwise byte-for-byte upstream.
//!
//! Per the Arkavo CWT v1 contract, callers MUST NOT roll their own
//! COSE_Sign1 parser. Use the entry points here.

use std::collections::HashMap;

use ciborium::Value as CborValue;
use coset::{CborSerializable, CoseKey, CoseSign1};
use opentdf::pdp::Entitlements;
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};

pub const GRANT_TYPE_ATTRIBUTE: &str = "opentdf_attribute";
pub const AUTHORIZATION_DETAILS_CLAIM: &str = "authorization_details";

#[derive(Debug)]
pub struct Grant {
    pub grant_type: String,
    pub actions: Vec<String>,
    pub locations: Vec<String>,
    pub obligations: Vec<String>,
}

// --- CWT path ---------------------------------------------------------------

pub fn verify_cose_sign1(
    cose1: &CoseSign1,
    cose_key_set_cbor: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    // The platform publishes a CBOR array of COSE_Keys. Parse and try each.
    let value: CborValue = ciborium::de::from_reader(cose_key_set_cbor)
        .map_err(|e| format!("parse COSE Key Set: {e}"))?;
    let arr = match value {
        CborValue::Array(a) => a,
        _ => return Err("COSE Key Set is not a CBOR array".into()),
    };
    for k in arr {
        let bytes = serialize_cbor(&k)?;
        let cose_key = match CoseKey::from_slice(&bytes) {
            Ok(k) => k,
            Err(_) => continue,
        };
        let pubkey = match ec2_to_p256(&cose_key) {
            Some(p) => p,
            None => continue,
        };
        let verified = cose1.verify_signature(&[], |sig, payload| {
            let s = Signature::from_slice(sig).map_err(|e| format!("bad sig bytes: {e}"))?;
            pubkey
                .verify(payload, &s)
                .map_err(|e| format!("verify: {e}"))
        });
        if verified.is_ok() {
            return Ok(());
        }
    }
    Err("no key in the published COSE Key Set verified the CWT signature".into())
}

pub fn serialize_cbor(v: &CborValue) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(v, &mut out)?;
    Ok(out)
}

/// Byte length of an X9.62 P-256 coordinate.
pub const P256_COORD_LEN: usize = 32;

/// Pull X/Y out of an EC2 COSE_Key on P-256, build a SEC1 uncompressed
/// point, and parse as a p256 VerifyingKey.
///
/// COSE_Key coordinates can legitimately arrive shorter than the curve's
/// natural byte length when their leading bytes are zero (CBOR / big-int
/// representations strip these). SEC1 uncompressed points require fixed
/// 32-byte coordinates for P-256, so left-pad each to length before
/// concatenating; reject anything that's too long (a sign the input isn't
/// a P-256 point).
pub fn ec2_to_p256(k: &CoseKey) -> Option<VerifyingKey> {
    let mut x: Option<Vec<u8>> = None;
    let mut y: Option<Vec<u8>> = None;
    for (label, value) in &k.params {
        // EC2 keys: -2 = X, -3 = Y per RFC 9052.
        let label_int = match label {
            coset::Label::Int(i) => *i,
            _ => continue,
        };
        let bytes = match value {
            CborValue::Bytes(b) => b.clone(),
            _ => continue,
        };
        match label_int {
            -2 => x = Some(bytes),
            -3 => y = Some(bytes),
            _ => {}
        }
    }
    let x = pad_p256_coord(&x?)?;
    let y = pad_p256_coord(&y?)?;
    let mut sec1 = Vec::with_capacity(1 + 2 * P256_COORD_LEN);
    sec1.push(0x04);
    sec1.extend_from_slice(&x);
    sec1.extend_from_slice(&y);
    VerifyingKey::from_sec1_bytes(&sec1).ok()
}

/// Left-pad an X9.62 coordinate to [`P256_COORD_LEN`] bytes. Returns
/// `None` if the input is already longer than the curve coordinate
/// length, which would indicate the COSE key is not on P-256.
pub fn pad_p256_coord(b: &[u8]) -> Option<[u8; P256_COORD_LEN]> {
    if b.len() > P256_COORD_LEN {
        return None;
    }
    let mut out = [0u8; P256_COORD_LEN];
    out[P256_COORD_LEN - b.len()..].copy_from_slice(b);
    Some(out)
}

pub fn parse_authorization_details(
    payload: &[u8],
) -> Result<Vec<Grant>, Box<dyn std::error::Error>> {
    let value: CborValue =
        ciborium::de::from_reader(payload).map_err(|e| format!("decode CWT claims: {e}"))?;
    let map = match value {
        CborValue::Map(m) => m,
        _ => return Err("CWT claims is not a CBOR map".into()),
    };
    for (k, v) in map {
        if let CborValue::Text(name) = k
            && name == AUTHORIZATION_DETAILS_CLAIM
        {
            return parse_grants_array(&v);
        }
    }
    Ok(Vec::new())
}

fn parse_grants_array(v: &CborValue) -> Result<Vec<Grant>, Box<dyn std::error::Error>> {
    let arr = match v {
        CborValue::Array(a) => a,
        _ => return Err("authorization_details is not a CBOR array".into()),
    };
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        if let Some(g) = parse_grant(entry)? {
            out.push(g);
        }
    }
    Ok(out)
}

fn parse_grant(v: &CborValue) -> Result<Option<Grant>, Box<dyn std::error::Error>> {
    let m = match v {
        CborValue::Map(m) => m,
        _ => return Ok(None),
    };
    let mut g = Grant {
        grant_type: String::new(),
        actions: Vec::new(),
        locations: Vec::new(),
        obligations: Vec::new(),
    };
    for (k, val) in m {
        let key = match k {
            CborValue::Text(s) => s.as_str(),
            _ => continue,
        };
        match key {
            "type" => {
                if let CborValue::Text(s) = val {
                    g.grant_type = s.clone();
                }
            }
            "actions" => g.actions = cbor_string_array(val),
            "locations" => g.locations = cbor_string_array(val),
            "obligations" => g.obligations = cbor_string_array(val),
            _ => {}
        }
    }
    Ok(Some(g))
}

fn cbor_string_array(v: &CborValue) -> Vec<String> {
    match v {
        CborValue::Array(a) => a
            .iter()
            .filter_map(|e| match e {
                CborValue::Text(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

// --- shared (CWT and JWT both produce a Vec<Grant>) -------------------------

pub fn entitlements_from_grants(grants: &[Grant]) -> Entitlements {
    let mut out: Entitlements = HashMap::new();
    for g in grants {
        if g.grant_type != GRANT_TYPE_ATTRIBUTE {
            continue;
        }
        for loc in &g.locations {
            let entry = out.entry(loc.to_ascii_lowercase()).or_default();
            for a in &g.actions {
                if !entry
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(a))
                {
                    entry.push(a.clone());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cartesian_flatten_dedupes_per_fqn() {
        let grants = vec![
            Grant {
                grant_type: "opentdf_attribute".into(),
                actions: vec!["read".into()],
                locations: vec![
                    "https://example.com/attr/classification/value/secret".into(),
                    "https://example.com/attr/classification/value/public".into(),
                ],
                obligations: vec![],
            },
            // Overlap on secret: read action should not duplicate.
            Grant {
                grant_type: "opentdf_attribute".into(),
                actions: vec!["read".into(), "update".into()],
                locations: vec!["https://example.com/attr/classification/value/secret".into()],
                obligations: vec![],
            },
        ];
        let ents = entitlements_from_grants(&grants);
        let secret = ents
            .get("https://example.com/attr/classification/value/secret")
            .unwrap();
        assert_eq!(secret.len(), 2);
    }

    #[test]
    fn skips_non_attribute_grant_types() {
        let grants = vec![Grant {
            grant_type: "something_else".into(),
            actions: vec!["read".into()],
            locations: vec!["https://example.com/attr/classification/value/secret".into()],
            obligations: vec![],
        }];
        assert!(entitlements_from_grants(&grants).is_empty());
    }

    /// Round-trip: build a CWT payload like the platform would, decode it
    /// with parse_authorization_details, assert the grants come back.
    #[test]
    fn cwt_payload_parser_handles_platform_shape() {
        // CBOR map with text key "authorization_details" → array of one map.
        let mut payload = Vec::new();
        ciborium::ser::into_writer(
            &CborValue::Map(vec![(
                CborValue::Text("authorization_details".into()),
                CborValue::Array(vec![CborValue::Map(vec![
                    (
                        CborValue::Text("type".into()),
                        CborValue::Text("opentdf_attribute".into()),
                    ),
                    (
                        CborValue::Text("actions".into()),
                        CborValue::Array(vec![CborValue::Text("read".into())]),
                    ),
                    (
                        CborValue::Text("locations".into()),
                        CborValue::Array(vec![CborValue::Text(
                            "https://example.com/attr/classification/value/secret".into(),
                        )]),
                    ),
                ])]),
            )]),
            &mut payload,
        )
        .unwrap();
        let grants = parse_authorization_details(&payload).unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].grant_type, "opentdf_attribute");
        assert_eq!(grants[0].actions, vec!["read"]);
    }

    // Regression for the gitar-bot finding on PR #78: ec2_to_p256 used to
    // concatenate X/Y unpadded, so a coordinate with a leading zero byte
    // (legitimate; CBOR / big-int strips it) produced a 64-byte SEC1
    // string instead of 65 and from_sec1_bytes rejected it.
    #[test]
    fn pad_p256_coord_left_pads_short_input() {
        let padded = pad_p256_coord(&[0x42]).unwrap();
        assert_eq!(padded.len(), P256_COORD_LEN);
        assert_eq!(padded[P256_COORD_LEN - 1], 0x42);
        assert!(padded[..P256_COORD_LEN - 1].iter().all(|b| *b == 0));
    }

    #[test]
    fn pad_p256_coord_passes_through_full_length() {
        let input = [0xab; P256_COORD_LEN];
        let padded = pad_p256_coord(&input).unwrap();
        assert_eq!(padded, input);
    }

    #[test]
    fn pad_p256_coord_rejects_over_length() {
        // 33 bytes — would indicate a key that isn't on P-256.
        assert!(pad_p256_coord(&[0u8; P256_COORD_LEN + 1]).is_none());
    }
}
