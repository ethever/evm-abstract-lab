//! Facts must be imported while their retained source identities are available.

use super::*;
use crate::{
    analysis::{
        engine::policy::{Acceleration, PolicyContext, RecentDistinctJumps, WholeStateMetadata},
        testing::{Entry, World},
    },
    domain::{
        DomainSpec, NumericValue, ValueAlgebra,
        identity::Symbol,
        relational::{EvmScalarQuery, QueryReason},
        symbolic::ExprKind,
    },
};
use revm_bytecode::opcode;
use std::num::NonZeroUsize;

fn guarded_input() -> (
    World,
    Entry,
    EvmMachinePayload<NumericValue>,
    ValueAlgebra<ProductAlgebra>,
) {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let domain = algebra.borrowed();
    // CALLVALUE; ISZERO; PUSH1 7; JUMPI; STOP; STOP; JUMPDEST; STOP.
    let (world, mut entry) = fixtures::fixture(&[(0x101, "341560075700005b00")]);
    entry.environment.value = TestValue::top();
    let initial = transfer::initial(&world, &entry, domain).unwrap();
    let execution = transfer::execute(
        &world,
        &entry,
        initial,
        &fixtures::config(),
        domain,
        &RecentDistinctJumps,
        &mut WorkBudget::new(20_000_000),
        transfer::StorageReadPolicy::Abstract,
    );
    assert!(execution.frontiers.is_empty());
    assert_eq!(execution.successors.len(), 2);
    let guarded = execution
        .successors
        .into_iter()
        .find(|successor| {
            let frame = successor.payload.active();
            frame.program.as_ref().unwrap().blocks()[frame.key.basic_block_index].start_pc == 7
        })
        .unwrap()
        .payload;
    let value = &guarded.active().call_value;
    assert_eq!(value.interval().unsigned_bounds(), (U256::ZERO, U256::MAX));
    assert!(
        matches!(value.expression().unwrap().kind(), ExprKind::Input(atom) if atom.symbol == Symbol::CallValue)
    );
    assert!(
        guarded
            .relations
            .has_direct_numeric_guarantee(value.expression().unwrap())
    );
    (world, entry, guarded, algebra)
}

#[test]
fn ordinary_admission_imports_the_nonpayable_guard_without_fixing_other_booleans() {
    let (_, entry, mut old, algebra) = guarded_input();
    let domain = algebra.borrowed();
    old.active_mut().stack = vec![
        TestValue::unsigned_range(U256::ZERO, U256::ONE)
            .unwrap()
            .with_expression(ExprId::fresh().unwrap()),
    ];
    old.normalize();
    let mut incoming = old.clone();
    incoming.active_mut().stack[0] = old.active().stack[0]
        .clone()
        .with_expression(ExprId::fresh().unwrap());
    let originals = [old.clone(), incoming.clone()];
    let mut budget = WorkBudget::new(20_000_000);
    let mut cx = PolicyContext::new(domain, &entry, &mut budget);
    let mut metadata = WholeStateMetadata::initialize(&old, &mut cx).unwrap();
    let admitted = metadata
        .admit_bounded(&old, &incoming, Acceleration::Join, &mut cx)
        .unwrap()
        .unwrap();
    assert!(metadata.has_bound());
    assert_eq!(admitted.active().call_value.singleton(), Some(U256::ZERO));
    assert!(admitted.relations.is_empty());
    assert!(admitted.active().stack[0].contains(U256::ZERO));
    assert!(admitted.active().stack[0].contains(U256::ONE));
    assert!(
        metadata
            .admit_bounded(&admitted, &admitted, Acceleration::Join, &mut cx)
            .unwrap()
            .is_none()
    );
    assert_eq!([old, incoming], originals);
}

#[test]
fn historical_input_scope_cannot_constrain_a_new_call_value() {
    let (_, entry, mut input, algebra) = guarded_input();
    let domain = algebra.borrowed();
    let ExprKind::Input(atom) = input.active().call_value.expression().unwrap().kind() else {
        unreachable!();
    };
    let replacement = ExprId::input(atom.scope.checked_add(1).unwrap(), Symbol::CallValue);
    input.active_mut().call_value = TestValue::top().with_expression(replacement);
    let mut budget = WorkBudget::new(20_000_000);
    let (_, admitted) =
        LoopBound::new_scheduled(&input, &input, &entry, domain, &mut budget, false).unwrap();
    assert_eq!(
        admitted.active().call_value.interval().unsigned_bounds(),
        (U256::ZERO, U256::MAX)
    );
    assert!(admitted.active().call_value.contains(U256::ONE));
    assert!(admitted.relations.is_empty());
}

#[test]
fn ordinary_join_covers_both_truth_branches_of_the_same_input() {
    let (_, entry, old, algebra) = guarded_input();
    let domain = algebra.borrowed();
    let mut incoming = old.clone();
    incoming.relations = Default::default();
    let guard = ExprId::operation(
        opcode::ISZERO,
        &[old.active().call_value.expression().unwrap().clone()],
        domain.spec().relations().expressions(),
    )
    .unwrap();
    incoming
        .relations
        .assume_checked(&guard, false, &domain.spec().relations(), domain)
        .unwrap();
    let mut budget = WorkBudget::new(20_000_000);
    let (_, admitted) =
        LoopBound::new_scheduled(&old, &incoming, &entry, domain, &mut budget, false).unwrap();
    for value in [U256::ZERO, U256::ONE, U256::MAX] {
        assert!(admitted.active().call_value.contains(value));
    }
    assert!(admitted.relations.is_empty());
}

#[test]
fn saved_read_facts_do_not_refine_the_replacement_current_store() {
    let (_, entry, mut input, algebra) = guarded_input();
    let domain = algebra.borrowed();
    let owner = input.active().key.address;
    let key = fixtures::word(0);
    let original = TestValue::top().with_expression(ExprId::fresh().unwrap());
    input
        .store
        .write_checked(owner, &key, &original, domain)
        .unwrap();
    input.active_mut().saved_store = input.store.snapshot();
    let old_read = input.store.read_checked(owner, &key, domain).unwrap();
    let guard = ExprId::operation(
        opcode::ISZERO,
        &[old_read.expression().unwrap().clone()],
        domain.spec().relations().expressions(),
    )
    .unwrap();
    input
        .relations
        .assume_checked(&guard, true, &domain.spec().relations(), domain)
        .unwrap();
    let replacement = TestValue::top().with_expression(ExprId::fresh().unwrap());
    input
        .store
        .write_checked(owner, &key, &replacement, domain)
        .unwrap();
    let current_read = input.store.read_checked(owner, &key, domain).unwrap();
    assert_ne!(old_read.expression(), current_read.expression());
    input.active_mut().stack = vec![old_read, current_read];
    input.normalize();
    let original = input.clone();
    let mut budget = WorkBudget::new(20_000_000);
    let (_, admitted) =
        LoopBound::new_scheduled(&input, &input, &entry, domain, &mut budget, false).unwrap();
    assert_eq!(admitted.active().stack[0].singleton(), Some(U256::ZERO));
    assert!(admitted.active().stack[1].contains(U256::ONE));
    assert_eq!(
        admitted
            .active()
            .saved_store
            .state()
            .read(owner, &key, domain)
            .singleton(),
        Some(U256::ZERO)
    );
    assert!(admitted.store.read(owner, &key, domain).contains(U256::ONE));
    assert_ne!(
        admitted.store.slot_root(owner, false).unwrap(),
        admitted
            .active()
            .saved_store
            .state()
            .slot_root(owner, false)
            .unwrap()
    );
    assert_eq!(input, original);
}

#[test]
fn optional_import_neither_prunes_infeasible_metadata_nor_fabricates_unknown_bounds() {
    let (_, entry, input, algebra) = guarded_input();
    let domain = algebra.borrowed();
    let mut contradictory = input.clone();
    contradictory.active_mut().call_value = TestValue::unsigned_range(U256::ONE, U256::from(2))
        .unwrap()
        .with_expression(input.active().call_value.expression().unwrap().clone());
    let mut budget = WorkBudget::new(20_000_000);
    let summary = project(&contradictory, &entry, domain, &mut budget).unwrap();
    assert_eq!(
        summary.active().call_value.interval().unsigned_bounds(),
        (U256::ONE, U256::from(2))
    );

    // A direct ordered bound requires more scalar facts than this tiny carrier
    // permits. Unknown remains the original upper bound, never an empty input.
    let tiny = ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::new(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    )));
    let tiny = tiny.borrowed();
    let mut unknown = input.clone();
    unknown.relations = Default::default();
    let expression = unknown.active().call_value.expression().unwrap().clone();
    let guard = ExprId::operation(
        opcode::LT,
        &[expression.clone(), ExprId::constant(U256::from(10))],
        tiny.spec().relations().expressions(),
    )
    .unwrap();
    unknown
        .relations
        .assume_checked(&guard, true, &domain.spec().relations(), domain)
        .unwrap();
    assert_eq!(
        unknown.relations.refine_numeric(
            &expression,
            unknown.active().call_value.numeric(),
            tiny,
            &tiny.spec().relations()
        ),
        EvmScalarQuery::Unknown(QueryReason::ScalarFactLimit)
    );
    let summary = project(&unknown, &entry, tiny, &mut budget).unwrap();
    assert_eq!(
        summary.active().call_value.interval().unsigned_bounds(),
        (U256::ZERO, U256::MAX)
    );
}

#[test]
fn refused_or_cancelled_import_does_not_install_a_bound_or_mutate_its_inputs() {
    let (_, entry, input, algebra) = guarded_input();
    let domain = algebra.borrowed();
    let before = input.clone();
    let mut setup = WorkBudget::new(20_000_000);
    let metadata =
        WholeStateMetadata::initialize(&input, &mut PolicyContext::new(domain, &entry, &mut setup))
            .unwrap();
    let mut budget = WorkBudget::new(0);
    let mut refused = metadata.clone();
    assert_eq!(
        refused
            .admit_bounded(
                &input,
                &input,
                Acceleration::Join,
                &mut PolicyContext::new(domain, &entry, &mut budget)
            )
            .err(),
        Some(MachineError::Work)
    );
    assert!(!refused.has_bound());
    let cancellation = analysis_control::Cancellation::new();
    let mut cancelled =
        analysis_control::with_cancellation(&cancellation, || WorkBudget::new(20_000_000));
    cancellation.cancel();
    let mut refused = metadata;
    assert_eq!(
        refused
            .admit_bounded(
                &input,
                &input,
                Acceleration::Join,
                &mut PolicyContext::new(domain, &entry, &mut cancelled)
            )
            .err(),
        Some(MachineError::Relations(QueryReason::Cancelled))
    );
    assert!(!refused.has_bound());
    assert_eq!(input, before);
}

#[test]
fn import_refusal_after_the_paid_copy_leaves_the_original_source_unchanged() {
    let (_, entry, input, algebra) = guarded_input();
    let domain = algebra.borrowed();
    let before = input.clone();
    let mut copy_budget = WorkBudget::new(20_000_000);
    input
        .copy_for_policy(&mut PolicyContext::new(domain, &entry, &mut copy_budget))
        .unwrap();
    let mut budget = WorkBudget::new(copy_budget.used());
    assert_eq!(
        project(&input, &entry, domain, &mut budget).err(),
        Some(MachineError::Work)
    );
    assert!(budget.exhausted());
    assert_eq!(budget.used(), copy_budget.used());
    assert_eq!(input, before);
}

#[test]
fn a_later_proved_scalar_cannot_refine_the_persistent_coarse_invariant() {
    let (_, entry, guarded, algebra) = guarded_input();
    let domain = algebra.borrowed();
    let mut unguarded = guarded.clone();
    unguarded.relations = Default::default();
    let mut budget = WorkBudget::new(20_000_000);
    let summary = project(&unguarded, &entry, domain, &mut budget).unwrap();
    let representation = EvmBoundedState::freeze(&summary, true, domain).unwrap();
    let (representation, bound, _) = representation
        .materialize(summary, domain, &mut budget)
        .unwrap();
    let bound = LoopBound {
        representation,
        bound,
    };
    for _ in 0..2 {
        assert!(
            bound
                .update_scheduled(&guarded, &entry, domain, &mut budget, false)
                .unwrap()
                .is_none()
        );
        assert!(bound.bound.active().call_value.contains(U256::ONE));
    }
}

#[test]
fn source_copy_refuses_unpaid_native_predicates_and_retains_the_ledger_cancellation() {
    use crate::{
        domain::{EvmValue, WordElement},
        resource::clone_payment::tests::{RECEIPT, private_value, reset},
    };
    let _scope = RECEIPT.scope();
    let (world, entry, _, algebra) = crate::resource::clone_payment::tests::fixture("00");
    let domain = algebra.borrowed();
    let mut input = transfer::initial(&world, &entry, domain).unwrap();
    let expression = ExprId::fresh().unwrap();
    input.active_mut().call_value = private_value(usize::MAX).with_expression(expression.clone());
    input
        .relations
        .observe(
            &expression,
            &NumericValue::unsigned_range(U256::from(5), U256::from(15)).unwrap(),
            &domain.spec().relations(),
        )
        .unwrap();
    let original = input.clone();
    let mut budget = WorkBudget::new(20_000_000);
    reset();
    assert_eq!(
        project(&input, &entry, domain, &mut budget).err(),
        Some(MachineError::Work)
    );
    assert_eq!(RECEIPT.get().clones, 0);
    assert!(RECEIPT.get().private_quotes > 0);
    assert_eq!(input, original);

    let cancellation = analysis_control::Cancellation::new();
    let mut numeric = input.active().call_value.numeric().clone();
    numeric.declared_clone = 64;
    numeric.cancel_price = Some(cancellation.clone());
    assert_eq!(numeric.clone_work(), 64);
    input.active_mut().call_value = EvmValue::from_numeric(numeric).with_expression(expression);
    let mut budget =
        analysis_control::with_cancellation(&cancellation, || WorkBudget::new(20_000_000));
    reset();
    RECEIPT.update(|receipt| receipt.armed = true);
    assert_eq!(
        project(&input, &entry, domain, &mut budget).err(),
        Some(MachineError::Relations(QueryReason::Cancelled))
    );
    assert!(cancellation.is_cancelled());
    assert_eq!(RECEIPT.get().clones, 0);
}
