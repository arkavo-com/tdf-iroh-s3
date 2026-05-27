use opentdf::prelude::*;
use tdf_iroh_s3::pdp::policy_extract::manifest_attr_value_fqns;
use tdf_iroh_s3::validation::structure::validate_tdf_structure;

/// Build a TDF in-memory with two attribute-value FQNs in its policy and
/// verify that `manifest_attr_value_fqns` extracts both of them.
#[test]
fn extracts_all_attribute_value_fqns_from_manifest_policy() {
    let policy = PolicyBuilder::new()
        .id_auto()
        .dissemination(["test@example.com"])
        .attribute_fqn("https://example/attr/clearance/value/secret")
        .expect("valid FQN")
        .attribute_fqn("https://example/attr/dept/value/eng")
        .expect("valid FQN")
        .build()
        .expect("build policy");

    let tdf_bytes = Tdf::encrypt(b"test payload")
        .kas_url("https://kas.example.com")
        .policy(policy)
        .to_bytes()
        .expect("encrypt TDF");

    // validate_tdf_structure opens the zip and parses the manifest
    let manifest = validate_tdf_structure(&tdf_bytes).expect("parse manifest");

    let mut fqns = manifest_attr_value_fqns(&manifest).expect("extract FQNs");
    fqns.sort();

    assert_eq!(
        fqns,
        vec![
            "https://example/attr/clearance/value/secret".to_string(),
            "https://example/attr/dept/value/eng".to_string(),
        ]
    );
}

/// A manifest with no attributes yields an empty list (no panic).
#[test]
fn empty_policy_yields_empty_fqns() {
    let policy = PolicyBuilder::new()
        .id_auto()
        .dissemination(["test@example.com"])
        .build()
        .expect("build policy");

    let tdf_bytes = Tdf::encrypt(b"test payload")
        .kas_url("https://kas.example.com")
        .policy(policy)
        .to_bytes()
        .expect("encrypt TDF");

    let manifest = validate_tdf_structure(&tdf_bytes).expect("parse manifest");
    let fqns = manifest_attr_value_fqns(&manifest).expect("extract FQNs");
    assert!(fqns.is_empty(), "expected no FQNs, got: {:?}", fqns);
}

/// Conditions nested inside AND/OR/NOT boolean nodes are still collected.
#[test]
fn extracts_fqns_from_nested_boolean_policy() {
    use opentdf::{AttributeCondition, AttributeIdentifier, AttributePolicy, AttributeValue, Operator};

    let cond_a = AttributePolicy::Condition(AttributeCondition {
        attribute: AttributeIdentifier::new("example.com", "alpha"),
        operator: Operator::Equals,
        value: Some(AttributeValue::String("v1".to_string())),
    });
    let cond_b = AttributePolicy::Condition(AttributeCondition {
        attribute: AttributeIdentifier::new("example.com", "beta"),
        operator: Operator::Equals,
        value: Some(AttributeValue::String("v2".to_string())),
    });
    let nested = AttributePolicy::and(vec![cond_a, cond_b]);

    let policy = opentdf::Policy::new(
        "a1b2c3d4-e5f6-7890-abcd-ef1234567890".to_string(),
        vec![nested],
        vec!["test@example.com".to_string()],
    );

    let tdf_bytes = Tdf::encrypt(b"test payload")
        .kas_url("https://kas.example.com")
        .policy(policy)
        .to_bytes()
        .expect("encrypt TDF");

    let manifest = validate_tdf_structure(&tdf_bytes).expect("parse manifest");
    let mut fqns = manifest_attr_value_fqns(&manifest).expect("extract FQNs");
    fqns.sort();

    assert_eq!(
        fqns,
        vec![
            "https://example.com/attr/alpha/value/v1".to_string(),
            "https://example.com/attr/beta/value/v2".to_string(),
        ]
    );
}

/// Present-operator conditions are skipped (assert existence without a specific
/// value); only concrete-value conditions contribute to the extracted FQNs.
#[test]
fn present_operator_conditions_are_skipped() {
    use opentdf::{AttributeCondition, AttributeIdentifier, AttributePolicy, AttributeValue, Operator};

    // Build a manifest with both a concrete-value condition AND a Present-operator condition.
    // Only the concrete-value FQN should be extracted.
    let cond_concrete = AttributePolicy::Condition(AttributeCondition {
        attribute: AttributeIdentifier::new("example.com", "clearance"),
        operator: Operator::Equals,
        value: Some(AttributeValue::String("secret".to_string())),
    });
    let cond_present = AttributePolicy::Condition(AttributeCondition {
        attribute: AttributeIdentifier::new("example.com", "department"),
        operator: Operator::Present,
        value: None,
    });
    let nested = AttributePolicy::and(vec![cond_concrete, cond_present]);

    let policy = opentdf::Policy::new(
        "a1b2c3d4-e5f6-7890-abcd-ef1234567890".to_string(),
        vec![nested],
        vec!["test@example.com".to_string()],
    );

    let tdf_bytes = Tdf::encrypt(b"test payload")
        .kas_url("https://kas.example.com")
        .policy(policy)
        .to_bytes()
        .expect("encrypt TDF");

    let manifest = validate_tdf_structure(&tdf_bytes).expect("parse manifest");
    let fqns = manifest_attr_value_fqns(&manifest).expect("extract FQNs");

    // Only the concrete-value FQN should be present; Present operator is skipped.
    assert_eq!(
        fqns,
        vec!["https://example.com/attr/clearance/value/secret".to_string()]
    );
}
