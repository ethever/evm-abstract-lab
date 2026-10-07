use super::{InputIdentity, RuntimeIdentity, Symbol, ValueIdentity};

#[test]
fn temporary_definition_identity_does_not_change_persistent_equality() {
    let first = ValueIdentity::none().with_runtime(RuntimeIdentity::new(7, 1));
    let second = ValueIdentity::none().with_runtime(RuntimeIdentity::new(7, 2));
    assert_eq!(first, second);
    assert!(first.same_identity(&first));
    assert!(!first.same_identity(&second));
    assert!(!ValueIdentity::none().same_identity(&ValueIdentity::none()));
    assert!(!first.join(&first).same_identity(&first));
    assert_eq!(serde_json::to_value(first).unwrap(), serde_json::json!({}));
}

#[test]
fn immutable_inputs_are_scoped_must_facts_kept_only_when_both_paths_agree() {
    let first = ValueIdentity::none().with_input(InputIdentity::new(1, Symbol::Caller));
    let cloned = first;
    let independent = ValueIdentity::none().with_input(InputIdentity::new(2, Symbol::Caller));
    let different = ValueIdentity::none().with_input(InputIdentity::new(1, Symbol::Origin));
    assert!(first.same_identity(&cloned));
    assert!(!first.same_identity(&independent));
    assert!(!first.same_identity(&different));
    assert_ne!(first, independent);
    assert_eq!(first.join(&cloned), first);
    assert_eq!(first.join(&independent), ValueIdentity::none());
    let encoded = serde_json::to_value(first).unwrap();
    assert_eq!(encoded["input"]["name"], "Caller");
    assert!(encoded["input"].get("scope").is_none());
    assert_eq!(first.input().unwrap().scope(), 1);
    assert_eq!(first.input().unwrap().symbol(), Symbol::Caller);
}

#[test]
fn clearing_runtime_preserves_immutable_input_identity() {
    let base = ValueIdentity::none().with_input(InputIdentity::new(1, Symbol::CallValue));
    let mut decorated = base.with_runtime(RuntimeIdentity::new(9, 2));
    decorated.forget_runtime();
    assert!(decorated.same_identity(&base));
}
