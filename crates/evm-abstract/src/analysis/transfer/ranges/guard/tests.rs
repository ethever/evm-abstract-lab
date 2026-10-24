use super::{
    Builder, GuardRetention, QUOTE_WORK, RangeKind, build, literal_memory_singleton_work, logical,
    output_release_work, release_work, value_copy_work,
};
#[cfg(test)]
use crate::analysis::testing::TestValue;
use crate::domain::ValueAlgebra;
use crate::{
    U256,
    analysis::{
        ExecutionConfig, FrontierReason, OutcomeKind,
        machine::fixtures::{fixture, word},
        transfer::{RetentionCause, boundary, empty_execution, initial},
    },
    domain::{
        EvmNumericAlgebra, EvmReduction, EvmValue, FiniteConstantSet, FiniteSetAlgebra,
        ProductAlgebra, WordElement,
        identity::Symbol,
        provenance::Origin,
        relational::QueryReason,
        symbolic::{ExprError, ExprId, ExprLimits},
    },
    resource::WorkBudget,
};
use revm_bytecode::opcode;

use crate::analysis::transfer::testing::procedure_context as context;

fn inputs() -> (TestValue, TestValue) {
    let mut offset = TestValue::top()
        .with_symbol(Symbol::CallValue, Some(71))
        .with_identity(81, 1);
    offset.set_origin(Origin::CallValue);
    let mut size = TestValue::top()
        .with_symbol(Symbol::CalldataLength, Some(71))
        .with_identity(81, 2);
    size.set_origin(Origin::Calldata);
    (offset, size)
}

// Independent expansion of the original unbudgeted helper. Keep the complete
// reductions so a dropped expression cannot hide a numeric/metadata difference.
type GuardStep<N> = (u8, Vec<EvmValue<N>>, EvmReduction<N>);

fn reference<D: EvmNumericAlgebra>(
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    domain: crate::domain::AlgebraRef<'_, D>,
) -> Vec<GuardStep<D::Element>> {
    let mut steps = Vec::new();
    let mut apply = |op, args: &[EvmValue<D::Element>]| {
        let reduced = domain.apply_detailed(op, args);
        steps.push((op, args.to_vec(), reduced.clone()));
        reduced.value
    };
    let limit = EvmValue::constant(logical::MAX_END);
    let empty = apply(opcode::ISZERO, std::slice::from_ref(size));
    let greater = apply(opcode::GT, &[offset.clone(), limit.clone()]);
    let offset_fits = apply(opcode::ISZERO, &[greater]);
    let remaining = apply(opcode::SUB, &[limit, offset.clone()]);
    let greater = apply(opcode::GT, &[size.clone(), remaining]);
    let size_fits = apply(opcode::ISZERO, &[greater]);
    let fits = apply(opcode::AND, &[offset_fits, size_fits]);
    apply(opcode::OR, &[empty, fits]);
    steps
}

fn assert_same_value<N: WordElement>(actual: &EvmValue<N>, expected: &EvmValue<N>) {
    assert_eq!(actual, expected);
    assert_eq!(actual.expression(), expected.expression());
    assert_eq!(actual.identity(), expected.identity());
    assert_eq!(actual.provenance(), expected.provenance());
    assert_eq!(
        actual.symbolic_limit_reached(),
        expected.symbolic_limit_reached()
    );
    if expected.same_identity(expected) {
        assert!(actual.same_identity(expected));
    }
}

fn verify_paid_operations<D: EvmNumericAlgebra>(numeric: D) {
    let algebra = ValueAlgebra::new(numeric);
    let domain = algebra.borrowed();
    let config = ExecutionConfig::default();
    let offset = EvmValue::<D::Element>::top()
        .with_symbol(Symbol::CallValue, Some(71))
        .with_identity(81, 1);
    let size = EvmValue::<D::Element>::top()
        .with_symbol(Symbol::CalldataLength, Some(71))
        .with_identity(81, 2);
    let finite = domain
        .join(
            &EvmValue::constant(U256::ZERO),
            &EvmValue::constant(U256::MAX),
        )
        .with_expression(ExprId::input(71, Symbol::Timestamp));
    for (offset, size) in [
        (offset, size),
        (finite.clone(), finite),
        (
            EvmValue::constant(U256::ZERO),
            EvmValue::constant(logical::MAX_END),
        ),
    ] {
        let steps = reference(&offset, &size, domain);
        assert_eq!(
            steps.iter().map(|step| step.0).collect::<Vec<_>>(),
            [
                opcode::ISZERO,
                opcode::GT,
                opcode::ISZERO,
                opcode::SUB,
                opcode::GT,
                opcode::ISZERO,
                opcode::AND,
                opcode::OR
            ]
        );
        let mut budget = WorkBudget::new(usize::MAX);
        let mut builder = Builder {
            domain,
            budget: &mut budget,
            retention: GuardRetention::new(&offset, &size),
        };
        for (step, (op, args, expected)) in steps.iter().enumerate() {
            let numeric_work = domain
                .numeric_algebra()
                .operation_work(*op, args.iter().map(EvmValue::numeric));
            let actual = if matches!(step, 1 | 3 | 4) {
                builder.copied(*op, [&args[0], &args[1]])
            } else {
                builder.apply(*op, args)
            }
            .unwrap();
            assert_same_value(&actual, &expected.value);
            assert!(actual.numeric().work_size() <= numeric_work);
            assert!(
                release_work(actual.work_size()) <= output_release_work(domain, numeric_work, args),
                "opcode {op:#x}: output size {}, release quote {}",
                actual.work_size(),
                output_release_work(domain, numeric_work, args),
            );
        }
        let mut budget = WorkBudget::new(usize::MAX);
        let built = build(&offset, &size, &mut context(&config, &mut budget, domain)).unwrap();
        assert_same_value(&built.request.offset, &offset);
        assert_same_value(&built.request.size, &size);
        assert_eq!(built.request.limit, EvmValue::constant(logical::MAX_END));
        assert_same_value(&built.condition, &steps[7].2.value);
        assert_same_value(
            &built.condition,
            &super::super::memory_condition(&offset, &size, domain),
        );
        assert!(!budget.exhausted());
    }
}
#[test]
fn paid_guard_operations_keep_semantics_for_product_and_finite_carriers() {
    for enabled in [false, true] {
        let spec = crate::domain::DomainSpec::default().with_relations(
            crate::domain::relational::RelationLimits {
                enabled,
                ..Default::default()
            },
        );
        verify_paid_operations(ProductAlgebra::from_spec(spec));
        verify_paid_operations(FiniteSetAlgebra::from_spec(spec));
    }
}

#[test]
fn paid_guard_operations_keep_semantics_for_opaque_carrier() {
    use crate::analysis::transfer::query::tests::{Policy, RECEIPT};

    let _receipt = RECEIPT.scope();
    // This carrier has no finite candidate view, and its structural bound is
    // independent of Product's inline fields and native finite cardinality.
    verify_paid_operations(Policy);
}

#[test]
fn opaque_output_release_and_execution_are_prepaid_before_the_operation() {
    use crate::analysis::transfer::query::tests::{Policy, RECEIPT, Word};

    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.cancel_on_apply = true);
    let algebra = ValueAlgebra::new(Policy);
    let domain = algebra.borrowed();
    let args = [EvmValue::<Word>::top(), EvmValue::top()];
    let numeric_work = domain
        .numeric_algebra()
        .operation_work(opcode::SUB, args.iter().map(EvmValue::numeric));
    let release = output_release_work(domain, numeric_work, &args);
    let execution = domain.operation_work(opcode::SUB, &args);
    for (remaining, paid) in [(release - 1, 0), (release + execution - 1, release)] {
        let cancellation = analysis_control::Cancellation::new();
        analysis_control::with_cancellation(&cancellation, || {
            let mut budget = WorkBudget::new(17 + QUOTE_WORK + remaining);
            assert!(budget.charge(17));
            let mut builder = Builder {
                domain,
                budget: &mut budget,
                retention: GuardRetention::new(&args[0], &args[1]),
            };
            assert!(matches!(
                builder.apply(opcode::SUB, &args),
                Err(FrontierReason::Work)
            ));
            assert_eq!(budget.used(), 17 + QUOTE_WORK + paid);
            assert!(budget.exhausted());
            // The opaque hook cancels on entry. A refused output reservation
            // or execution fee must leave that hook entirely unexecuted.
            assert!(!cancellation.is_cancelled());
        });
    }
}

// Quote only cached metadata of the already computed reference steps. Each
// pair is the paid prefix and the next actual ProductAlgebra operation's exact price.
fn operation_refusals(
    offset: &TestValue,
    size: &TestValue,
    domain: crate::domain::AlgebraRef<'_, ProductAlgebra>,
) -> Vec<(usize, usize)> {
    let mut prefix = QUOTE_WORK;
    for value in [offset, size] {
        prefix += QUOTE_WORK + value_copy_work(value) + 2;
        if value.expression().is_none() {
            prefix += literal_memory_singleton_work(value);
        }
    }
    prefix += domain.literal_quote_work(logical::MAX_END) * 2;
    reference(offset, size, domain)
        .iter()
        .enumerate()
        .map(|(step, (op, args, _))| {
            if matches!(step, 1 | 3 | 4) {
                prefix += QUOTE_WORK + args.iter().map(value_copy_work).sum::<usize>();
            }
            let numeric_work = domain
                .numeric_algebra()
                .operation_work(*op, args.iter().map(EvmValue::numeric));
            prefix += QUOTE_WORK + output_release_work(domain, numeric_work, args);
            let cost = domain.operation_work(*op, args);
            let refusal = (prefix, cost);
            prefix += cost;
            refusal
        })
        .collect()
}

#[test]
fn caller_copy_fees_cover_native_finite_carrier_and_saturate_before_actions() {
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.enabled = false;
    let domain =
        crate::domain::ValueAlgebra::new(FiniteSetAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let (left, right) = (
        EvmValue::<FiniteConstantSet>::constant(U256::ONE),
        EvmValue::constant(U256::from(2)),
    );
    let mut budget =
        WorkBudget::new(QUOTE_WORK + value_copy_work(&left) + value_copy_work(&right) - 1);
    assert!(domain.operation_work(opcode::GT, &[left.clone(), right.clone()]) > 0);
    let mut builder = Builder {
        domain,
        budget: &mut budget,
        retention: GuardRetention::new(&left, &right),
    };
    assert!(builder.copied(opcode::GT, [&left, &right]).is_err());
    assert_eq!(budget.used(), QUOTE_WORK);
    assert!(budget.exhausted());
    assert_eq!(release_work(usize::MAX), usize::MAX);

    // A small shared DAG has an enormous cached expanded-node count. The
    // lifetime fee must refuse before cloning or calling ProductAlgebra. Numeric
    // clone_work is separate, so the finite release bound need not saturate.
    let limits = ExprLimits {
        max_nodes: usize::MAX,
        max_depth: usize::MAX,
    };
    let mut expression = ExprId::input(71, Symbol::CallValue);
    for _ in 0..usize::BITS - 2 {
        expression =
            ExprId::operation(opcode::ADD, &[expression.clone(), expression], limits).unwrap();
    }
    let offset = TestValue::top().with_expression(expression);
    let mut budget = WorkBudget::new(1_000_000);
    let config = ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    assert!(
        build(
            &offset,
            &word(1),
            &mut context(
                &config,
                &mut budget,
                ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
            )
        )
        .is_err()
    );
    assert_eq!(budget.used(), QUOTE_WORK * 2);
    assert!(budget.exhausted());
}

#[test]
fn each_of_eight_operation_refusals_keeps_paid_prefix_payload_and_prior_failure() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = ExecutionConfig::default();
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    // Step 7 loses its expression; refusing step 8 must still report plain Work.
    config_numeric.relations.max_nodes = 10;
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let (offset, size) = inputs();
    let payload = initial(&world, &entry, domain).unwrap();
    let mut covered = empty_execution(payload.clone());
    let mut full = WorkBudget::new(usize::MAX);
    assert!(super::super::logical::failure(
        &mut covered,
        &mut context(
            &config,
            &mut full,
            ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
        ),
        19
    ));
    let gas_work = full.used();
    let preflight =
        super::super::LITERAL_MEMORY_METADATA_WORK + literal_memory_singleton_work(&size);
    let refusals = operation_refusals(&offset, &size, domain);
    assert_eq!(refusals.len(), 8);
    for (step, (prefix, cost)) in refusals.into_iter().enumerate() {
        let mut result = empty_execution(payload.clone());
        let mut budget = WorkBudget::new(7 + gas_work + preflight + prefix + cost - 1);
        assert!(budget.charge(7));
        assert!(
            !super::super::memory(
                &mut result,
                &offset,
                &size,
                &mut context(
                    &config,
                    &mut budget,
                    ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
                ),
                19,
            ),
            "step {}",
            step + 1
        );
        assert_eq!(
            budget.used(),
            7 + gas_work + preflight + prefix,
            "step {}",
            step + 1
        );
        assert!(budget.exhausted());
        assert_eq!(result.payload, payload);
        assert_eq!(result.outcomes.len(), 1);
        assert_eq!(result.outcomes[0].kind, OutcomeKind::Failure);
        assert_eq!(result.outcomes[0].data, covered.outcomes[0].data);
        assert_eq!(result.outcomes[0].store, covered.outcomes[0].store);
        assert!(result.completed_calls.is_empty());
        assert!(result.successors.is_empty());
        assert!(result.diagnostics.is_empty());
        assert!(result.retention_failures.is_empty());
        assert_eq!(result.frontiers.len(), 1);
        assert_eq!(result.frontiers[0].pc, 19);
        assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
        assert!(result.frontiers[0].range.is_none());
    }
}

#[test]
fn finite_raw_guard_error_attaches_only_to_the_actual_missing_expression_frontier() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = ExecutionConfig::default();
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.max_nodes = 10;
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let (offset, size) = inputs();
    let steps = reference(&offset, &size, domain);
    assert!(
        steps[..6]
            .iter()
            .all(|step| step.2.symbolic_error.is_none())
    );
    assert!(matches!(
        steps[6].2.symbolic_error,
        Some(ExprError::Limit { nodes: 11, .. })
    ));
    assert!(steps[7].2.symbolic_error.is_none());
    let mut result = empty_execution(initial(&world, &entry, domain).unwrap());
    let before = result.payload.clone();
    boundary(&mut result, 3, FrontierReason::Work);
    let mut budget = WorkBudget::new(config.max_work);
    assert!(!super::super::memory(
        &mut result,
        &offset,
        &size,
        &mut context(
            &config,
            &mut budget,
            ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
        ),
        19,
    ));
    assert!(!budget.exhausted());
    assert_eq!(result.payload, before);
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(result.outcomes[0].kind, OutcomeKind::Failure);
    assert_eq!(result.retention_failures, [(1, RetentionCause::Expression)]);
    assert_eq!(result.frontiers.len(), 2);
    assert_eq!(result.frontiers[1].pc, 19);
    assert_eq!(
        result.frontiers[1].reason,
        FrontierReason::Relations(QueryReason::ExpressionLimit)
    );
    let witness = result.frontiers[1].range.as_ref().unwrap();
    assert_eq!(witness.request.kind, RangeKind::LogicalMemory);
    assert_same_value(&witness.request.offset, &offset);
    assert_same_value(&witness.request.size, &size);
    assert_same_value(&witness.condition, &steps[7].2.value);
    assert_eq!(witness.nonzero, None);
    assert_eq!(*witness.payload, before);
}

#[test]
fn raw_finite_error_does_not_turn_a_proven_constant_guard_into_a_failure() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = ExecutionConfig::default();
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.max_nodes = 1;
    let leaf = ExprId::input(71, Symbol::CallValue);
    let expression = ExprId::operation(opcode::NOT, &[leaf], Default::default()).unwrap();
    let offset = TestValue::top().with_expression(expression);
    let size = word(0);
    let mut budget = WorkBudget::new(config.max_work);
    let built = build(
        &offset,
        &size,
        &mut context(
            &config,
            &mut budget,
            ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec())),
        ),
    )
    .unwrap();
    assert_eq!(built.retention.cause(), Some(RetentionCause::Expression));
    assert_eq!(built.condition.singleton(), Some(U256::ONE));
    // allow_memory bypasses literal admission because the offset root exceeds
    // the cap, then keeps the original proven-constant success path.
    let mut result = empty_execution(
        initial(
            &world,
            &entry,
            ValueAlgebra::new(&ProductAlgebra::default()),
        )
        .unwrap(),
    );
    assert!(super::super::allow_memory(
        &mut result,
        &offset,
        &size,
        &mut context(
            &config,
            &mut budget,
            ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
        ),
        19,
    ));
    assert!(result.frontiers.is_empty());
    assert!(result.retention_failures.is_empty());
    assert!(!budget.exhausted());
}

#[test]
fn literal_and_cancelled_paths_keep_their_original_boundaries_without_guard_fallback() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let payload = initial(
        &world,
        &entry,
        ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec())),
    )
    .unwrap();
    let mut result = empty_execution(payload.clone());
    let fee = super::super::LITERAL_MEMORY_METADATA_WORK + 64 * 4;
    let mut budget = WorkBudget::new(fee);
    assert!(super::super::allow_memory(
        &mut result,
        &TestValue::top(),
        &word(0),
        &mut context(
            &config,
            &mut budget,
            ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
        ),
        19,
    ));
    assert_eq!(budget.used(), fee);
    assert!(!budget.exhausted());
    assert_eq!(result.payload, payload);
    assert!(result.frontiers.is_empty());

    let cancellation = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&cancellation, || {
        let mut result = empty_execution(payload.clone());
        let mut budget = WorkBudget::new(config.max_work);
        assert!(super::super::logical::failure(
            &mut result,
            &mut context(
                &config,
                &mut budget,
                ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
            ),
            19
        ));
        let paid = budget.used();
        cancellation.cancel();
        let (offset, size) = inputs();
        assert!(!super::super::allow_memory(
            &mut result,
            &offset,
            &size,
            &mut context(
                &config,
                &mut budget,
                ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
            ),
            19,
        ));
        assert_eq!(budget.used(), paid);
        assert_eq!(result.payload, payload);
        assert_eq!(result.outcomes.len(), 1);
        assert_eq!(result.outcomes[0].kind, OutcomeKind::Failure);
        assert_eq!(result.frontiers.len(), 1);
        assert_eq!(result.frontiers[0].pc, 19);
        assert_eq!(
            result.frontiers[0].reason,
            FrontierReason::Relations(QueryReason::Cancelled)
        );
        assert!(result.retention_failures.is_empty());
    });
}

#[test]
fn identification_outcomes_preserve_value_layers_and_the_same_payment() {
    for flagged in [false, true] {
        let mut original = inputs().0;
        original.forget_expression();
        if flagged {
            original = original.with_symbolic_limit();
        }
        for expression in [None, Some(ExprId::input(83, Symbol::Timestamp))] {
            let expected_failure = expression.is_none();
            let expected = expression.as_ref().map_or_else(
                || original.clone(),
                |expression| original.clone().with_expression(expression.clone()),
            );
            let mut budget = WorkBudget::new(usize::MAX);
            let numeric = ProductAlgebra::default();
            let mut builder = Builder {
                domain: ValueAlgebra::new(&numeric),
                budget: &mut budget,
                retention: GuardRetention::new(&original, &original),
            };
            let mut allocations = 0;
            let (actual, identity_failed) = builder
                .identify(&original, true, || {
                    allocations += 1;
                    expression
                })
                .unwrap();
            assert_eq!(allocations, 1);
            assert_eq!(identity_failed, expected_failure);
            assert_same_value(&actual, &expected);
            assert_eq!(actual.identity(), original.identity());
            assert_eq!(actual.provenance(), original.provenance());
            assert_eq!(actual.symbolic_limit_reached(), flagged);
            assert_eq!(
                budget.used(),
                QUOTE_WORK
                    + value_copy_work(&original)
                    + 2
                    + literal_memory_singleton_work(&original)
            );
            assert!(!budget.exhausted());
        }
    }
}

#[test]
fn identification_short_circuits_without_calling_the_allocator() {
    for (original, enabled) in [
        (TestValue::top(), false),
        (inputs().0.with_symbolic_limit(), true),
        (word(7), true),
        (
            TestValue::unsigned_range(U256::from(7), U256::from(7)).unwrap(),
            true,
        ),
        (
            word(7).with_expression(ExprId::input(83, Symbol::Timestamp)),
            true,
        ),
    ] {
        let mut budget = WorkBudget::new(usize::MAX);
        let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
        let domain = domain.borrowed();
        let mut builder = Builder {
            domain,
            budget: &mut budget,
            retention: GuardRetention::new(&original, &original),
        };
        let (actual, identity_failed) = builder
            .identify(&original, enabled, || {
                panic!("identification must not allocate on a short-circuited path")
            })
            .unwrap();
        assert_same_value(&actual, &original);
        assert!(!identity_failed);
    }
}

#[test]
fn either_unmarked_operand_identity_failure_vetoes_finite_guard_evidence() {
    for missing in [0, 1] {
        let config = ExecutionConfig::default();
        let mut config_numeric = crate::analysis::testing::NumericLimits::default();
        config_numeric.relations.max_nodes = 1;
        let domain =
            crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
        let domain = domain.borrowed();
        let (offset, size) = if missing == 0 {
            (TestValue::top(), inputs().1)
        } else {
            (inputs().0, TestValue::top())
        };
        let steps = reference(&offset, &size, domain);
        assert_eq!(
            steps[missing].2.symbolic_error,
            Some(ExprError::Limit {
                nodes: 2 + missing,
                depth: 2
            }),
            "missing operand {missing}"
        );
        if missing == 0 {
            assert!(
                steps[1..]
                    .iter()
                    .all(|step| step.2.symbolic_error.is_none())
            );
        }

        let mut allocations = 0;
        let mut budget = WorkBudget::new(usize::MAX);
        let mut built = super::build_with_allocator(
            &offset,
            &size,
            &mut context(
                &config,
                &mut budget,
                ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec())),
            ),
            || {
                allocations += 1;
                None
            },
        )
        .unwrap();
        assert_eq!(allocations, 1, "missing operand {missing}");
        assert_same_value(&built.request.offset, &offset);
        assert!(
            !built.request.offset.symbolic_limit_reached(),
            "missing operand {missing}"
        );
        assert!(
            !built.request.size.symbolic_limit_reached(),
            "missing operand {missing}"
        );
        assert_same_value(&built.request.size, &size);
        assert_same_value(&built.condition, &steps[7].2.value);
        assert!(built.condition.singleton().is_none());
        assert!(built.condition.symbolic_expression().is_none());
        assert!(built.condition.may_be_zero() && built.condition.may_be_nonzero());
        let (prefix, cost) = *operation_refusals(&offset, &size, domain).last().unwrap();
        assert_eq!(budget.used(), prefix + cost);
        assert!(!budget.exhausted());
        assert_eq!(built.retention.cause(), None, "missing operand {missing}");
        built.retention.record(None);
        built
            .retention
            .record(steps[missing].2.symbolic_error.as_ref());
        assert_eq!(built.retention.cause(), None, "missing operand {missing}");

        let mut previously_eligible = GuardRetention::new(&offset, &size);
        previously_eligible.record(steps[missing].2.symbolic_error.as_ref());
        assert_eq!(
            previously_eligible.cause(),
            Some(RetentionCause::Expression)
        );
        previously_eligible.veto_identity_failure();
        previously_eligible.record(steps[missing].2.symbolic_error.as_ref());
        assert_eq!(previously_eligible.cause(), None);

        let (world, entry) = fixture(&[(0x101, "00")]);
        let mut result = empty_execution(
            initial(
                &world,
                &entry,
                ValueAlgebra::new(&ProductAlgebra::default()),
            )
            .unwrap(),
        );
        let before = result.payload.clone();
        assert!(
            super::super::partition(
                &mut result,
                &built.request,
                super::super::Condition::prepare(&built.condition, built.singleton, &mut budget)
                    .unwrap(),
                Some(&built.retention),
                &mut context(
                    &config,
                    &mut budget,
                    ValueAlgebra::new(&ProductAlgebra::from_spec(config_numeric.spec()))
                ),
                19,
            )
            .is_none()
        );
        assert_eq!(result.payload, before);
        assert_eq!(result.frontiers.len(), 1);
        assert_eq!(result.frontiers[0].pc, 19);
        assert_eq!(
            result.frontiers[0].reason,
            FrontierReason::Relations(QueryReason::ExpressionLimit)
        );
        assert!(result.retention_failures.is_empty());
        assert!(result.outcomes.is_empty());
        assert!(result.successors.is_empty());
        assert!(!budget.exhausted());
        let witness = result.frontiers[0].range.as_ref().unwrap();
        assert_same_value(&witness.request.offset, &offset);
        assert_same_value(&witness.request.size, &size);
        assert_same_value(&witness.condition, &built.condition);
        assert_eq!(*witness.payload, before);
    }
}
