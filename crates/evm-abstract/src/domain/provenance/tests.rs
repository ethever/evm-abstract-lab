use super::{Origin, OriginSet, Provenance};

#[test]
fn source_and_role_metadata_do_not_contain_value_identity() {
    let source = Provenance::source(Origin::Storage);
    assert_eq!(source, Provenance::source(Origin::Storage));
    let json = serde_json::to_value(&source).unwrap();
    assert!(
        json.get("identity").is_none()
            && json.get("input").is_none()
            && json.get("symbol").is_none()
    );
    assert_eq!(json["origins"]["Sources"], serde_json::json!(["Storage"]));
}

#[test]
fn arithmetic_metadata_joins_sources_without_asserting_address_role() {
    let source = Provenance::source(Origin::Calldata).with_code_address_role();
    let result = Provenance::transfer(std::slice::from_ref(&source));
    assert!(!result.is_code_address());
    assert_eq!(
        result.origins().sources(),
        OriginSet::from_sources([Origin::Calldata, Origin::Arithmetic])
            .unwrap()
            .sources()
    );
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
fn code_address_role_is_a_must_property_separate_from_origin_labels() {
    let role = Provenance::top().with_code_address_role();
    assert!(role.is_code_address());
    assert!(!role.is_top());
    assert!(role.join(&role).is_code_address());
    assert!(!role.join(&Provenance::top()).is_code_address());
    assert!(!Provenance::transfer(std::slice::from_ref(&role)).is_code_address());
}
