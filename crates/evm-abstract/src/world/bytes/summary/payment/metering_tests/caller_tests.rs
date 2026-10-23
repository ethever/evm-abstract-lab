use super::*;
use crate::{
    domain::{
        DomainSpec,
        relational::RelationLimits,
        symbolic::{ByteStateKind, ExprKind},
    },
    resource::SummaryError,
    world::bytes::{
        COPY_FACT_LIMIT,
        summary::{instantiate_scalar, instantiate_scalar_with_payment, scalar},
    },
};
use std::num::NonZeroUsize;

fn leaf(upper: u64, excluded: Option<U256>) -> EvmByteArraySummary<Word> {
    EvmByteArraySummary {
        length: EvmValue::constant(U256::from(upper)),
        memory: false,
        content: Content::Cells(BTreeMap::from([(
            0,
            EvmValue::from_numeric(Word::window(U256::ZERO, U256::from(31), excluded))
                .with_origin(Origin::Storage)
                .with_identity(17, 2),
        )])),
        words: BTreeMap::new(),
    }
}

#[test]
fn paid_summary_equality_matches_derived_equality_and_refuses_native_eq() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let controls = controls::<Word>();
    for left in &controls {
        for right in &controls {
            let expected = left == right;
            let mut budget = WorkBudget::new(100_000);
            assert_eq!(
                left.eq_with_payment(right, domain.borrowed(), &mut budget)
                    .unwrap(),
                expected
            );
        }
    }
    for price in [1_000_000, usize::MAX] {
        let domain = ValueAlgebra::new(Policy {
            refusal: Some((Hook::Equal, Stage::Literal)),
            refusal_fee: price,
            ..Policy::default()
        });
        reset();
        let mut budget = WorkBudget::new(100_000);
        assert_eq!(
            controls[0].eq_with_payment(&controls[0], domain.borrowed(), &mut budget),
            Err(CloneError::Work)
        );
        assert_eq!(hooks(Hook::Equal), 0);
    }
}

#[test]
fn independent_scalar_replay_preserves_native_constraints_and_wrapper_metadata() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for value in [
        EvmValue::constant(U256::from(9)),
        EvmValue::top(),
        EvmValue::from_numeric(Word::window(
            U256::ZERO,
            U256::from(31),
            Some(U256::from(7)),
        ))
        .with_origin(Origin::Storage)
        .with_identity(17, 2),
    ] {
        let expected = instantiate_scalar(&value, domain.borrowed()).unwrap();
        let mut budget = WorkBudget::new(100_000);
        let actual =
            instantiate_scalar_with_payment(&value, domain.borrowed(), &mut budget).unwrap();
        assert_eq!(actual.numeric(), expected.numeric());
        assert_eq!(actual.provenance(), expected.provenance());
        assert_eq!(actual.identity(), expected.identity());
        assert_eq!(
            std::mem::discriminant(actual.expression().unwrap().kind()),
            std::mem::discriminant(expected.expression().unwrap().kind())
        );
        if value.numeric().excluded.is_some() {
            assert!(!actual.contains(U256::from(7)));
            assert!(actual.numeric().observation().contains(U256::from(7)));
            assert_ne!(
                actual.expression(),
                expected.expression(),
                "fresh source per replay"
            );
        }
    }
}

#[test]
fn private_byte_replay_keeps_sidecars_and_independent_sources() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let summary = leaf(32, Some(U256::from(7)));
    let expected = summary.instantiate(domain.borrowed()).unwrap();
    let mut budget = WorkBudget::new(1_000_000);
    let actual = summary
        .instantiate_with_payment(domain.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(actual.root.nodes(), expected.root.nodes());
    assert_eq!(actual.root.depth(), expected.root.depth());
    assert_eq!(actual.length.numeric(), expected.length.numeric());
    assert_eq!(actual.bytes, expected.bytes);
    assert_eq!(actual.default, expected.default);
    assert_eq!(actual.operands.len(), expected.operands.len());
    assert!(!actual.bytes[&0].contains(U256::from(7)));
    assert!(
        actual.bytes[&0]
            .numeric()
            .observation()
            .contains(U256::from(7))
    );
    let (
        ByteStateKind::Input {
            source: left,
            bindings: actual_bindings,
            ..
        },
        ByteStateKind::Input {
            source: right,
            bindings: expected_bindings,
            ..
        },
    ) = (actual.root.kind(), expected.root.kind())
    else {
        panic!("input sources")
    };
    assert_ne!(left, right);
    assert_eq!(actual_bindings[0].numeric, expected_bindings[0].numeric);
    assert!(matches!(
        actual_bindings[0].value.kind(),
        ExprKind::Fresh(_)
    ));
    let retained = actual.operands.get(&actual_bindings[0].value).unwrap();
    assert!(!retained.contains(U256::from(7)));
    assert!(
        retained.provenance().is_top(),
        "binding sidecar strips old wrapper metadata"
    );
}

#[test]
fn replay_prices_compaction_restore_and_observation_import_before_hooks() {
    let _scope = RECEIPT.scope();
    for price in [1_000_000, usize::MAX] {
        for (kind, stage, private) in [
            (Hook::Compact, Stage::Input, true),
            (Hook::Restore, Stage::Copied, true),
            (Hook::FromObservation, Stage::Constructed, false),
        ] {
            let domain = ValueAlgebra::new(Policy {
                refusal: Some((kind, stage)),
                refusal_fee: price,
                ..Policy::default()
            });
            let source = leaf(32, private.then_some(U256::from(7)));
            reset();
            let mut budget = WorkBudget::new(100_000);
            assert_eq!(
                source.instantiate_with_payment(domain.borrowed(), &mut budget),
                Err(SummaryError::Budget(CloneError::Work)),
                "{kind:?}"
            );
            assert_eq!(hooks(kind), 0);
            assert!(RECEIPT.get().quotes[kind as usize] > 0);
        }
    }
}

#[test]
fn compacted_native_result_has_its_own_later_binding_import_tariff() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for price in [1_000_000, usize::MAX] {
        let mut source = leaf(32, Some(U256::from(7)));
        let Content::Cells(cells) = &mut source.content else {
            panic!("cells")
        };
        let mut word = Word::window(U256::ZERO, U256::from(31), Some(U256::from(7)));
        word.compacted_copy_fee = price;
        cells.insert(0, EvmValue::from_numeric(word));
        reset();
        let mut budget = WorkBudget::new(100_000);
        assert_eq!(
            source.instantiate_with_payment(domain.borrowed(), &mut budget),
            Err(SummaryError::Budget(CloneError::Work))
        );
        assert_eq!(hooks(Hook::Compact), 2);
        assert_eq!(hooks(Hook::Import), 0);
        assert_eq!(RECEIPT.get().quotes[Hook::Import as usize], 1);
    }
}

#[test]
fn native_copy_query_constructor_refusals_on_replay_execute_no_denied_hook() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for price in [1_000_000, usize::MAX] {
        for copy in [false, true] {
            let mut source = leaf(32, Some(U256::from(7)));
            let mut length = Word::constant(U256::from(32));
            if copy {
                length.copy_fee = price;
            } else {
                length.query_fee = price;
            }
            source.length = EvmValue::from_numeric(length);
            reset();
            let mut budget = WorkBudget::new(100_000);
            assert_eq!(
                source.instantiate_with_payment(domain.borrowed(), &mut budget),
                Err(SummaryError::Budget(CloneError::Work))
            );
            assert_eq!(hooks(if copy { Hook::Copy } else { Hook::Singleton }), 0);
        }
        let domain = ValueAlgebra::new(Policy {
            refusal: Some((Hook::UnknownByte, Stage::Constructed)),
            refusal_fee: price,
            ..Policy::default()
        });
        let source = leaf(32, Some(U256::from(7)));
        reset();
        let mut budget = WorkBudget::new(100_000);
        assert_eq!(
            source.instantiate_with_payment(domain.borrowed(), &mut budget),
            Err(SummaryError::Budget(CloneError::Work))
        );
        assert_eq!(hooks(Hook::UnknownByte), 0);
    }
}

#[test]
fn native_product_and_finite_replay_projection_parity_and_actual_root_reads() {
    fn check<D: EvmNumericAlgebra>(domain: ValueAlgebra<D>) {
        for mut summary in controls::<D::Element>() {
            summary.words.clear();
            let expected = summary.instantiate(domain.borrowed()).unwrap();
            let mut budget = WorkBudget::new(usize::MAX);
            let actual = summary
                .instantiate_with_payment(domain.borrowed(), &mut budget)
                .unwrap();
            assert_eq!(actual.root.nodes(), expected.root.nodes());
            assert_eq!(actual.root.depth(), expected.root.depth());
            assert_eq!(actual.length.numeric(), expected.length.numeric());
            assert_eq!(actual.bytes, expected.bytes);
            assert_eq!(actual.default, expected.default);
            let expected = actual.summary_projection(domain.borrowed()).unwrap();
            let actual = actual
                .summary_projection_with_payment(domain.borrowed(), &mut budget)
                .unwrap();
            assert_eq!(actual, expected);
        }
    }
    check(ValueAlgebra::new(ProductAlgebra::default()));
    check(ValueAlgebra::new(FiniteSetAlgebra::default()));
}

#[test]
fn dense_compaction_keeps_the_original_retention_limit_success() {
    let domain = ValueAlgebra::new(ProductAlgebra::from_spec(
        DomainSpec::new(
            NonZeroUsize::new(512).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        )
        .with_relations(RelationLimits {
            max_nodes: 16_384,
            max_depth: 256,
            ..RelationLimits::default()
        }),
    ));
    let dense = (0..=255)
        .map(|value| EvmValue::constant(U256::from(value)))
        .reduce(|left, right| domain.join(&left, &right))
        .unwrap();
    let dense = scalar(&dense, domain.borrowed());
    let summary = EvmByteArraySummary {
        length: EvmValue::constant(U256::from(84)),
        memory: false,
        content: Content::Cells((0..84).map(|index| (index, dense.clone())).collect()),
        words: BTreeMap::new(),
    };
    let expected = summary.instantiate(domain.borrowed()).unwrap();
    let mut budget = WorkBudget::new(usize::MAX);
    let actual = summary
        .instantiate_with_payment(domain.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(actual.root.nodes(), expected.root.nodes());
    assert_eq!(actual.root.nodes(), 1_179);
    assert_eq!(actual.bytes, expected.bytes);
    let tiny = ValueAlgebra::new(ProductAlgebra::from_spec(domain.spec().with_relations(
        RelationLimits {
            max_nodes: 1_178,
            ..domain.spec().relations()
        },
    )));
    let expected_error = summary.instantiate(tiny.borrowed()).unwrap_err();
    let mut budget = WorkBudget::new(usize::MAX);
    assert_eq!(
        summary.instantiate_with_payment(tiny.borrowed(), &mut budget),
        Err(SummaryError::Source(expected_error))
    );
}

#[test]
fn whole_word_replay_retains_private_predicate_on_the_normalized_fresh_term() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let mut source = leaf(64, Some(U256::from(7)));
    source.words.insert(
        0,
        EvmValue::from_numeric(Word::window(U256::ZERO, U256::MAX, Some(U256::from(7)))),
    );
    let expected = source.instantiate(domain.borrowed()).unwrap();
    let mut budget = WorkBudget::new(usize::MAX);
    let actual = source
        .instantiate_with_payment(domain.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(actual.root.nodes(), expected.root.nodes());
    let retained = |array: &crate::world::EvmByteArray<Word>| {
        array
            .operands
            .iter()
            .filter(|(expression, _)| matches!(expression.kind(), ExprKind::WordRead { .. }))
            .map(|(_, value)| value.numeric().excluded)
            .collect::<Vec<_>>()
    };
    assert_eq!(retained(&actual), retained(&expected));
    assert_eq!(retained(&actual), [Some(U256::from(7))]);
    assert_ne!(actual.root, expected.root);
}

#[test]
fn projection_uses_the_current_root_even_with_a_conflicting_cached_cell() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let mut source = crate::world::EvmByteArray::<Word>::exact(&[9]);
    source.bytes.insert(0, EvmValue::constant(U256::from(7)));
    let expected = source.summary_projection(domain.borrowed()).unwrap();
    let mut budget = WorkBudget::new(100_000);
    let actual = source
        .summary_projection_with_payment(domain.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(actual, expected);
    assert!(matches!(&actual.content, Content::Exact(bytes) if bytes.as_ref() == [9]));
}

#[test]
fn sparse_projection_prices_only_reachable_positions_and_keeps_the_length_boundary() {
    let domain = ValueAlgebra::new(ProductAlgebra::default());
    let unknown = crate::world::EvmByteArray::unknown_checked().unwrap();
    let mut small = WorkBudget::new(20_000);
    assert_eq!(
        unknown
            .summary_projection_with_payment(domain.borrowed(), &mut small)
            .unwrap(),
        unknown.summary_projection(domain.borrowed()).unwrap()
    );
    for upper in [3, COPY_FACT_LIMIT, COPY_FACT_LIMIT + 1] {
        let summary = EvmByteArraySummary {
            length: EvmValue::unsigned_range(U256::ZERO, U256::from(upper)).unwrap(),
            memory: false,
            content: Content::Cells(BTreeMap::from([
                (0, EvmValue::constant(U256::from(9))),
                (2, EvmValue::constant(U256::from(7))),
            ])),
            words: BTreeMap::new(),
        };
        let mut setup = WorkBudget::new(20_000_000);
        let runtime = summary
            .instantiate_with_payment(domain.borrowed(), &mut setup)
            .unwrap();
        let original = runtime.clone();
        let expected = runtime.summary_projection(domain.borrowed()).unwrap();
        let mut measured = WorkBudget::new(20_000_000);
        let actual = runtime
            .summary_projection_with_payment(domain.borrowed(), &mut measured)
            .unwrap();
        assert_eq!(actual, expected);
        let fee = measured.used();
        let mut exact = WorkBudget::new(fee);
        assert_eq!(
            runtime
                .summary_projection_with_payment(domain.borrowed(), &mut exact)
                .unwrap(),
            expected
        );
        assert_eq!(exact.used(), fee);
        let mut short = WorkBudget::new(fee - 1);
        assert!(matches!(
            runtime.summary_projection_with_payment(domain.borrowed(), &mut short),
            Err(SummaryError::Budget(CloneError::Work))
        ));
        assert_eq!(runtime, original);
    }
}

#[test]
fn a_new_length_copy_is_repriced_before_symbolic_expression_discovery() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for price in [1_000_000, usize::MAX] {
        let mut source = leaf(32, Some(U256::from(7)));
        let mut length = Word::constant(U256::from(32));
        length.copied_query_fee = price;
        source.length = EvmValue::from_numeric(length);
        reset();
        let mut budget = WorkBudget::new(100_000);
        assert_eq!(
            source.instantiate_with_payment(domain.borrowed(), &mut budget),
            Err(SummaryError::Budget(CloneError::Work))
        );
        assert_eq!(hooks(Hook::Singleton), 1);
        assert_eq!(RECEIPT.get().quotes[Hook::Singleton as usize], 2);
    }
}

#[test]
fn captured_and_current_cancellation_stop_all_caller_primitives_before_native_hooks() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for cancel_captured in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let source = leaf(32, Some(U256::from(7)));
        let runtime = crate::world::EvmByteArray::<Word>::exact(&[9]);
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(100_000));
        if cancel_captured {
            captured.cancel();
        } else {
            current.cancel();
        }
        reset();
        analysis_control::with_cancellation(&current, || {
            assert_eq!(
                source.eq_with_payment(&source, domain.borrowed(), &mut budget),
                Err(CloneError::Cancelled)
            );
            assert_eq!(
                source.instantiate_with_payment(domain.borrowed(), &mut budget),
                Err(SummaryError::Budget(CloneError::Cancelled))
            );
            assert_eq!(
                runtime.summary_projection_with_payment(domain.borrowed(), &mut budget),
                Err(SummaryError::Budget(CloneError::Cancelled))
            );
            assert_eq!(
                instantiate_scalar_with_payment(&source.length, domain.borrowed(), &mut budget),
                Err(SummaryError::Budget(CloneError::Cancelled))
            );
        });
        assert_eq!(RECEIPT.get(), Receipt::default());
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn exact_materialization_fee_succeeds_and_one_less_publishes_no_runtime_array() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let source = leaf(32, Some(U256::from(7)));
    let mut measured = WorkBudget::new(usize::MAX);
    source
        .instantiate_with_payment(domain.borrowed(), &mut measured)
        .unwrap();
    let fee = measured.used();
    let mut exact = WorkBudget::new(fee);
    source
        .instantiate_with_payment(domain.borrowed(), &mut exact)
        .unwrap();
    assert_eq!(exact.used(), fee);
    let mut short = WorkBudget::new(fee - 1);
    assert_eq!(
        source.instantiate_with_payment(domain.borrowed(), &mut short),
        Err(SummaryError::Budget(CloneError::Work))
    );
    assert_eq!(source.content, leaf(32, Some(U256::from(7))).content);
}
