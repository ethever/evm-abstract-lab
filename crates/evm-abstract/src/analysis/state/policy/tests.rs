//! Whole inputs, retained metadata and independently instantiated guarantees.

#[cfg(test)]
use crate::analysis::testing::{ByteArray, TestValue};
use crate::domain::EvmNumericAlgebra as _;

use super::{EvmPayloadSummary, ProcedureOutput};
use crate::{
    analysis::{
        engine::policy::{
            Acceleration, CoarseSummaries, ForgetJumpHistory, MachinePartition, PolicyContext,
            PolicySite, ProcedureSummaryPolicy, ProjectedSummaries, StructuralPartition,
            WholeStateMetadata,
        },
        machine::{EvmChildFrame, EvmContinuation, MachineError, fixtures},
        transfer,
    },
    domain::{
        ProductAlgebra, ValueAlgebra,
        relational::{QueryReason, RelationLimits},
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use std::collections::BTreeSet;

fn range(upper: u64) -> TestValue {
    TestValue::unsigned_range(U256::ZERO, U256::from(upper)).unwrap()
}

#[test]
fn process_private_data_cannot_be_reinterpreted_as_native_input() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (world, entry) = fixtures::fixture(&[(0x101, "600160005500")]);
    let initial = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let mut budget = WorkBudget::new(20_000_000);
    let process = super::payload::caller::form_with_runtime(
        &initial,
        &entry,
        algebra.borrowed(),
        &mut budget,
        true,
    )
    .unwrap();
    assert_eq!(
        process.semantics(),
        crate::analysis::TransferSemantics::KnownProcess
    );
    let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
    assert!(matches!(
        process.instantiate(&mut cx),
        Err(MachineError::Process(
            crate::analysis::ProcessBoundary::NativeInstantiation
        ))
    ));
    let native = EvmPayloadSummary::project(&initial, &mut cx).unwrap();
    assert_eq!(
        native.semantics(),
        crate::analysis::TransferSemantics::Native
    );
    assert!(native.instantiate(&mut cx).is_ok());
    assert!(matches!(
        process.regional_join(&native, Acceleration::Join, &mut cx),
        Err(MachineError::Shape)
    ));
}

#[test]
fn scheduled_join_keeps_finite_bounds_before_and_after_promotion() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut initial = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    initial.active_mut().stack = vec![range(4)];
    let mut incoming = initial.clone();
    incoming.active_mut().stack[0] = range(5);
    let mut budget = WorkBudget::new(1_000_000_000);
    let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
    let mut metadata = WholeStateMetadata::initialize(&initial, &mut cx).unwrap();
    let exact = metadata
        .admit_correlated(
            &initial,
            &incoming,
            Acceleration::Join,
            &BTreeSet::new(),
            &mut cx,
        )
        .unwrap()
        .unwrap();
    assert!(!metadata.has_bound());
    assert!(exact.active().stack[0].contains(U256::from(5)));
    assert!(!exact.active().stack[0].contains(U256::from(6)));

    let bounded = metadata
        .admit_bounded(&exact, &incoming, Acceleration::Join, &mut cx)
        .unwrap()
        .unwrap();
    assert!(metadata.has_bound());
    assert!(!bounded.active().stack[0].contains(U256::from(6)));
    incoming.active_mut().stack[0] = range(6);
    let grown = metadata
        .admit_bounded(&bounded, &incoming, Acceleration::Join, &mut cx)
        .unwrap()
        .unwrap();
    assert!(grown.active().stack[0].contains(U256::from(6)));
    assert!(!grown.active().stack[0].contains(U256::from(7)));

    let work = metadata.work_size();
    assert!(
        metadata
            .admit_bounded(&grown, &incoming, Acceleration::Join, &mut cx)
            .unwrap()
            .is_none()
    );
    assert_eq!(metadata.work_size(), work);
    assert!(metadata.has_bound());
    incoming.active_mut().stack[0] = range(7);
    let widened = metadata
        .admit_bounded(&grown, &incoming, Acceleration::Widen, &mut cx)
        .unwrap()
        .unwrap();
    assert!(widened.active().stack[0].contains(U256::from(7)));
    assert!(widened.active().stack[0].contains(U256::ONE << 128usize));
    assert!(!grown.active().stack[0].contains(U256::ONE << 128usize));
}

#[test]
fn capacity_promotion_covers_without_numeric_widening_and_is_a_change() {
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(
        ProductAlgebra::default()
            .spec()
            .with_relations(RelationLimits {
                max_constraints: 1,
                ..ProductAlgebra::default().spec().relations()
            }),
    ));
    let domain = domain.borrowed();
    let algebra = domain;
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut initial = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    initial.active_mut().stack = vec![range(4)];
    let mut incoming = initial.clone();
    incoming.active_mut().stack[0] = range(5);
    let mut budget = WorkBudget::new(1_000_000_000);
    let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
    let mut metadata = WholeStateMetadata::initialize(&initial, &mut cx).unwrap();
    let promoted = metadata
        .admit_correlated(
            &initial,
            &incoming,
            Acceleration::Join,
            &BTreeSet::new(),
            &mut cx,
        )
        .unwrap()
        .unwrap();
    assert!(metadata.has_bound());
    assert!(promoted.active().stack[0].contains(U256::from(5)));
    assert!(!promoted.active().stack[0].contains(U256::from(6)));

    let mut same = WholeStateMetadata::initialize(&initial, &mut cx).unwrap();
    assert!(!same.has_bound());
    assert!(
        same.admit_bounded(&initial, &initial, Acceleration::Join, &mut cx)
            .unwrap()
            .is_some()
    );
    assert!(
        same.has_bound(),
        "a metadata-only promotion must be published"
    );
}

#[test]
fn history_preparation_erases_suspended_and_active_frames_before_keying() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut left = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    left.active_mut().key.jump_history = vec![1, 2];
    let mut child = left.active().clone();
    child.key.jump_history = vec![3];
    left.call_stack.push_child(EvmChildFrame {
        state: child,
        continuation: EvmContinuation {
            return_block: None,
            output_offset: fixtures::word(0),
            output_size: fixtures::word(0),
            creation: None,
        },
    });
    let mut right = left.clone();
    for frame in right.call_stack.iter_mut() {
        frame.key.jump_history = vec![4, 5];
    }
    let before = left.clone();
    let mut budget = WorkBudget::new(1_000_000_000);
    let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
    let structural_left = StructuralPartition
        .prepare(&left, PolicySite::World, (), &mut cx)
        .unwrap();
    let structural_right = StructuralPartition
        .prepare(&right, PolicySite::World, (), &mut cx)
        .unwrap();
    assert_ne!(structural_left.key, structural_right.key);
    let prepared_left = ForgetJumpHistory
        .prepare(&left, PolicySite::World, (), &mut cx)
        .unwrap();
    let prepared_right = ForgetJumpHistory
        .prepare(&right, PolicySite::Body, (), &mut cx)
        .unwrap();
    assert_eq!(prepared_left.key, prepared_right.key);
    assert_eq!(prepared_left.payload, prepared_right.payload);
    assert_eq!(left, before);
    for frame in prepared_left.payload.call_stack.iter() {
        assert!(frame.key.jump_history.is_empty());
    }
    let relative_left = EvmPayloadSummary::project(&left, &mut cx).unwrap();
    let relative_right = EvmPayloadSummary::project(&right, &mut cx).unwrap();
    let prepared_left = ForgetJumpHistory
        .prepare_relative(&relative_left, PolicySite::Procedure, (), &mut cx)
        .unwrap();
    let prepared_right = ForgetJumpHistory
        .prepare_relative(&relative_right, PolicySite::Subscriber, (), &mut cx)
        .unwrap();
    assert_eq!(prepared_left.key, prepared_right.key);
    assert!(
        prepared_left
            .payload
            .key()
            .frames
            .iter()
            .all(|frame| frame.jump_history.is_empty())
    );
    assert!(
        prepared_left
            .payload
            .frames
            .iter()
            .all(|frame| frame.key.jump_history.is_empty())
    );
}

#[test]
fn coarse_admission_erases_aliases_observations_and_first_output_content() {
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let algebra = domain;
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let key = TestValue::top().with_expression(ExprId::fresh().unwrap());
    input.active_mut().stack = vec![key.clone(), key.clone()];
    let owner = input.active().key.address;
    input
        .store
        .write_checked(owner, &key, &fixtures::word(7), domain)
        .unwrap();
    let before = input.clone();
    let mut budget = WorkBudget::new(1_000_000_000);
    let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
    let projected = EvmPayloadSummary::project(&input, &mut cx).unwrap();
    assert!(projected.active().stack_aliases.same_class(0, 1));
    assert_ne!(projected.storage_observations, Default::default());
    let coarse = CoarseSummaries
        .admit_relative(
            &projected,
            &projected,
            PolicySite::Procedure,
            Acceleration::Join,
            &mut cx,
        )
        .unwrap();
    assert!(!coarse.active().stack_aliases.same_class(0, 1));
    assert_eq!(coarse.storage_observations, Default::default());
    let original = projected.instantiate(&mut cx).unwrap();
    let independent = coarse.instantiate(&mut cx).unwrap();
    assert_eq!(
        original.active().stack[0].expression(),
        original.active().stack[1].expression()
    );
    assert_ne!(
        independent.active().stack[0].expression(),
        independent.active().stack[1].expression()
    );
    assert!(!projected.semantic_eq(&coarse, &mut cx).unwrap());
    assert_eq!(input, before);

    let output = ProcedureOutput::project(&ByteArray::exact(&[7]), &input.store, &mut cx).unwrap();
    let retained = ProjectedSummaries
        .admit_output(None, &output, Acceleration::Join, &mut cx)
        .unwrap();
    let coarsened = CoarseSummaries
        .admit_output(None, &output, Acceleration::Join, &mut cx)
        .unwrap();
    let retained_data = retained.data().instantiate(cx.domain).unwrap();
    let coarse_data = coarsened.data().instantiate(cx.domain).unwrap();
    assert_eq!(
        retained_data.byte_at(0, domain).singleton(),
        Some(U256::from(7))
    );
    assert!(coarse_data.byte_at(0, domain).contains(U256::from(255)));
    assert_eq!(coarsened.data().len().singleton(), Some(U256::ONE));
    assert_ne!(retained, coarsened);
}

#[test]
fn helpers_refuse_before_mutation_when_work_or_retained_cancellation_stops_them() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let before = input.clone();
    let mut budget = WorkBudget::new(0);
    let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
    assert!(matches!(
        input.copy_for_policy(&mut cx),
        Err(MachineError::Work)
    ));
    assert_eq!(cx.budget.used(), 0);
    assert_eq!(input, before);
    let token = analysis_control::Cancellation::new();
    let mut budget = analysis_control::with_cancellation(&token, || WorkBudget::new(1_000_000));
    token.cancel();
    let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
    assert!(matches!(
        EvmPayloadSummary::project(&input, &mut cx),
        Err(MachineError::Relations(QueryReason::Cancelled)),
    ));
    assert_eq!(cx.budget.used(), 0);
    assert_eq!(input, before);
}
