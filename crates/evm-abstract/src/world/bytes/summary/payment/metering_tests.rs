use super::*;
use crate::{
    U256,
    domain::{FiniteSetAlgebra, ProductAlgebra, ValueAlgebra, provenance::Origin},
};
use std::{collections::BTreeMap, sync::Arc};

mod caller_tests;
mod carrier;
use carrier::{FEE, Hook, Policy, RECEIPT, Receipt, Stage, Word};

fn controls<N: WordElement>() -> Vec<EvmByteArraySummary<N>> {
    vec![
        EvmByteArraySummary {
            length: EvmValue::constant(U256::ZERO),
            memory: true,
            content: Content::Zero,
            words: BTreeMap::new(),
        },
        EvmByteArraySummary {
            length: EvmValue::constant(U256::from(3)),
            memory: false,
            content: Content::Exact(Arc::from([1, 0, 2])),
            words: BTreeMap::new(),
        },
        EvmByteArraySummary {
            length: EvmValue::constant(U256::ONE),
            memory: false,
            content: Content::Cells(BTreeMap::from([(0, EvmValue::constant(U256::ZERO))])),
            words: BTreeMap::from([(0, EvmValue::constant(U256::from(9)))]),
        },
        EvmByteArraySummary {
            length: EvmValue::unsigned_range(U256::ONE, U256::from(4)).unwrap(),
            memory: true,
            content: Content::Cells(BTreeMap::from([
                (0, EvmValue::unknown_byte().with_origin(Origin::Memory)),
                (2, EvmValue::constant(U256::from(7))),
            ])),
            words: BTreeMap::from([(32, EvmValue::top())]),
        },
    ]
}

fn check_pure_equivalence<D: EvmNumericAlgebra>(domain: ValueAlgebra<D>) {
    let controls = controls::<D::Element>();
    for left in &controls {
        let expected = left.forget_loop_content();
        let mut budget = WorkBudget::new(usize::MAX);
        assert_eq!(
            left.forget_loop_content_with_payment(domain.borrowed(), &mut budget)
                .unwrap(),
            expected
        );
        for right in &controls {
            let expected = left.project_to_layout(right, domain.borrowed());
            let mut budget = WorkBudget::new(usize::MAX);
            assert_eq!(
                left.project_to_layout_with_payment(right, domain.borrowed(), &mut budget)
                    .unwrap(),
                expected
            );
            for widen in [false, true] {
                let expected = left.join(right, domain.borrowed(), widen);
                let mut budget = WorkBudget::new(usize::MAX);
                assert_eq!(
                    left.join_with_payment(right, domain.borrowed(), widen, &mut budget)
                        .unwrap(),
                    expected
                );
            }
        }
    }
}

#[test]
fn product_and_native_finite_metering_preserve_pure_values_and_all_content_shapes() {
    check_pure_equivalence(ValueAlgebra::new(ProductAlgebra::default()));
    check_pure_equivalence(ValueAlgebra::new(FiniteSetAlgebra::default()));
}

fn private_summary(upper: u64) -> EvmByteArraySummary<Word> {
    let private = || {
        EvmValue::from_numeric(Word::window(
            U256::ZERO,
            U256::from(upper),
            Some(U256::from(7)),
        ))
    };
    EvmByteArraySummary {
        length: EvmValue::constant(U256::from(upper)),
        memory: true,
        content: Content::Cells(BTreeMap::from([(
            0,
            private().with_origin(Origin::Storage).with_identity(41, 3),
        )])),
        words: BTreeMap::from([(0, private()), (32, private())]),
    }
}

fn reset() {
    RECEIPT.set(Receipt::default());
}

fn hooks(hook: Hook) -> usize {
    RECEIPT.get().hooks[hook as usize]
}

#[test]
fn affordable_native_private_predicates_survive_projection_join_and_widen() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let left = private_summary(32);
    let mut right = private_summary(64);
    right.memory = false;
    right.words.remove(&32);
    let expected = left.project_to_layout(&right, domain.borrowed());
    reset();
    let mut budget = WorkBudget::new(100_000);
    let projected = left
        .project_to_layout_with_payment(&right, domain.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(projected, expected);
    let Content::Cells(cells) = &projected.content else {
        panic!("cells")
    };
    let Content::Cells(original) = &left.content else {
        panic!("cells")
    };
    assert!(cells[&0].same_identity(&original[&0]));
    assert!(!cells[&0].contains(U256::from(7)));
    assert!(cells[&0].numeric().observation().contains(U256::from(7)));
    assert_eq!(projected.words.len(), 1);
    for widen in [false, true] {
        let expected = left.join(&right, domain.borrowed(), widen);
        reset();
        let mut budget = WorkBudget::new(100_000);
        let joined = left
            .join_with_payment(&right, domain.borrowed(), widen, &mut budget)
            .unwrap();
        let receipt = RECEIPT.get();
        assert!(receipt.quotes[Hook::Equal as usize] > 0);
        assert!(receipt.quotes[Hook::IsTop as usize] > 0);
        assert_eq!(receipt.hooks[Hook::Widen as usize] > 0, widen);
        assert_eq!(joined, expected);
        let Content::Cells(cells) = &joined.content else {
            panic!("cells")
        };
        for value in [&cells[&0], &joined.words[&0]] {
            assert!(!value.contains(U256::from(7)));
            assert!(value.contains(U256::from(8)));
            assert!(value.numeric().observation().contains(U256::from(7)));
        }
        assert!(!joined.memory);
        assert!(joined.words.contains_key(&0) && !joined.words.contains_key(&32));
    }
}

#[test]
fn unknown_byte_recognition_ignores_metadata_and_zero_canonicalization_is_preserved() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let summary = &controls::<Word>()[3];
    let expected = summary.project_to_layout(summary, domain.borrowed());
    reset();
    let mut budget = WorkBudget::new(100_000);
    let projected = summary
        .project_to_layout_with_payment(summary, domain.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(projected, expected);
    let Content::Cells(cells) = &projected.content else {
        panic!("cells")
    };
    assert!(!cells.contains_key(&0));
    let zero = &controls::<Word>()[2];
    let mut budget = WorkBudget::new(100_000);
    let projected = zero
        .project_to_layout_with_payment(zero, domain.borrowed(), &mut budget)
        .unwrap();
    assert!(matches!(projected.content, Content::Zero));
}

#[test]
fn finite_and_max_refusals_precede_each_selected_native_hook() {
    let _scope = RECEIPT.scope();
    for price in [1_000_000, usize::MAX] {
        for (kind, stage) in [
            (Hook::Equal, Stage::Input),
            (Hook::Join, Stage::Copied),
            (Hook::Widen, Stage::Joined),
            (Hook::Project, Stage::Joined),
            (Hook::Canonical, Stage::Projected),
            (Hook::IsTop, Stage::Canonical),
        ] {
            let domain = ValueAlgebra::new(Policy {
                refusal: Some((kind, stage)),
                refusal_fee: price,
                ..Policy::default()
            });
            let left = private_summary(32);
            let right = private_summary(64);
            reset();
            let mut budget = WorkBudget::new(50_000);
            let widen = kind == Hook::Widen;
            let other = if kind == Hook::Equal { &left } else { &right };
            assert_eq!(
                left.join_with_payment(other, domain.borrowed(), widen, &mut budget),
                Err(CloneError::Work),
                "{kind:?}/{price}"
            );
            assert_eq!(hooks(kind), 0, "refused {kind:?} must not execute");
            assert!(RECEIPT.get().quotes[kind as usize] > 0);
        }
    }
}

#[test]
fn constructor_and_numeric_sentinel_equality_refusals_precede_their_hooks() {
    let _scope = RECEIPT.scope();
    for price in [1_000_000, usize::MAX] {
        for (kind, stage, control) in [
            (Hook::Literal, Stage::Literal, 1),
            (Hook::UnknownByte, Stage::Constructed, 2),
            (Hook::Equal, Stage::Copied, 2),
            (Hook::Range, Stage::Constructed, 2),
        ] {
            let domain = ValueAlgebra::new(Policy {
                refusal: Some((kind, stage)),
                refusal_fee: price,
                ..Policy::default()
            });
            let controls = controls::<Word>();
            let source = &controls[control];
            reset();
            let mut budget = WorkBudget::new(50_000);
            let result = if kind == Hook::Range {
                source.forget_loop_content_with_payment(domain.borrowed(), &mut budget)
            } else {
                source.project_to_layout_with_payment(source, domain.borrowed(), &mut budget)
            };
            assert_eq!(result, Err(CloneError::Work), "{kind:?}/{price}");
            assert_eq!(hooks(kind), 0);
            assert!(RECEIPT.get().quotes[kind as usize] > 0);
        }
    }
}

#[test]
fn a_join_result_with_a_different_copy_tariff_refuses_before_projection() {
    let _scope = RECEIPT.scope();
    for price in [1_000_000, usize::MAX] {
        let domain = ValueAlgebra::new(Policy {
            joined_copy_fee: price,
            ..Policy::default()
        });
        let left = private_summary(32);
        let right = private_summary(64);
        for widen in [false, true] {
            reset();
            let mut budget = WorkBudget::new(50_000);
            assert_eq!(
                left.join_with_payment(&right, domain.borrowed(), widen, &mut budget),
                Err(CloneError::Work)
            );
            assert_eq!(hooks(Hook::Join), 1);
            assert_eq!(hooks(Hook::Widen), usize::from(widen));
            assert_eq!(hooks(Hook::Project), 0);
        }
    }
}

#[test]
fn a_copied_zero_cell_needs_its_own_new_query_tariff() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for price in [1_000_000, usize::MAX] {
        let mut source = controls::<Word>().remove(2);
        let Content::Cells(cells) = &mut source.content else {
            panic!("cells")
        };
        let mut word = Word::window(U256::ZERO, U256::ZERO, None);
        word.query_fee = FEE;
        word.copied_query_fee = price;
        cells.insert(0, EvmValue::from_numeric(word));
        reset();
        let mut budget = WorkBudget::new(50_000);
        assert_eq!(
            source.project_to_layout_with_payment(&source, domain.borrowed(), &mut budget),
            Err(CloneError::Work)
        );
        assert_eq!(hooks(Hook::Copy), 1);
        assert_eq!(
            hooks(Hook::Singleton),
            1,
            "only the original length query runs"
        );
        assert_eq!(RECEIPT.get().quotes[Hook::Singleton as usize], 2);
    }
}

#[test]
fn exact_fee_succeeds_and_one_less_cannot_publish_a_projected_or_joined_result() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let left = private_summary(32);
    let right = private_summary(64);
    for project in [false, true] {
        let run = |budget: &mut WorkBudget| {
            if project {
                left.project_to_layout_with_payment(&right, domain.borrowed(), budget)
            } else {
                left.join_with_payment(&right, domain.borrowed(), true, budget)
            }
        };
        let mut measure = WorkBudget::new(100_000);
        let expected = run(&mut measure).unwrap();
        let fee = measure.used();
        let mut exact = WorkBudget::new(fee);
        assert_eq!(run(&mut exact).unwrap(), expected);
        let mut short = WorkBudget::new(fee - 1);
        assert_eq!(run(&mut short), Err(CloneError::Work));
    }
}

#[test]
fn both_cancellation_owners_stop_all_summary_methods_before_native_hooks() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for captured_owner in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let source = private_summary(32);
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(100_000));
        if captured_owner {
            captured.cancel();
        } else {
            current.cancel();
        }
        reset();
        analysis_control::with_cancellation(&current, || {
            assert_eq!(
                source.project_to_layout_with_payment(&source, domain.borrowed(), &mut budget),
                Err(CloneError::Cancelled)
            );
            assert_eq!(
                source.join_with_payment(&source, domain.borrowed(), true, &mut budget),
                Err(CloneError::Cancelled)
            );
            assert_eq!(
                source.forget_loop_content_with_payment(domain.borrowed(), &mut budget),
                Err(CloneError::Cancelled)
            );
        });
        assert_eq!(RECEIPT.get(), Receipt::default());
        assert_eq!(budget.used(), 0);
    }
}
