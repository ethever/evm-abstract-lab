use super::*;

fn value(excluded: Option<U256>) -> EvmValue<Word> {
    EvmValue::from_numeric(Word::window(U256::ZERO, U256::MAX, excluded))
}

fn bytes() -> EvmByteArraySummary<Word> {
    EvmByteArraySummary {
        length: value(None),
        memory: false,
        content: Content::Zero,
        words: BTreeMap::new(),
    }
}

fn covers(left: &EvmByteArraySummary<Word>, right: &EvmByteArraySummary<Word>) -> bool {
    left.covers_with_payment(
        right,
        ValueAlgebra::new(Policy::default()).borrowed(),
        &mut WorkBudget::new(1_000_000),
    )
    .unwrap()
}

#[test]
fn summary_coverage_byte_length_memory_and_missing_word_do_not_erase_constraints() {
    let _scope = RECEIPT.scope();
    let mut left = bytes();
    let mut right = bytes();
    left.length = value(Some(U256::from(7)));
    assert!(!covers(&left, &right));
    assert!(covers(&right, &left));
    right.length = value(Some(U256::from(7)));
    left.memory = true;
    assert!(!covers(&left, &right));
    right.memory = true;
    assert!(covers(&left, &right));
    left.words.insert(0, value(Some(U256::from(7))));
    assert!(!covers(&left, &right), "omitted word is unconstrained");
    right.words.insert(32, value(Some(U256::from(7))));
    assert!(!covers(&left, &right), "different word coordinate");
    right.words.insert(0, value(Some(U256::from(7))));
    assert!(covers(&left, &right));
    right.words.insert(0, value(None));
    assert!(
        !covers(&left, &right),
        "top-looking private word cannot cover Top"
    );
}

#[test]
fn summary_coverage_byte_content_requires_actual_equality_including_sparse_default() {
    let _scope = RECEIPT.scope();
    let mut left = bytes();
    let mut right = bytes();
    left.content = Content::Cells(BTreeMap::from([(0, value(Some(U256::from(7))))]));
    right.content = Content::Cells(BTreeMap::from([(0, value(Some(U256::from(7))))]));
    assert!(covers(&left, &right));
    right.content = Content::Cells(BTreeMap::new());
    assert!(!covers(&left, &right));
    assert!(
        !covers(&right, &left),
        "different content is intentionally unsupported"
    );
    right.content = Content::Cells(BTreeMap::from([(0, value(None))]));
    assert!(!covers(&left, &right));
    left.content = Content::Exact(Arc::from([1, 2, 3]));
    right.content = Content::Exact(Arc::from([1, 2, 3]));
    assert!(covers(&left, &right));
    right.content = Content::Exact(Arc::from([1, 2, 4]));
    assert!(!covers(&left, &right));
    right.content = Content::Zero;
    assert!(!covers(&left, &right));
}

#[test]
fn summary_coverage_byte_proof_runs_only_paid_top_equal_and_refuses_without_poisoning() {
    let _scope = RECEIPT.scope();
    let mut left = bytes();
    let mut right = bytes();
    left.content = Content::Cells(BTreeMap::from([(0, value(Some(U256::from(7))))]));
    right.content = Content::Cells(BTreeMap::from([(0, value(Some(U256::from(7))))]));
    RECEIPT.set(Receipt::default());
    assert!(covers(&left, &right));
    let calls = RECEIPT.get();
    assert!(calls.hooks[Hook::Equal as usize] > 0 && calls.hooks[Hook::IsTop as usize] > 0);
    for hook in [
        Hook::Copy,
        Hook::Observation,
        Hook::FromObservation,
        Hook::Join,
        Hook::Widen,
        Hook::Project,
        Hook::Canonical,
    ] {
        assert_eq!(calls.hooks[hook as usize], 0);
    }
    for refused in [Hook::IsTop, Hook::Equal] {
        let domain = ValueAlgebra::new(Policy {
            refusal: Some((refused, Stage::Input)),
            ..Policy::default()
        });
        let mut budget = WorkBudget::new(1_000_000);
        let result = left
            .covers_with_payment(&right, domain.borrowed(), &mut budget)
            .unwrap();
        assert_eq!(
            result,
            refused == Hook::IsTop,
            "unavailable Top can still use paid equality"
        );
        assert!(!budget.exhausted());
    }
    let domain = ValueAlgebra::new(Policy::default());
    let mut full = WorkBudget::new(1_000_000);
    assert!(
        left.covers_with_payment(&right, domain.borrowed(), &mut full)
            .unwrap()
    );
    for limit in [0, full.used() - 1, full.used()] {
        let mut budget = WorkBudget::new(limit);
        assert_eq!(
            left.covers_with_payment(&right, domain.borrowed(), &mut budget)
                .unwrap(),
            limit == full.used()
        );
        assert!(!budget.exhausted());
    }
}
