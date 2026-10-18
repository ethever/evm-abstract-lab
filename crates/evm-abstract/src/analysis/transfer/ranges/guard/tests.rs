use super::{
    AbstractValue, Builder, CONSTANT_WORK_SIZE, Domain, GuardRetention, QUOTE_WORK, RangeKind,
    TransferContext, build, copy_work, literal_memory_singleton_work, logical, output_release_work,
    release_work,
};
use crate::{
    U256,
    analysis::{
        ExecutionConfig, FrontierReason, OutcomeKind,
        machine::fixtures::{fixture, word},
        transfer::{RetentionCause, boundary, empty_execution, initial},
    },
    domain::{
        Profile, Reduction,
        identity::Symbol,
        provenance::Origin,
        relational::QueryReason,
        symbolic::{ExprError, ExprId, ExprLimits},
    },
    resource::WorkBudget,
};
use revm_bytecode::opcode;

fn context<'a>(config: &'a ExecutionConfig, budget: &'a mut WorkBudget) -> TransferContext<'a> {
    TransferContext {
        config,
        domain: config.domain().unwrap(),
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: true,
        discover_code: false,
    }
}

fn inputs() -> (AbstractValue, AbstractValue) {
    let mut offset = AbstractValue::top()
        .with_symbol(Symbol::CallValue, Some(71))
        .with_identity(81, 1);
    offset.set_origin(Origin::CallValue);
    let mut size = AbstractValue::top()
        .with_symbol(Symbol::CalldataLength, Some(71))
        .with_identity(81, 2);
    size.set_origin(Origin::Calldata);
    (offset, size)
}

// Independent expansion of the original unbudgeted helper. Keep the complete
// reductions so a dropped expression cannot hide a numeric/metadata difference.
fn reference(
    offset: &AbstractValue,
    size: &AbstractValue,
    domain: Domain,
) -> Vec<(u8, Vec<AbstractValue>, Reduction)> {
    let mut steps = Vec::new();
    let mut apply = |op, args: &[AbstractValue]| {
        let reduced = domain.apply_detailed(op, args);
        steps.push((op, args.to_vec(), reduced.clone()));
        reduced.value
    };
    let limit = AbstractValue::constant(logical::MAX_END);
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

fn assert_same_value(actual: &AbstractValue, expected: &AbstractValue) {
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

#[test]
fn all_eight_paid_operations_preserve_the_original_values_in_both_profiles() {
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        for enabled in [false, true] {
            let mut config = ExecutionConfig::default();
            config.analysis.domain_profile = profile;
            config.analysis.relations.enabled = enabled;
            let domain = config.domain().unwrap();
            let (offset, size) = inputs();
            let finite = domain
                .join(&word(0), &AbstractValue::constant(U256::MAX))
                .with_expression(ExprId::input(71, Symbol::Timestamp));
            for (offset, size) in [
                (offset, size),
                (finite.clone(), finite),
                (word(0), AbstractValue::constant(logical::MAX_END)),
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
                        opcode::OR,
                    ],
                );
                let mut budget = WorkBudget::new(usize::MAX);
                let mut builder = Builder {
                    domain,
                    budget: &mut budget,
                    retention: GuardRetention::new(&offset, &size),
                };
                for (step, (op, args, expected)) in steps.iter().enumerate() {
                    let actual = if matches!(step, 1 | 3 | 4) {
                        builder.copied(*op, [&args[0], &args[1]])
                    } else {
                        builder.apply(*op, args)
                    }
                    .unwrap();
                    assert_same_value(&actual, &expected.value);
                    assert!(
                        release_work(actual.work_size()) <= output_release_work(domain, *op, args)
                    );
                }
                let mut budget = WorkBudget::new(usize::MAX);
                let built = build(&offset, &size, &mut context(&config, &mut budget)).unwrap();
                assert_same_value(&built.request.offset, &offset);
                assert_same_value(&built.request.size, &size);
                assert_eq!(
                    built.request.limit,
                    AbstractValue::constant(logical::MAX_END)
                );
                assert_same_value(&built.condition, &steps[7].2.value);
                assert_same_value(
                    &built.condition,
                    &super::super::memory_condition(&offset, &size, domain),
                );
                assert!(!budget.exhausted());
            }
        }
    }
}

// Quote only cached metadata of the already computed reference steps. Each
// pair is the paid prefix and the next actual Domain operation's exact price.
fn operation_refusals(
    offset: &AbstractValue,
    size: &AbstractValue,
    domain: Domain,
) -> Vec<(usize, usize)> {
    let mut prefix = QUOTE_WORK;
    for value in [offset, size] {
        prefix +=
            QUOTE_WORK + copy_work(value.work_size() + 1) + literal_memory_singleton_work(value);
    }
    prefix += copy_work(CONSTANT_WORK_SIZE) * 2;
    reference(offset, size, domain)
        .iter()
        .enumerate()
        .map(|(step, (op, args, _))| {
            if matches!(step, 1 | 3 | 4) {
                prefix += QUOTE_WORK
                    + args
                        .iter()
                        .map(|arg| copy_work(arg.work_size()))
                        .sum::<usize>();
            }
            prefix += QUOTE_WORK + output_release_work(domain, *op, args);
            let cost = domain.operation_work(args);
            let refusal = (prefix, cost);
            prefix += cost;
            refusal
        })
        .collect()
}

#[test]
fn caller_copy_fees_cover_constants_only_and_saturate_before_actions() {
    let mut config = ExecutionConfig::default();
    config.analysis.domain_profile = Profile::ConstantsOnly;
    config.analysis.relations.enabled = false;
    let domain = config.domain().unwrap();
    let (left, right) = (word(1), word(2));
    let mut budget = WorkBudget::new(
        QUOTE_WORK + copy_work(left.work_size()) + copy_work(right.work_size()) - 1,
    );
    assert_eq!(domain.operation_work(&[left.clone(), right.clone()]), 1);
    let mut builder = Builder {
        domain,
        budget: &mut budget,
        retention: GuardRetention::new(&left, &right),
    };
    assert!(builder.copied(opcode::GT, [&left, &right]).is_err());
    assert_eq!(budget.used(), QUOTE_WORK);
    assert!(budget.exhausted());
    assert_eq!(copy_work(usize::MAX), usize::MAX);
    assert_eq!(release_work(usize::MAX), usize::MAX);

    // A small shared DAG has an enormous cached expanded-node count. The
    // saturated lifetime fee must refuse before cloning or calling Domain.
    let limits = ExprLimits {
        max_nodes: usize::MAX,
        max_depth: usize::MAX,
    };
    let mut expression = ExprId::input(71, Symbol::CallValue);
    for _ in 0..usize::BITS - 2 {
        expression =
            ExprId::operation(opcode::ADD, &[expression.clone(), expression], limits).unwrap();
    }
    let offset = AbstractValue::top().with_expression(expression);
    let mut budget = WorkBudget::new(usize::MAX);
    let config = ExecutionConfig::default();
    assert!(build(&offset, &word(1), &mut context(&config, &mut budget)).is_err());
    assert_eq!(budget.used(), QUOTE_WORK * 2);
    assert!(budget.exhausted());
}

#[test]
fn each_of_eight_operation_refusals_keeps_paid_prefix_payload_and_prior_failure() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut config = ExecutionConfig::default();
    // Step 7 loses its expression; refusing step 8 must still report plain Work.
    config.analysis.relations.max_nodes = 10;
    let domain = config.domain().unwrap();
    let (offset, size) = inputs();
    let payload = initial(&world, &entry, domain).unwrap();
    let mut covered = empty_execution(payload.clone());
    let mut full = WorkBudget::new(usize::MAX);
    assert!(super::super::logical::failure(
        &mut covered,
        &mut context(&config, &mut full),
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
                &mut context(&config, &mut budget),
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
    let mut config = ExecutionConfig::default();
    config.analysis.relations.max_nodes = 10;
    let domain = config.domain().unwrap();
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
        &mut context(&config, &mut budget),
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
    let mut config = ExecutionConfig::default();
    config.analysis.relations.max_nodes = 1;
    let leaf = ExprId::input(71, Symbol::CallValue);
    let expression = ExprId::operation(opcode::NOT, &[leaf], Default::default()).unwrap();
    let offset = AbstractValue::top().with_expression(expression);
    let size = word(0);
    let mut budget = WorkBudget::new(config.max_work);
    let built = build(&offset, &size, &mut context(&config, &mut budget)).unwrap();
    assert_eq!(built.retention.cause(), Some(RetentionCause::Expression));
    assert_eq!(built.condition.singleton(), Some(U256::ONE));
    // allow_memory bypasses literal admission because the offset root exceeds
    // the cap, then keeps the original proven-constant success path.
    let mut result = empty_execution(initial(&world, &entry, Domain::default()).unwrap());
    assert!(super::super::allow_memory(
        &mut result,
        &offset,
        &size,
        &mut context(&config, &mut budget),
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
    let payload = initial(&world, &entry, config.domain().unwrap()).unwrap();
    let mut result = empty_execution(payload.clone());
    let mut budget = WorkBudget::new(736);
    assert!(super::super::allow_memory(
        &mut result,
        &AbstractValue::top(),
        &word(0),
        &mut context(&config, &mut budget),
        19,
    ));
    assert_eq!(budget.used(), 736);
    assert!(!budget.exhausted());
    assert_eq!(result.payload, payload);
    assert!(result.frontiers.is_empty());

    let cancellation = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&cancellation, || {
        let mut result = empty_execution(payload.clone());
        let mut budget = WorkBudget::new(config.max_work);
        assert!(super::super::logical::failure(
            &mut result,
            &mut context(&config, &mut budget),
            19
        ));
        let paid = budget.used();
        cancellation.cancel();
        let (offset, size) = inputs();
        assert!(!super::super::allow_memory(
            &mut result,
            &offset,
            &size,
            &mut context(&config, &mut budget),
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
            let mut builder = Builder {
                domain: Domain::default(),
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
                    + copy_work(original.work_size() + 1)
                    + literal_memory_singleton_work(&original)
            );
            assert!(!budget.exhausted());
        }
    }
}

#[test]
fn identification_short_circuits_without_calling_the_allocator() {
    for (original, enabled) in [
        (AbstractValue::top(), false),
        (inputs().0.with_symbolic_limit(), true),
        (word(7), true),
        (
            AbstractValue::unsigned_range(U256::from(7), U256::from(7)).unwrap(),
            true,
        ),
        (
            word(7).with_expression(ExprId::input(83, Symbol::Timestamp)),
            true,
        ),
    ] {
        let (actual, identity_failed) = super::identify(original.clone(), enabled, || {
            panic!("identification must not allocate on a short-circuited path")
        });
        assert_same_value(&actual, &original);
        assert!(!identity_failed);
    }
}

#[test]
fn unmarked_offset_identity_failure_vetoes_the_real_finite_guard_error() {
    let mut config = ExecutionConfig::default();
    config.analysis.relations.max_nodes = 1;
    let domain = config.domain().unwrap();
    let offset = AbstractValue::top();
    let size = inputs().1;
    let steps = reference(&offset, &size, domain);
    assert_eq!(
        steps[0].2.symbolic_error,
        Some(ExprError::Limit { nodes: 2, depth: 2 })
    );
    assert!(
        steps[1..]
            .iter()
            .all(|step| step.2.symbolic_error.is_none())
    );

    let mut allocations = 0;
    let mut budget = WorkBudget::new(usize::MAX);
    let mut built =
        super::build_with_allocator(&offset, &size, &mut context(&config, &mut budget), || {
            allocations += 1;
            None
        })
        .unwrap();
    assert_eq!(allocations, 1);
    assert_same_value(&built.request.offset, &offset);
    assert!(!built.request.offset.symbolic_limit_reached());
    assert_same_value(&built.request.size, &size);
    assert_same_value(&built.condition, &steps[7].2.value);
    assert!(built.condition.singleton().is_none());
    assert!(built.condition.symbolic_expression().is_none());
    assert!(built.condition.may_be_zero() && built.condition.may_be_nonzero());
    let (prefix, cost) = *operation_refusals(&offset, &size, domain).last().unwrap();
    assert_eq!(
        budget.used(),
        prefix + cost + QUOTE_WORK + literal_memory_singleton_work(&built.condition)
    );
    assert!(!budget.exhausted());
    assert_eq!(built.retention.cause(), None);
    built.retention.record(None);
    built.retention.record(steps[0].2.symbolic_error.as_ref());
    assert_eq!(built.retention.cause(), None);

    let mut previously_eligible = GuardRetention::new(&offset, &size);
    previously_eligible.record(steps[0].2.symbolic_error.as_ref());
    assert_eq!(
        previously_eligible.cause(),
        Some(RetentionCause::Expression)
    );
    previously_eligible.veto_identity_failure();
    previously_eligible.record(steps[0].2.symbolic_error.as_ref());
    assert_eq!(previously_eligible.cause(), None);

    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut result = empty_execution(initial(&world, &entry, domain).unwrap());
    let before = result.payload.clone();
    assert!(
        super::super::partition(
            &mut result,
            &built.request,
            &built.condition,
            Some(&built.retention),
            &mut context(&config, &mut budget),
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

#[test]
fn unmarked_size_identity_failure_also_vetoes_finite_guard_evidence() {
    let mut config = ExecutionConfig::default();
    config.analysis.relations.max_nodes = 1;
    let offset = inputs().0;
    let size = AbstractValue::top();
    let steps = reference(&offset, &size, config.domain().unwrap());
    assert_eq!(
        steps[1].2.symbolic_error,
        Some(ExprError::Limit { nodes: 3, depth: 2 })
    );
    let mut allocations = 0;
    let mut budget = WorkBudget::new(usize::MAX);
    let built =
        super::build_with_allocator(&offset, &size, &mut context(&config, &mut budget), || {
            allocations += 1;
            None
        })
        .unwrap();
    assert_eq!(allocations, 1);
    assert_same_value(&built.request.offset, &offset);
    assert_same_value(&built.request.size, &size);
    assert!(!built.request.size.symbolic_limit_reached());
    assert_same_value(&built.condition, &steps[7].2.value);
    assert_eq!(built.retention.cause(), None);
    assert!(!budget.exhausted());
}
