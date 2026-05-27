//! CWT (COSE_Sign1) verification against an issuer-published COSE_KeySet.
//!
//! Supported algorithms: ES256 only. Supported COSE_Key shapes:
//! `kty = EC2`, `crv = P-256`. Other shapes are dropped at parse time, not
//! at verify time.
//!
//! Replay protection is intentionally minimal — the verifier records `cti`
//! values in [`VerifiedClaims`] for audit logging but does not maintain a
//! seen-set. Issuers are expected to keep `exp` short and bind tokens via
//! `cnf.iroh_node_id` when single-use semantics matter.

pub mod cose_keys;
pub mod cwt;
pub mod pep_check;

#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_signer;

pub use cose_keys::CoseKeyCache;
pub use cwt::{VerifiedClaims, Verifier, VerifyError};

/// Scope value the catalog write path requires to be present in the CWT's
/// `scope` claim (space-separated, OAuth-style).
pub const SCOPE_CATALOG_WRITE: &str = "catalog.write";
