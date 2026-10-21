//! Opaque carrier and receipts independent of the default product representation.

use super::*;
use crate::{
    Fork,
    analysis::{EnginePolicies, ExecutionConfig, Status, analyze_world_with, transfer::relations},
    domain::{EvmValue, ValueAlgebra, symbolic::ExprId},
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use alloy_primitives::Address;

mod carrier;
pub(in crate::analysis::transfer) use carrier::{LITERALS, Policy, RECEIPT, Word};

pub(in crate::analysis::transfer) use crate::analysis::transfer::testing::context;

#[test]
fn independent_singleton_query_pays_once_and_refuses_before_execution() {
    use proptest::{prelude::*, test_runner::TestRunner};
    let check = |word: U256, fee: usize, prior: usize| {
        let _receipt = RECEIPT.scope();
        RECEIPT.update(|receipt| {
            receipt.watched = Some(word);
            receipt.fee = fee;
        });
        let value = Word::constant(word);
        let mut paid = WorkBudget::new(prior + fee);
        prop_assert!(paid.charge(prior));
        prop_assert_eq!(singleton(&value, &mut paid), Ok(Some(word)));
        prop_assert_eq!(
            (RECEIPT.get().quotes, RECEIPT.get().queries, paid.used()),
            (1, 1, prior + fee)
        );
        let mut short = WorkBudget::new(prior + fee - 1);
        prop_assert!(short.charge(prior));
        prop_assert_eq!(singleton(&value, &mut short), Err(FrontierReason::Work));
        prop_assert_eq!(
            (RECEIPT.get().quotes, RECEIPT.get().queries, short.used()),
            (2, 1, prior)
        );
        RECEIPT.update(|receipt| receipt.fee = usize::MAX);
        let mut unlimited = WorkBudget::new(usize::MAX);
        prop_assert!(unlimited.charge(prior));
        prop_assert_eq!(singleton(&value, &mut unlimited), Err(FrontierReason::Work));
        prop_assert_eq!(
            (
                RECEIPT.get().quotes,
                RECEIPT.get().queries,
                unlimited.used()
            ),
            (3, 1, prior)
        );
        Ok(())
    };
    // Keep the original 7 / 61 / 60 witness, and exercise real ledger prefixes
    // and a finite quote adjacent to the refusal sentinel before generation.
    for (word, fee, prior) in [
        (U256::from(7), 61, 0),
        (U256::ZERO, usize::MAX - 1, 0),
        (U256::MAX, 1, 17),
    ] {
        check(word, fee, prior).unwrap();
    }
    TestRunner::default()
        .run(
            &(any::<[u64; 4]>(), 1_usize..1_000_001, 0_usize..1025),
            |(limbs, fee, prior)| check(U256::from_limbs(limbs), fee, prior),
        )
        .unwrap();
}

#[test]
fn singleton_query_observes_both_current_and_captured_cancellation() {
    let _receipt = RECEIPT.scope();
    let word = Word::constant(U256::from(7));
    let token = analysis_control::Cancellation::new();
    let mut captured = analysis_control::with_cancellation(&token, || WorkBudget::new(1000));
    token.cancel();
    assert_eq!(
        singleton(&word, &mut captured),
        Err(FrontierReason::Relations(QueryReason::Cancelled))
    );
    let mut outside = WorkBudget::new(1000);
    analysis_control::with_cancellation(&token, || {
        assert_eq!(
            singleton(&word, &mut outside),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
    });
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    let during_quote = analysis_control::Cancellation::new();
    RECEIPT.update(|receipt| receipt.cancel_on_quote = true);
    analysis_control::with_cancellation(&during_quote, || {
        assert_eq!(
            singleton(&word, &mut outside),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
    });
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, outside.used()),
        (1, 0, 0)
    );
}

#[test]
fn cancellation_during_singleton_keeps_payment_but_discards_the_answer() {
    let _receipt = RECEIPT.scope();
    for cancel_ledger in [false, true] {
        let owner = analysis_control::Cancellation::new();
        let active = analysis_control::Cancellation::new();
        let mut budget = analysis_control::with_cancellation(&owner, || WorkBudget::new(1000));
        let word = Word {
            cancel_query_owner: cancel_ledger.then(|| owner.clone()),
            ..Word::constant(U256::from(7))
        };
        RECEIPT.update(|receipt| receipt.cancel_on_query = !cancel_ledger);
        analysis_control::with_cancellation(&active, || {
            assert_eq!(
                singleton(&word, &mut budget),
                Err(FrontierReason::Relations(QueryReason::Cancelled))
            );
        });
        assert_eq!(budget.used(), 61);
        assert_eq!(owner.is_cancelled(), cancel_ledger);
        assert_eq!(active.is_cancelled(), !cancel_ledger);
    }
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (2, 2));
}

#[test]
fn refinement_prepays_opaque_singletons_and_leaves_refused_values_intact() {
    let _receipt = RECEIPT.scope();
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let expression = ExprId::fresh().unwrap();
    let mut relations = crate::domain::relational::RelationState::default();
    relations.add_equal(
        &expression,
        &ExprId::constant(U256::from(7)),
        &config.analysis.relations,
    );
    let before = EvmValue::constant(U256::from(7)).with_expression(expression);
    let mut value = before.clone();
    let mut budget = WorkBudget::new(61);
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(true)
    );
    assert_eq!(value, before);
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, budget.used()),
        (1, 1, 61)
    );
    let mut refused = WorkBudget::new(60);
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut refused, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (2, 1));
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let mut refused = WorkBudget::new(usize::MAX);
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut refused, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, refused.used()),
        (3, 1, 0)
    );
    RECEIPT.update(|receipt| {
        receipt.fee = 61;
        receipt.cancel_on_query = true;
    });
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut budget = WorkBudget::new(1000);
        assert_eq!(
            relations::refine(
                &mut value,
                &relations,
                &mut context(&config, &mut budget, algebra.borrowed())
            ),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
        assert_eq!(budget.used(), 61);
    });
    assert_eq!(value, before);
}

fn scalar_fixture() -> (
    ExecutionConfig,
    EvmValue<Word>,
    crate::domain::relational::RelationState,
) {
    let config = ExecutionConfig::default();
    let expression = ExprId::fresh().unwrap();
    let before = EvmValue::from_numeric(Word {
        lower: U256::ONE,
        upper: U256::from(9),
        excluded: Some(U256::from(5)),
        ..Word::top()
    })
    .with_expression(expression.clone())
    .with_symbolic_limit();
    let mut relations = crate::domain::relational::RelationState::default();
    relations.add_equal(
        &expression,
        &ExprId::constant(U256::from(7)),
        &config.analysis.relations,
    );
    (config, before, relations)
}

#[test]
fn cooperative_refinement_skips_discovery_and_checks_stop_before_interpreting_hooks() {
    use crate::{
        analysis::{
            MachineError,
            cooperation::{LocalInput, RefinementError, StackFacts, apply_refinements},
        },
        domain::{ReductionStatus, WordObservation},
    };
    use analysis_control::{Cancellation, with_cancellation};

    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let (_, before, _) = scalar_fixture();
    let algebra = ValueAlgebra::new(Policy);
    let mut input = crate::analysis::transfer::initial(
        &EvmWorld::new(Fork::Osaka, "cooperative refinement"),
        &EvmEntry::new(Address::ZERO),
        algebra.borrowed(),
    )
    .unwrap();
    input.active_mut().stack.push(before.clone());
    let observation = WordObservation::constant(U256::from(7));
    let facts = StackFacts::at(0, observation.clone());
    let paid = algebra
        .borrowed()
        .refinement_work(&before, observation.work_size())
        + before.clone_work();
    let mut budget = WorkBudget::new(1_000_000);
    let LocalInput::Reachable { payload, limited } =
        apply_refinements(input.clone(), &facts, algebra.borrowed(), &mut budget).unwrap()
    else {
        panic!("a MAX standalone query quote cannot refuse an unused query")
    };
    assert!(limited.is_empty());
    let refined = &payload.active().stack[0];
    assert_eq!(
        (refined.numeric().lower, refined.numeric().upper),
        (U256::from(7), U256::from(7))
    );
    assert_eq!(refined.numeric().excluded, before.numeric().excluded);
    assert_eq!(refined.expression(), before.expression());
    assert!(refined.symbolic_limit_reached());
    assert!(budget.used() > paid);

    // charge(0) alone would accept this sticky exhausted ledger.
    let mut exhausted = WorkBudget::new(1_000_000);
    assert!(!exhausted.charge(1_000_001));
    assert!(matches!(
        apply_refinements(input.clone(), &facts, algebra.borrowed(), &mut exhausted),
        Err(RefinementError::Stopped(MachineError::Work))
    ));
    assert_eq!(exhausted.used(), 0);

    for captured in [false, true] {
        for (status, word) in [
            (ReductionStatus::Stable, 7),
            (ReductionStatus::Empty, 7),
            (ReductionStatus::Stable, 5),
        ] {
            let owner = Cancellation::new();
            let current = Cancellation::new();
            let mut budget = with_cancellation(&owner, || WorkBudget::new(1_000_000));
            assert!(budget.charge(17));
            let mut input = input.clone();
            input.active_mut().stack[0] = before.clone().with_numeric(Word {
                cancel_refinement_owner: captured.then_some(owner),
                ..before.numeric().clone()
            });
            RECEIPT.update(|receipt| {
                receipt.cancel_on_constrain = word == 5;
                receipt.cancel_on_reduce = word == 7;
                receipt.reduction_status = status;
            });
            let facts = StackFacts::at(0, WordObservation::constant(U256::from(word)));
            let result = with_cancellation(&current, || {
                apply_refinements(input, &facts, algebra.borrowed(), &mut budget)
            });
            assert!(matches!(
                result,
                Err(RefinementError::Stopped(MachineError::Relations(
                    QueryReason::Cancelled
                )))
            ));
            assert_eq!(
                budget.used(),
                17 + paid,
                "hook cancellation retains only the paid prefix"
            );
        }
    }
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
}

#[test]
fn refined_result_has_one_paid_query_and_preserves_owned_constraints() {
    let _receipt = RECEIPT.scope();
    let (config, before, relations) = scalar_fixture();
    let algebra = ValueAlgebra::new(Policy);
    let mut budget = WorkBudget::new(1_000_000);
    let mut value = before.clone();
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(true)
    );
    assert_eq!(
        (value.numeric().lower, value.numeric().upper),
        (U256::from(7), U256::from(7))
    );
    assert_eq!(value.numeric().excluded, Some(U256::from(5)));
    assert!(
        value.contains(U256::from(7))
            && !value.contains(U256::from(5))
            && !value.contains(U256::from(8))
    );
    assert_eq!(value.expression(), before.expression());
    assert!(!value.symbolic_limit_reached());
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));

    // The initial nonsingleton query and the scalar envelope have already been
    // paid. Refusal at the result query cannot install that detached import.
    let success_work = budget.used();
    let mut limited = WorkBudget::new(success_work - 1);
    let mut value = before.clone();
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut limited, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!(limited.used(), success_work - 61);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (2, 1));

    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let mut unlimited = WorkBudget::new(usize::MAX);
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut unlimited, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(value, before);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (3, 1));
}

#[test]
fn refinement_checks_cancellation_after_import_reduction_and_result_query() {
    let _receipt = RECEIPT.scope();
    let (config, before, relations) = scalar_fixture();
    let algebra = ValueAlgebra::new(Policy);
    for stage in 0..3 {
        RECEIPT.update(|receipt| {
            receipt.cancel_on_constrain = stage == 0;
            receipt.cancel_on_reduce = stage == 1;
            receipt.cancel_on_query = stage == 2;
        });
        let token = analysis_control::Cancellation::new();
        analysis_control::with_cancellation(&token, || {
            let mut budget = WorkBudget::new(1_000_000);
            let mut value = before.clone();
            assert_eq!(
                relations::refine(
                    &mut value,
                    &relations,
                    &mut context(&config, &mut budget, algebra.borrowed())
                ),
                Err(FrontierReason::Relations(QueryReason::Cancelled))
            );
            assert_eq!(value, before);
            assert!(budget.used() > 64);
        });
    }
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
}

#[test]
fn opaque_exclusion_can_refute_a_relational_equality() {
    let (config, before, relations) = scalar_fixture();
    let mut value = EvmValue::from_numeric(Word {
        excluded: Some(U256::from(7)),
        ..before.numeric().clone()
    })
    .with_expression(before.expression().unwrap().clone());
    let before = value.clone();
    let algebra = ValueAlgebra::new(Policy);
    let mut budget = WorkBudget::new(1_000_000);
    assert_eq!(
        relations::refine(
            &mut value,
            &relations,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(false)
    );
    assert_eq!(value, before);
    assert!(!value.contains(U256::from(7)));
}

#[test]
fn branch_discovery_pays_once_and_stops_before_returning_a_refused_successor() {
    use crate::analysis::transfer::{empty_execution, initial};

    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let world = EvmWorld::<Word>::new(Fork::Osaka, "query branch");
    let entry = EvmEntry::concrete(
        Address::ZERO,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let copy_work = input.work_size();
    let before_relations = input.relations.clone();
    let condition = EvmValue::constant(U256::from(7)).with_expression(ExprId::fresh().unwrap());
    let _receipt = RECEIPT.scope();
    let mut result = empty_execution(input.clone());
    let mut budget = WorkBudget::new(copy_work + 61);
    let successor = relations::branch(
        &mut result,
        &condition,
        true,
        &mut context(&config, &mut budget, algebra.borrowed()),
        17,
    )
    .unwrap();
    assert_eq!(successor.relations, before_relations);
    assert!(result.frontiers.is_empty());
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, budget.used()),
        (1, 1, copy_work + 61)
    );
    for maximum in [copy_work + 60, usize::MAX] {
        if maximum == usize::MAX {
            RECEIPT.update(|receipt| receipt.fee = usize::MAX);
        }
        let mut result = empty_execution(input.clone());
        let mut budget = WorkBudget::new(maximum);
        assert!(
            relations::branch(
                &mut result,
                &condition,
                true,
                &mut context(&config, &mut budget, algebra.borrowed()),
                17
            )
            .is_none()
        );
        assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
        assert_eq!(result.payload.relations, before_relations);
        assert_eq!(budget.used(), copy_work);
    }
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (3, 1));
    RECEIPT.update(|receipt| {
        receipt.fee = 61;
        receipt.cancel_on_query = true;
    });
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut result = empty_execution(input);
        let mut budget = WorkBudget::new(1_000_000);
        assert!(
            relations::branch(
                &mut result,
                &condition,
                true,
                &mut context(&config, &mut budget, algebra.borrowed()),
                17
            )
            .is_none()
        );
        assert_eq!(
            result.frontiers[0].reason,
            FrontierReason::Relations(QueryReason::Cancelled)
        );
    });
}

#[test]
fn complete_operation_envelope_owns_internal_queries_without_a_second_payment() {
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        receipt.watched = None;
        receipt.fee = usize::MAX;
    });
    let algebra = ValueAlgebra::new(Policy);
    let arg = EvmValue::<Word>::top()
        .with_expression(ExprId::fresh().unwrap())
        .with_symbolic_limit();
    let work = algebra.operation_work(std::slice::from_ref(&arg));
    let mut budget = WorkBudget::new(work);
    let result = algebra
        .apply_budgeted(revm_bytecode::opcode::ISZERO, &[arg], &mut budget)
        .unwrap();
    assert!(result.value.symbolic_limit_reached());
    assert_eq!(budget.used(), work);
    // One result discovery reused by both facade metadata decisions, plus one
    // operand discovery for expression construction, all inside the envelope.
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 2));
}

#[test]
fn storage_discovery_prepays_the_opaque_owner_query() {
    use crate::analysis::transfer::{StorageReadPolicy, execute, initial};

    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let owner_word = U256::from(0x7171);
    let owner = Address::from_word(owner_word.into());
    let mut world = EvmWorld::new(Fork::Osaka, "query discovery");
    world
        .insert(
            owner,
            EvmAccount::<Word>::from_hex("5f5400", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::concrete(
        owner,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let mut input = initial(&world, &entry, algebra.borrowed()).unwrap();
    input.active_mut().address_operand = Some(EvmValue::constant(owner_word));
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.watched = Some(owner_word));
    for refused in [true, false] {
        RECEIPT.update(|receipt| {
            receipt.fee = if refused { usize::MAX } else { 61 };
        });
        let mut budget = WorkBudget::new(10_000_000);
        let result = execute(
            &world,
            &entry,
            input.clone(),
            &config,
            algebra.borrowed(),
            &crate::analysis::RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Discover,
        );
        assert_eq!(
            result
                .frontiers
                .iter()
                .any(|frontier| frontier.reason == FrontierReason::Work),
            refused
        );
        assert_eq!(RECEIPT.get().quotes, if refused { 1 } else { 2 });
        assert_eq!(RECEIPT.get().queries, usize::from(!refused));
    }
}

#[test]
fn public_pop_probe_needs_no_singleton_discovery_even_with_a_max_quote() {
    let _receipt = RECEIPT.scope();
    let owner = Address::repeat_byte(0x71);
    let mut world = EvmWorld::new(Fork::Osaka, "singleton query receipt");
    world
        .insert(
            owner,
            EvmAccount::<Word>::from_hex("60075000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::concrete(
        owner,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let config = ExecutionConfig {
        max_work: 10_000_000,
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    };
    let algebra = ValueAlgebra::new(Policy);
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let skipped = analyze_world_with(
        world.clone(),
        entry.clone(),
        config.clone(),
        &algebra,
        &EnginePolicies::default(),
    )
    .unwrap();
    assert_eq!(skipped.status(), Status::Converged);
    assert!(skipped.frontiers().is_empty());
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    RECEIPT.update(|receipt| {
        receipt.fee = 61;
        receipt.quotes = 0;
        receipt.queries = 0;
    });
    let paid =
        analyze_world_with(world, entry, config, &algebra, &EnginePolicies::default()).unwrap();
    assert_eq!(paid.status(), Status::Converged);
    assert!(paid.frontiers().is_empty());
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    assert_eq!(skipped.work(), paid.work());
    assert!(
        paid.states()
            .iter()
            .any(|state| state.exit_stack.is_empty())
    );
}
