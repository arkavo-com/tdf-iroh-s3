//! Pull attribute-value FQNs out of a TDF manifest's policy. Used at ingest
//! time to denormalize the resource side of the PDP check onto the event,
//! so reads don't need to round-trip to S3.

use anyhow::{Context, Result};
use opentdf::manifest::TdfManifestExt;
use opentdf::{AttributePolicy, AttributeValue, LogicalOperator, TdfManifest};
use opentdf::fqn::AttributeFqn;

/// Walk the manifest's policy and collect every attribute-value FQN
/// referenced by a leaf condition.
///
/// Each `AttributeCondition` leaf contributes exactly one FQN string of the
/// form `https://<namespace>/attr/<name>/value/<value>`.  Conditions whose
/// value is not a plain string (or is absent, e.g. `Present`/`NotPresent`
/// operators) are skipped because they do not identify a specific attribute
/// value.
///
/// Boolean nodes (`AND`, `OR`, `NOT`) are traversed recursively.
pub fn manifest_attr_value_fqns(manifest: &TdfManifest) -> Result<Vec<String>> {
    let policy = manifest
        .get_policy()
        .context("Failed to decode policy from manifest")?;

    let mut fqns = Vec::new();
    for attr_policy in &policy.body.attributes {
        collect_fqns(attr_policy, &mut fqns);
    }
    Ok(fqns)
}

/// Recursively walk an `AttributePolicy` tree and push FQN strings onto `out`.
fn collect_fqns(policy: &AttributePolicy, out: &mut Vec<String>) {
    match policy {
        AttributePolicy::Condition(cond) => {
            // Only emit a value-FQN when the condition carries a plain string value.
            if let Some(AttributeValue::String(val)) = &cond.value {
                let fqn = AttributeFqn::with_value(
                    &cond.attribute.namespace,
                    &cond.attribute.name,
                    val,
                );
                out.push(fqn.to_url());
            }
            // We only extract a concrete attribute-value FQN. Skip:
            // - Operator::Present / NotPresent: assert existence with no specific value.
            // - Array-valued operators (In/AnyOf/AllOf/MinimumOf/MaximumOf): v1 PDP checks
            //   only against equality-style FQNs; richer matchers come later.
            // Leaving these out keeps the catalog event's attribute_value_fqns vector
            // tight; the PDP will simply not match the affected resource against any
            // reader.
        }
        AttributePolicy::Logical(logical) => match logical {
            LogicalOperator::AND { conditions } | LogicalOperator::OR { conditions } => {
                for child in conditions {
                    collect_fqns(child, out);
                }
            }
            LogicalOperator::NOT { condition } => {
                collect_fqns(condition, out);
            }
        },
    }
}
