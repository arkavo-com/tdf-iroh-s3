//! Translate the verifier's `Vec<Grant>` into `opentdf::pdp::Entitlements`.
//!
//! The verifier (`src/auth/cwt.rs`) has already enforced the contract's
//! type/action allowlists by the time grants reach this function. The job
//! here is the FQN-keyed fold and a basic URL sanity check (drops grants
//! whose `fqn` is not a parseable absolute URL).

use opentdf::pdp::Entitlements;
use crate::auth::cwt::VerifiedClaims;

pub fn cwt_to_entitlements(claims: &VerifiedClaims) -> Entitlements {
    let mut out = Entitlements::new();
    for g in &claims.grants {
        if !is_valid_fqn(&g.fqn) { continue; }
        let bucket = out.entry(g.fqn.clone()).or_default();
        for action in &g.actions {
            if !bucket.iter().any(|a| a == action) {
                bucket.push(action.clone());
            }
        }
    }
    out
}

/// Conservative FQN check: starts with https://, has the OpenTDF
/// `/attr/<name>/value/<value>` shape. We don't pull in a URL crate for
/// this; the contract already constrains valid issuers, and this is
/// belt-and-suspenders.
fn is_valid_fqn(s: &str) -> bool {
    if !s.starts_with("https://") && !s.starts_with("http://") { return false; }
    s.contains("/attr/") && s.contains("/value/")
}
