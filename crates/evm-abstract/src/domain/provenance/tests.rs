use super::{Origin, OriginSet, Provenance, RuntimeIdentity};

#[test]
fn equal_source_labels_never_prove_value_equality() {
    let first = Provenance::source(Origin::Storage).with_identity(RuntimeIdentity::new(7, 1));
    let second = Provenance::source(Origin::Storage).with_identity(RuntimeIdentity::new(7, 2));
    assert_eq!(
        first, second,
        "persistent equality ignores fresh identities"
    );
    assert!(!first.same_identity(&second));
    assert!(first.same_identity(&first.clone()));
    assert!(!Provenance::top().same_identity(&Provenance::top()));
}

#[test]
fn join_and_transfer_forget_identity() {
    let source = Provenance::source(Origin::Calldata).with_identity(RuntimeIdentity::new(2, 8));
    let joined = source.join(&source);
    assert!(!joined.same_identity(&source));
    let result = Provenance::transfer(std::slice::from_ref(&source));
    assert!(!result.same_identity(&source));
    assert_eq!(
        result.origins().sources(),
        OriginSet::from_sources([Origin::Calldata, Origin::Arithmetic])
            .expect("nonempty")
            .sources()
    );
}

#[test]
fn identity_scope_prevents_different_executions_becoming_equal() {
    let first = Provenance::top().with_identity(RuntimeIdentity::new(1, 4));
    let other_execution = Provenance::top().with_identity(RuntimeIdentity::new(2, 4));
    assert!(!first.same_identity(&other_execution));
    let serialized = serde_json::to_value(&first).expect("serialize");
    assert!(serialized.get("identity").is_none());
}

#[test]
fn source_join_is_associative_commutative_and_idempotent() {
    let a = OriginSet::source(Origin::Storage);
    let b = OriginSet::source(Origin::Calldata);
    let c = OriginSet::source(Origin::Arithmetic);
    assert_eq!(a.join(&b), b.join(&a));
    assert_eq!(a.join(&a), a);
    assert_eq!(a.join(&b).join(&c), a.join(&b.join(&c)));
    assert_eq!(a.join(&OriginSet::top()), OriginSet::top());
    assert!(a.meet(&b).is_none());
    assert!(OriginSet::from_sources([]).is_none());
}

#[test]
fn code_address_role_is_a_must_property_and_supplies_no_identity() {
    let role = Provenance::top().with_code_address_role();
    assert!(role.is_code_address());
    assert!(!role.is_top());
    assert!(role.join(&role).is_code_address());
    assert!(!role.join(&Provenance::top()).is_code_address());
    assert!(!Provenance::transfer(std::slice::from_ref(&role)).is_code_address());
    assert!(!role.same_identity(&role));
}
