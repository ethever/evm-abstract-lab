use super::{memory_condition, sequence_condition};
#[cfg(test)]
use crate::analysis::testing::{Account, Entry, TestValue, World};
use crate::domain::ProductAlgebra;
use crate::domain::ValueAlgebra;
use alloy_primitives::U256;

fn value(value: U256) -> TestValue {
    TestValue::constant(value)
}

#[test]
fn sequence_guard_agrees_with_checked_math_including_empty_and_overflow() {
    let cases = [
        U256::ZERO,
        U256::from(1),
        U256::from(31),
        U256::from(32),
        U256::MAX - U256::from(1),
        U256::MAX,
    ];
    for offset in cases {
        for size in cases {
            for length in cases {
                let expected = offset.checked_add(size).is_some_and(|end| end <= length);
                let actual = sequence_condition(
                    &value(offset),
                    &value(size),
                    &value(length),
                    ValueAlgebra::new(&ProductAlgebra::default()),
                );
                assert_eq!(actual.singleton(), Some(U256::from(u8::from(expected))));
            }
        }
    }
}

#[test]
fn memory_guard_distinguishes_zero_size_and_word_rounded_u256_end() {
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let bound = super::logical::MAX_END;
    for (offset, size, expected) in [
        (U256::MAX, 0, true),
        (U256::ZERO, 1, true),
        (U256::from(31), 2, true),
        (bound - U256::from(1), 1, true),
        (bound - U256::from(1), 2, false),
        (bound, 1, false),
        (bound + U256::from(1), 0, true),
        (U256::MAX, 1, false),
    ] {
        let actual = memory_condition(&value(offset), &value(U256::from(size)), domain);
        assert_eq!(actual.singleton(), Some(U256::from(u8::from(expected))));
    }
    assert_eq!(
        memory_condition(&value(U256::ZERO), &value(bound), domain).singleton(),
        Some(U256::from(1))
    );
}

fn execution() -> crate::analysis::transfer::Execution {
    use crate::Fork;
    use alloy_primitives::Address;
    let address = Address::from([0x11; 20]);
    let mut world = World::new(Fork::Osaka, "range partition unit test");
    world.insert(address, Account::empty()).unwrap();
    crate::analysis::transfer::Execution {
        payload: super::super::initial(
            &world,
            &Entry::new(address),
            ValueAlgebra::new(&ProductAlgebra::default()),
        )
        .unwrap(),
        executed_pcs: Vec::new(),
        instruction_progress: Vec::new(),
        instruction_values: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        completed_calls: Vec::new(),
        retention_failures: Vec::new(),
    }
}

#[test]
fn symbolic_logical_partition_retains_success_and_pre_effect_failure() {
    use crate::{
        analysis::ExecutionConfig,
        domain::{relational::CheckResult, symbolic::ExprId},
    };
    let mut result = execution();
    let before = result.payload.active().memory.clone();
    let config = ExecutionConfig {
        max_memory_bytes: 64,
        ..ExecutionConfig::default()
    };
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let mut budget = super::super::WorkBudget::new(10_000_000);
    let transfer_numeric = ProductAlgebra::from_spec(config_numeric.spec());
    let mut context = crate::analysis::transfer::testing::context(
        &config,
        &mut budget,
        ValueAlgebra::new(&transfer_numeric),
    );
    let source = ExprId::fresh().unwrap();
    let size = TestValue::top().with_expression(source.clone());
    assert!(super::memory(
        &mut result,
        &value(U256::ZERO),
        &size,
        &mut context,
        7
    ));
    assert!(result.frontiers.is_empty());
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(
        result.outcomes[0].kind,
        crate::analysis::OutcomeKind::Failure
    );
    assert_eq!(result.outcomes[0].data.len().singleton(), Some(U256::ZERO));
    assert_eq!(result.payload.active().memory, before);
    for (length, legal) in [(0, true), (32, true), (64, true), (65, true), (4096, true)] {
        let word = U256::from(length);
        assert_eq!(
            super::memory_condition(
                &value(U256::ZERO),
                &value(word),
                ValueAlgebra::new(&ProductAlgebra::default()),
            )
            .singleton(),
            Some(U256::from(u8::from(legal))),
            "host extraction capacity does not exclude logical success"
        );
        // The retained OR/AND guard keeps both partitions. A candidate is not
        // imported into F to search its compound Boolean alternatives.
        assert_eq!(
            result.payload.relations.can_equal(
                &source,
                word,
                &config_numeric.relations,
                context.domain
            ),
            CheckResult::MayBeFeasible
        );
    }
}

#[test]
fn assumption_capacity_failure_preserves_the_whole_unmodified_input() {
    use crate::{
        analysis::ExecutionConfig,
        domain::{relational::CheckResult, symbolic::ExprId},
    };
    let mut result = execution();
    let config = ExecutionConfig {
        max_memory_bytes: 64,
        ..ExecutionConfig::default()
    };
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.max_constraints = 1;
    let algebra = crate::analysis::testing::product(config_numeric.spec());
    let domain = algebra.borrowed();
    let other = ExprId::fresh().unwrap();
    assert_eq!(
        result
            .payload
            .relations
            .assume(&other, true, &config_numeric.relations, domain),
        CheckResult::MayBeFeasible
    );
    let before = result.payload.clone();
    let source = ExprId::fresh().unwrap();
    let size = TestValue::top().with_expression(source);
    let mut budget = super::super::WorkBudget::new(10_000_000);
    let mut context = crate::analysis::transfer::testing::context(&config, &mut budget, domain);
    assert!(!super::memory(
        &mut result,
        &value(U256::ZERO),
        &size,
        &mut context,
        7
    ));
    assert_eq!(result.payload, before);
    assert_eq!(result.frontiers.len(), 1);
    let pending = result.frontiers[0].range.as_ref().unwrap();
    assert_eq!(pending.nonzero, None);
    assert_eq!(
        pending.request.kind,
        crate::analysis::RangeKind::LogicalMemory
    );
    assert_eq!(
        pending.request.limit.singleton(),
        Some(super::logical::MAX_END)
    );
    assert_eq!(*pending.payload, before);
    assert_eq!(
        result.frontiers[0].reason,
        crate::analysis::FrontierReason::Relations(
            crate::domain::relational::QueryReason::ConstraintLimit
        )
    );
}

#[test]
fn encoder_limit_keeps_success_guard_and_gas_failure_coverage() {
    use crate::{
        analysis::ExecutionConfig,
        domain::{
            relational::{Constraint, QueryReason},
            symbolic::{ByteStateId, ByteView, ExprId, SourceId},
        },
    };
    let mut result = execution();
    let before = result.payload.active().memory.clone();
    let config = ExecutionConfig {
        max_memory_bytes: 64,
        ..ExecutionConfig::default()
    };
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.max_nodes = 64;
    let length = ExprId::constant(U256::from(64));
    let hash = ExprId::keccak(
        ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), length.clone()),
            ExprId::constant(U256::ZERO),
            length,
        ),
        config_numeric.relations.expressions(),
    )
    .unwrap();
    let size = TestValue::top().with_expression(hash);
    let condition = super::memory_condition(
        &value(U256::ZERO),
        &size,
        ValueAlgebra::new(&ProductAlgebra::default()),
    );
    let expression = condition.symbolic_expression().unwrap();
    assert!(expression.nodes() <= config_numeric.relations.max_nodes);
    let mut budget = super::super::WorkBudget::new(10_000_000);
    let transfer_numeric = ProductAlgebra::from_spec(config_numeric.spec());
    let mut context = crate::analysis::transfer::testing::context(
        &config,
        &mut budget,
        ValueAlgebra::new(&transfer_numeric),
    );
    assert!(super::memory(
        &mut result,
        &value(U256::ZERO),
        &size,
        &mut context,
        7
    ));
    assert_eq!(result.payload.active().memory, before);
    assert!(result.frontiers.is_empty());
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(
        result.outcomes[0].kind,
        crate::analysis::OutcomeKind::Failure
    );
    assert!(result.payload.relations.constraints().any(|atom| atom
        == &Constraint::Truth {
            expression: expression.clone(),
            nonzero: true,
        }));
    assert_eq!(
        result
            .diagnostics
            .iter()
            .filter(|(_, kind)| *kind
                == crate::analysis::DiagnosticKind::RelationalPrecisionLost(
                    QueryReason::ExpressionLimit
                ))
            .count(),
        2
    );
}

mod literal_memory {
    use super::{TestValue, execution, value};
    use crate::{
        U256,
        analysis::{
            ExecutionConfig, FrontierReason, MachinePayload,
            transfer::{Execution, WorkBudget},
        },
        domain::{
            DomainSpec, ProductAlgebra, ReductionStatus, SingletonProof, ValueAlgebra,
            facts::{UnaryPredicate, WordBounds},
            relational::{Constraint, QueryReason},
            symbolic::ExprId,
        },
    };

    use super::super::{
        LITERAL_MEMORY_BOUNDS_WORK, LITERAL_MEMORY_METADATA_WORK, RangeQueries, allow_memory,
        literal_memory_singleton_work, logical, memory_condition,
    };

    // Construct operands with independent component facts still retained. This
    // does not change any execution policy, complete-analysis fixture or budget.
    fn component_fixture_domain() -> ValueAlgebra<ProductAlgebra> {
        use std::num::NonZeroUsize;
        ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::new(
            NonZeroUsize::new(512).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(2).unwrap(),
        )))
    }

    use crate::analysis::transfer::testing::context;

    fn assert_uninstalled(result: &Execution, before: &MachinePayload) {
        assert_eq!(&result.payload, before);
        assert!(result.outcomes.is_empty());
        assert!(result.successors.is_empty());
        assert!(result.completed_calls.is_empty());
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn literal_admission_and_rejection_have_the_same_fee_at_k8_and_k512() {
        let bound = logical::MAX_END;
        for enabled in [false, true] {
            for capacity in [8, 512] {
                let config = ExecutionConfig::default();
                let config_numeric = crate::analysis::testing::NumericLimits {
                    max_constants: capacity,
                    relations: crate::domain::relational::RelationLimits {
                        enabled,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                assert_eq!(config.max_work, 20_000_000);
                for (offset, size, allowed) in [
                    (U256::ZERO, U256::ONE, true),
                    (U256::ZERO, bound, true),
                    (bound - U256::ONE, U256::ONE, true),
                    (bound - U256::ONE, U256::from(2), false),
                    (bound, U256::ONE, false),
                    (bound + U256::ONE, U256::ONE, false),
                    (U256::MAX - U256::ONE, U256::from(2), false),
                    (U256::MAX, U256::ONE, false),
                    (U256::MAX, U256::ZERO, true),
                ] {
                    let mut result = execution();
                    let before = result.payload.clone();
                    let mut budget = WorkBudget::new(config.max_work);
                    assert_eq!(
                        allow_memory(
                            &mut result,
                            &value(offset),
                            &value(size),
                            &mut context(
                                &config,
                                &mut budget,
                                ValueAlgebra::new(&ProductAlgebra::from_spec(
                                    config_numeric.spec()
                                ))
                            ),
                            7,
                        ),
                        allowed,
                        "offset={offset}, size={size}, K={capacity}, relations={enabled}"
                    );
                    assert_eq!(budget.used(), if size == U256::ZERO { 736 } else { 1_600 });
                    assert!(budget.used() < 31_200);
                    assert!(!budget.exhausted());
                    assert!(result.frontiers.is_empty());
                    assert_uninstalled(&result, &before);
                }
            }
        }
    }

    #[test]
    fn numeric_zero_size_admits_unknown_symbolic_multivalue_and_max_offsets() {
        let config = ExecutionConfig::default();
        let config_numeric = crate::analysis::testing::NumericLimits::default();
        let domain =
            crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
        let domain = domain.borrowed();
        let numeric_zero = TestValue::unsigned_range(U256::ZERO, U256::ZERO).unwrap();
        assert!(numeric_zero.constants().is_none());
        assert_eq!(numeric_zero.singleton(), Some(U256::ZERO));
        let exact_zero = component_fixture_domain()
            .from_facts(&[UnaryPredicate::Congruent(
                crate::domain::congruence::Congruence::exact(U256::ZERO),
            )])
            .unwrap();
        assert!(exact_zero.constants().is_none());
        assert_eq!(exact_zero.singleton(), Some(U256::ZERO));
        assert_eq!(exact_zero.congruence().modulus_residue(), None);
        let modulo_zero = component_fixture_domain()
            .from_facts(&[
                UnaryPredicate::UnsignedBounds(WordBounds::new(U256::ZERO, U256::ZERO).unwrap()),
                UnaryPredicate::congruent(U256::from(3), U256::ZERO).unwrap(),
            ])
            .unwrap();
        assert!(modulo_zero.constants().is_none());
        assert_eq!(modulo_zero.singleton(), Some(U256::ZERO));
        assert_eq!(value(U256::ZERO).congruence().modulus_residue(), None);
        assert!(numeric_zero.congruence().is_top());
        assert_eq!(
            numeric_zero.congruence().modulus_residue(),
            Some((U256::ONE, U256::ZERO))
        );
        assert!(!modulo_zero.congruence().is_top());
        assert_eq!(
            modulo_zero.congruence().modulus_residue(),
            Some((U256::from(3), U256::ZERO))
        );
        let general_offset = domain.join(&value(U256::ZERO), &value(U256::MAX));
        assert!(!general_offset.congruence().is_top());
        assert!(literal_memory_singleton_work(&general_offset) > 4_096);
        for offset in [
            TestValue::top(),
            TestValue::top().with_expression(ExprId::fresh().unwrap()),
            general_offset,
            value(U256::MAX),
        ] {
            for (size, query_work) in [
                (&value(U256::ZERO), 608),
                (&numeric_zero, 512),
                (&exact_zero, 512),
                (&modulo_zero, 4_608),
            ] {
                assert_eq!(literal_memory_singleton_work(size), query_work);
                let mut result = execution();
                let before = result.payload.clone();
                let fee = LITERAL_MEMORY_METADATA_WORK + query_work;
                let mut budget = WorkBudget::new(fee);
                assert!(allow_memory(
                    &mut result,
                    &offset,
                    size,
                    &mut context(
                        &config,
                        &mut budget,
                        ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
                    ),
                    7,
                ));
                assert_eq!(budget.used(), fee);
                assert!(!budget.exhausted());
                assert!(result.frontiers.is_empty());
                assert_uninstalled(&result, &before);
            }
        }
        let end = TestValue::unsigned_range(logical::MAX_END, logical::MAX_END).unwrap();
        for (offset, fee) in [
            (&numeric_zero, 1_408),
            (&exact_zero, 1_408),
            (&modulo_zero, 5_504),
        ] {
            let mut result = execution();
            let before = result.payload.clone();
            let mut budget = WorkBudget::new(fee);
            assert!(allow_memory(
                &mut result,
                offset,
                &end,
                &mut context(
                    &config,
                    &mut budget,
                    ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
                ),
                7,
            ));
            assert_uninstalled(&result, &before);
            assert!(result.frontiers.is_empty());
            assert!(!budget.exhausted());
            assert_eq!(budget.used(), fee);
        }
    }

    #[test]
    fn finite_cardinality_prices_first_and_candidate_lookup_without_capacity_amplification() {
        let config = ExecutionConfig::default();
        let config_numeric = crate::analysis::testing::NumericLimits {
            max_constants: 512,
            ..Default::default()
        };
        let domain =
            crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
        let domain = domain.borrowed();
        for (len, query_work) in [
            (1, 608),
            (2, 608),
            (3, 640),
            (10, 864),
            (11, 1_280),
            (71, 1_664),
            (512, 2_432),
        ] {
            let size = (1..len).fold(value(U256::ZERO), |set, item| {
                domain.join(&set, &value(U256::from(item)))
            });
            assert_eq!(size.finite_constants().cardinality(), Some(len));
            assert_eq!(size.singleton(), (len == 1).then_some(U256::ZERO));
            assert_eq!(literal_memory_singleton_work(&size), query_work);
            let fee = LITERAL_MEMORY_METADATA_WORK + query_work;
            let mut budget = WorkBudget::new(fee);
            let mut result = execution();
            let before = result.payload.clone();
            assert_eq!(
                allow_memory(
                    &mut result,
                    &TestValue::top(),
                    &size,
                    &mut context(
                        &config,
                        &mut budget,
                        ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
                    ),
                    7,
                ),
                len == 1,
            );
            assert_eq!(budget.used(), fee);
            assert_eq!(budget.exhausted(), len != 1);
            assert_uninstalled(&result, &before);
            if len == 1 {
                assert!(result.frontiers.is_empty());
            } else {
                assert_eq!(result.frontiers.len(), 1);
                assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
            }
        }
    }

    #[test]
    fn finite_candidate_excluded_by_other_components_cannot_be_treated_as_zero() {
        let summary = component_fixture_domain().apply_detailed(
            revm_bytecode::opcode::LT,
            &[TestValue::unknown_byte(), value(U256::from(256))],
        );
        assert_eq!(summary.status, ReductionStatus::FactLimit);
        let size = summary.value;
        // The finite component is the boolean hull; known bits/interval prove
        // one. Its first candidate zero contradicts those other components.
        assert_eq!(size.finite_constants().cardinality(), Some(2));
        assert!(size.finite_constants().contains(U256::ZERO));
        assert!(!size.contains(U256::ZERO));
        assert_eq!(size.known_bits().singleton(), Some(U256::ONE));
        assert_eq!(size.singleton(), Some(U256::ONE));
        assert_eq!(literal_memory_singleton_work(&size), 608);
        let config = ExecutionConfig::default();
        let config_numeric = crate::analysis::testing::NumericLimits::default();
        let mut budget = WorkBudget::new(1_600);
        let mut result = execution();
        let before = result.payload.clone();
        assert!(!allow_memory(
            &mut result,
            &value(U256::MAX),
            &size,
            &mut context(
                &config,
                &mut budget,
                ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
            ),
            7,
        ));
        assert_eq!(budget.used(), 1_600);
        assert!(!budget.exhausted());
        assert!(result.frontiers.is_empty());
        assert_uninstalled(&result, &before);
    }

    #[test]
    fn literal_preflight_refuses_work_cancellation_and_inherited_exhaustion_atomically() {
        let config = ExecutionConfig::default();
        let config_numeric = crate::analysis::testing::NumericLimits::default();
        let prior = 7;
        for mode in 0..3 {
            let cancellation = analysis_control::Cancellation::new();
            analysis_control::with_cancellation(&cancellation, || {
                let mut result = execution();
                let before = result.payload.clone();
                let mut budget = WorkBudget::new(if mode == 0 {
                    prior + LITERAL_MEMORY_METADATA_WORK - 1
                } else {
                    config.max_work
                });
                assert!(budget.charge(prior));
                if mode == 1 {
                    cancellation.cancel();
                } else if mode == 2 {
                    assert!(!budget.charge(config.max_work));
                }
                assert!(!allow_memory(
                    &mut result,
                    &TestValue::top(),
                    &value(U256::ZERO),
                    &mut context(
                        &config,
                        &mut budget,
                        ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
                    ),
                    7,
                ));
                assert_eq!(budget.used(), prior);
                assert!(budget.exhausted());
                assert_uninstalled(&result, &before);
                assert_eq!(result.frontiers.len(), 1);
                let frontier = &result.frontiers[0];
                assert_eq!(frontier.pc, 7);
                assert_eq!(frontier.target, Some(before.key()));
                assert!(frontier.range.is_none());
                assert!(frontier.payload.is_none());
                assert_eq!(
                    frontier.reason,
                    if mode == 1 {
                        FrontierReason::Relations(QueryReason::Cancelled)
                    } else {
                        FrontierReason::Work
                    }
                );
            });
        }
        // Refusing each later phase retains its actual earlier payments; no
        // singleton query or bounds operation runs on an unpaid reservation.
        assert_eq!(LITERAL_MEMORY_METADATA_WORK, 128);
        assert_eq!(LITERAL_MEMORY_BOUNDS_WORK, 128);
        let operands = (value(U256::ZERO), value(U256::ONE));
        let original = operands.clone();
        for (limit, paid) in [(735, 128), (863, 736), (1_471, 864), (1_599, 1_472)] {
            let mut result = execution();
            let before = result.payload.clone();
            let mut budget = WorkBudget::new(prior + limit);
            assert!(budget.charge(prior));
            assert!(!allow_memory(
                &mut result,
                &operands.0,
                &operands.1,
                &mut context(
                    &config,
                    &mut budget,
                    ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
                ),
                7,
            ));
            assert_eq!(budget.used(), prior + paid);
            assert!(budget.exhausted());
            assert_uninstalled(&result, &before);
            assert_eq!(result.frontiers.len(), 1);
            assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
            assert_eq!(result.frontiers[0].target, Some(before.key()));
            assert_eq!(operands, original);
        }
    }

    #[test]
    fn unknown_symbolic_and_finite_multi_ranges_keep_full_guard_and_partition_fees() {
        use crate::analysis::transfer::relations::identify;
        use revm_bytecode::opcode;

        let quote = 128usize;
        let copy_work = |value: &TestValue| {
            value
                .clone_work()
                .saturating_add(value.work_size())
                .saturating_add(32)
        };
        for capacity in [8, 512] {
            // Keep the private helpers' original 1B allowance; exercise the
            // default enabled relation policy without changing those fixtures.
            let config = ExecutionConfig {
                max_work: 1_000_000_000,
                ..ExecutionConfig::default()
            };
            let config_numeric = crate::analysis::testing::NumericLimits {
                max_constants: capacity,
                ..Default::default()
            };
            assert!(config_numeric.relations.enabled);
            let domain =
                crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
            let domain = domain.borrowed();
            let source = ExprId::fresh().unwrap();
            for (offset, size) in [
                (value(U256::ZERO), TestValue::top()),
                (
                    value(U256::ZERO),
                    TestValue::top().with_expression(source.clone()),
                ),
                (
                    value(U256::ZERO),
                    domain
                        .join(&value(U256::ZERO), &value(U256::MAX))
                        .with_expression(source.clone()),
                ),
                (TestValue::top(), value(U256::ONE)),
            ] {
                let preflight = LITERAL_MEMORY_METADATA_WORK
                    + literal_memory_singleton_work(&size)
                    + if size.singleton().is_some() {
                        assert!(offset.singleton().is_none());
                        LITERAL_MEMORY_METADATA_WORK + literal_memory_singleton_work(&offset)
                    } else {
                        0
                    };
                assert_eq!(
                    preflight,
                    if size.constants().is_some() {
                        if size.singleton().is_some() {
                            1_376
                        } else {
                            4_832
                        }
                    } else {
                        640
                    },
                );
                // Builder stages replaced the blanket reservation. Quote their
                // owners from reference arguments, independently of budget.used().
                let mut queries = RangeQueries {
                    size: SingletonProof::Answer(size.singleton()),
                    offset: if size.singleton().is_some() {
                        SingletonProof::Answer(offset.singleton())
                    } else {
                        SingletonProof::NotQueried
                    },
                };
                let mut receipts = vec![quote];
                let [guard_offset, guard_size] = [(&offset, queries.offset), (&size, queries.size)]
                    .map(|(input, proof)| {
                        receipts.push(quote);
                        receipts.push(copy_work(input).saturating_add(2));
                        let singleton = match proof {
                            SingletonProof::Answer(answer) => answer,
                            SingletonProof::NotQueried if input.expression().is_none() => {
                                receipts.push(literal_memory_singleton_work(input));
                                input.singleton()
                            }
                            SingletonProof::NotQueried => None,
                        };
                        if singleton.is_some() {
                            input.clone()
                        } else {
                            identify(input.clone(), config_numeric.relations.enabled)
                        }
                    });
                receipts.push(
                    domain
                        .literal_quote_work(logical::MAX_END)
                        .saturating_mul(2),
                );
                let mut operations = 0;
                let mut apply = |op, args: &[TestValue], copied| {
                    if copied {
                        receipts.push(quote);
                        receipts.push(
                            args.iter()
                                .fold(0usize, |work, arg| work.saturating_add(copy_work(arg))),
                        );
                    }
                    receipts.push(quote);
                    // The selected numeric operation quote bounds the actual
                    // output structure; its release is an independent owner.
                    use crate::domain::EvmNumericAlgebra as _;
                    let numeric = domain
                        .numeric_algebra()
                        .operation_work(op, args.iter().map(TestValue::numeric));
                    let nodes = args.iter().fold(1usize, |nodes, arg| {
                        nodes.saturating_add(arg.expression().map_or(1, |expr| expr.nodes()))
                    });
                    receipts.push(
                        numeric
                            .saturating_add(crate::domain::provenance::MAX_ORIGINS)
                            .saturating_add(nodes)
                            .saturating_add(32),
                    );
                    receipts.push(domain.operation_work(op, args));
                    operations += 1;
                    domain.apply_detailed(op, args).value
                };
                let bound = value(logical::MAX_END);
                let empty = apply(opcode::ISZERO, std::slice::from_ref(&guard_size), false);
                let greater = apply(opcode::GT, &[guard_offset.clone(), bound.clone()], true);
                let offset_fits = apply(opcode::ISZERO, &[greater], false);
                let remaining = apply(opcode::SUB, &[bound, guard_offset], true);
                let greater = apply(opcode::GT, &[guard_size, remaining], true);
                let size_fits = apply(opcode::ISZERO, &[greater], false);
                let fits = apply(opcode::AND, &[offset_fits, size_fits], false);
                let condition = apply(opcode::OR, &[empty, fits], false);
                assert_eq!(operations, 8);
                let guard_work = receipts
                    .iter()
                    .fold(0usize, |work, fee| work.saturating_add(*fee));
                // A complete independent receipt also rejects a missing or
                // doubled debit when adjacent one-short cuts have equal prices.
                let mut guard_budget = WorkBudget::new(config.max_work);
                let _guard = super::super::guard::build_after_queries(
                    &offset,
                    &size,
                    &mut queries,
                    &mut context(
                        &config,
                        &mut guard_budget,
                        ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec())),
                    ),
                )
                .expect("the original 1B allowance funds the complete guard");
                assert_eq!(guard_budget.used(), guard_work);
                assert!(!guard_budget.exhausted());
                receipts.push(condition.numeric().work_size().saturating_mul(2));
                let mut result = execution();
                let before = result.payload.clone();
                receipts.push(
                    before
                        .work_size()
                        .saturating_mul(3)
                        .saturating_add(condition.work_size()),
                );
                let mut paid = preflight;
                for (stage, fee) in receipts.into_iter().enumerate() {
                    let mut result = execution();
                    let before = result.payload.clone();
                    let prior = 7usize;
                    let limit = prior.saturating_add(paid).saturating_add(fee) - 1;
                    let mut budget = WorkBudget::new(limit);
                    assert!(budget.charge(prior));
                    assert!(!allow_memory(
                        &mut result,
                        &offset,
                        &size,
                        &mut context(
                            &config,
                            &mut budget,
                            ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
                        ),
                        7,
                    ));
                    assert_eq!(budget.used(), prior.saturating_add(paid), "stage {stage}");
                    assert!(budget.exhausted());
                    assert_uninstalled(&result, &before);
                    assert_eq!(result.frontiers.len(), 1);
                    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
                    paid = paid.saturating_add(fee);
                }
                let mut budget = WorkBudget::new(config.max_work);
                assert!(allow_memory(
                    &mut result,
                    &offset,
                    &size,
                    &mut context(
                        &config,
                        &mut budget,
                        ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
                    ),
                    7,
                ));
                assert!(budget.used() > preflight + guard_work);
                assert!(!budget.exhausted());
                assert!(result.frontiers.is_empty());
                assert_eq!(result.payload.active(), before.active());
                assert_eq!(result.payload.store, before.store);
                assert!(result.outcomes.is_empty());
                assert!(result.successors.is_empty());
                assert!(
                    result
                        .payload
                        .relations
                        .constraints()
                        .any(|atom| matches!(atom, Constraint::Truth { nonzero: true, .. }))
                );
                if size.expression().is_some() || offset.expression().is_some() {
                    let expression = memory_condition(&offset, &size, domain)
                        .symbolic_expression()
                        .unwrap();
                    assert!(result.payload.relations.constraints().any(|atom| atom
                        == &Constraint::Truth {
                            expression: expression.clone(),
                            nonzero: true,
                        }));
                }
            }
        }
    }

    #[test]
    fn inherited_symbolic_limits_and_invalid_operand_roots_do_not_take_the_fastpath() {
        let config = ExecutionConfig::default();
        let mut config_numeric = crate::analysis::testing::NumericLimits::default();
        config_numeric.relations.max_nodes = 2;
        let source = ExprId::fresh().unwrap();
        let expression = ExprId::operation(
            revm_bytecode::opcode::ADD,
            &[source.clone(), source],
            Default::default(),
        )
        .unwrap();
        assert!(expression.nodes() > config_numeric.relations.max_nodes);
        for (offset, size) in [
            (value(U256::ZERO).with_symbolic_limit(), value(U256::ZERO)),
            (value(U256::MAX), value(U256::ZERO).with_symbolic_limit()),
            (
                value(U256::ZERO).with_expression(expression.clone()),
                value(U256::ZERO),
            ),
            (
                value(U256::MAX),
                value(U256::ZERO).with_expression(expression),
            ),
        ] {
            let mut result = execution();
            let before = result.payload.clone();
            let operands = (offset.clone(), size.clone());
            let mut budget = WorkBudget::new(LITERAL_MEMORY_METADATA_WORK);
            assert!(!allow_memory(
                &mut result,
                &offset,
                &size,
                &mut context(
                    &config,
                    &mut budget,
                    ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
                ),
                7,
            ));
            assert_eq!(budget.used(), LITERAL_MEMORY_METADATA_WORK);
            assert!(budget.exhausted());
            assert_eq!((offset, size), operands);
            assert_uninstalled(&result, &before);
            assert_eq!(result.frontiers.len(), 1);
            assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
        }
    }
}
