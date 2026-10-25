use super::*;
use crate::{
    Fork,
    analysis::{EvmChildFrame, EvmContinuation},
    domain::{ProductAlgebra, ValueAlgebra},
};
use std::cell::Cell;

fn nested(
    algebra: crate::domain::AlgebraRef<'_, ProductAlgebra>,
) -> (
    EvmEntry<crate::domain::NumericValue>,
    EvmMachinePayload<crate::domain::NumericValue>,
) {
    let (world, entry) = fixtures::fixture(&[(0x101, "00"), (0x202, "00")]);
    let mut input = transfer::initial(&world, &entry, algebra).unwrap();
    let alias = fixtures::word(17).with_expression(ExprId::fresh().unwrap());
    input.active_mut().stack = vec![alias.clone(), alias];
    input.active_mut().key.jump_history = vec![1, 2];
    let mut child = input.active().clone();
    child.key.code_address = fixtures::address(0x202);
    child.key.address = fixtures::address(0x303);
    child.key.address_value = fixtures::address(0x303).into();
    child.key.is_static = true;
    child.key.jump_history = vec![3, 4, 5];
    child.saved_store = input.store.snapshot();
    input.call_stack.push_child(EvmChildFrame {
        state: child,
        continuation: EvmContinuation {
            return_block: Some(7),
            output_offset: fixtures::word(13),
            output_size: fixtures::word(19),
            creation: None,
        },
    });
    (entry, input)
}

fn root_region(input: &EvmMachinePayload<crate::domain::NumericValue>) -> HybridRegion {
    let frame = &input.call_stack.root().state.key;
    HybridRegion {
        source: ProcessSource::Runtime {
            source: frame.code_address,
            hash: frame.code_hash,
            fork: Fork::Osaka,
        },
        code_address: frame.code_address,
        code_hash: frame.code_hash,
    }
}

#[test]
fn changed_owned_machine_matches_borrowed_and_moves_unmodified_data() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (entry, input) = nested(algebra.borrowed());
    let regions = [root_region(&input)];
    let policy = Selective {
        inner: &StructuralPartition,
        regions: &regions,
    };
    let mut budget = WorkBudget::new(20_000_000);
    let borrowed = policy
        .prepare(
            &input,
            PolicySite::World,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap();
    let pointer = input.active().stack.as_ptr();
    let owned = policy
        .prepare_owned(
            input,
            PolicySite::World,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap();
    assert_eq!(owned.key, borrowed.key);
    assert_eq!(owned.payload, borrowed.payload);
    assert_eq!(owned.payload.active().stack.as_ptr(), pointer);
    assert_eq!(
        owned.payload.call_stack.root().state.key.jump_history,
        [1, 2]
    );
    assert!(owned.payload.active().key.jump_history.is_empty());
}

#[test]
fn changed_owned_relative_preserves_independent_location_and_exact_work() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (entry, input) = nested(algebra.borrowed());
    let regions = [root_region(&input)];
    let policy = Selective {
        inner: &StructuralPartition,
        regions: &regions,
    };
    let mut budget = WorkBudget::new(20_000_000);
    let mut relative = RelativeState::project(
        &input,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
    )
    .unwrap();
    relative.location.frames[0].jump_history = vec![11];
    relative.location.frames[1].jump_history = vec![12, 13, 14, 15];
    let borrowed = policy
        .prepare_relative(
            &relative,
            PolicySite::Body,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap();
    let pointer = relative.frames[1].stack.as_ptr();
    let original_work = relative.work;
    let owned = policy
        .prepare_relative_owned(
            relative,
            PolicySite::Body,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap();
    assert_eq!(owned.key, borrowed.key);
    assert_eq!(owned.payload, borrowed.payload);
    assert_eq!(owned.payload.frames[1].stack.as_ptr(), pointer);
    assert_eq!(owned.payload.frames[0].key.jump_history, [1, 2]);
    assert_eq!(owned.payload.location.frames[0].jump_history, [11]);
    assert_eq!(owned.payload.work, original_work - 3);
    assert_eq!(owned.payload.work, owned.payload.measure_work());
}

// Uses the default owned adapter: a custom policy may stage and modify an
// arbitrary private candidate, then refuse while returning the untouched input.
struct StageThenRefuse;
impl MachinePartition<ProductAlgebra, ()> for StageThenRefuse {
    type Key = (crate::analysis::MachineKey, ());
    fn prepare(
        &self,
        input: &EvmMachinePayload<crate::domain::NumericValue>,
        _: PolicySite,
        _: (),
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<Partitioned<Self::Key, crate::domain::NumericValue>, MachineError> {
        let mut candidate = input.copy_for_policy(cx)?;
        candidate.active_mut().key.is_static = false;
        candidate.active_mut().stack.clear();
        Err(MachineError::Shape)
    }
    fn prepare_relative(
        &self,
        input: &RelativeState<crate::domain::NumericValue>,
        _: PolicySite,
        _: (),
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<PartitionedRelative<Self::Key, crate::domain::NumericValue>, MachineError> {
        let mut candidate = input.copy_for_policy(cx)?;
        candidate.frames.clear();
        candidate.location.frames.clear();
        candidate.work = 0;
        Err(MachineError::Shape)
    }
}

#[test]
fn custom_inner_refusal_restores_complete_machine_and_relative_input() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (entry, input) = nested(algebra.borrowed());
    let expected = input.clone();
    let history = input.active().key.jump_history.as_ptr();
    let stack = input.active().stack.as_ptr();
    let policy = Selective {
        inner: &StageThenRefuse,
        regions: &[],
    };
    let mut budget = WorkBudget::new(20_000_000);
    let refused = policy
        .prepare_owned(
            input,
            PolicySite::World,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap_err();
    assert_eq!(refused.error, MachineError::Shape);
    assert_eq!(refused.input, expected);
    assert_eq!(refused.input.active().stack.as_ptr(), stack);
    assert_eq!(refused.input.active().key.jump_history.as_ptr(), history);
    let mut relative = RelativeState::project(
        &expected,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
    )
    .unwrap();
    relative.location.frames[1].jump_history = vec![23];
    let expected = relative.clone();
    let refused = policy
        .prepare_relative_owned(
            relative,
            PolicySite::Body,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap_err();
    assert_eq!(refused.error, MachineError::Shape);
    assert_eq!(refused.input, expected);
}

#[test]
fn every_machine_and_relative_work_refusal_preserves_original_input() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (entry, input) = nested(algebra.borrowed());
    let policy = Selective {
        inner: &StructuralPartition,
        regions: &[],
    };
    let mut funded = WorkBudget::new(20_000_000);
    policy
        .prepare_owned(
            input.clone(),
            PolicySite::World,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut funded),
        )
        .unwrap();
    for limit in 0..funded.used() {
        let original = input.clone();
        let stack = original.active().stack.as_ptr();
        let history = original.active().key.jump_history.as_ptr();
        let mut denied = WorkBudget::new(limit);
        let refused = policy
            .prepare_owned(
                original,
                PolicySite::World,
                (),
                &mut PolicyContext::new(algebra.borrowed(), &entry, &mut denied),
            )
            .unwrap_err();
        assert_eq!(refused.error, MachineError::Work, "limit={limit}");
        assert_eq!(refused.input, input, "limit={limit}");
        assert_eq!(refused.input.active().stack.as_ptr(), stack);
        assert_eq!(refused.input.active().key.jump_history.as_ptr(), history);
        assert!(denied.exhausted());
    }
    let mut setup = WorkBudget::new(20_000_000);
    let mut relative = RelativeState::project(
        &input,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut setup),
    )
    .unwrap();
    relative.location.frames[0].jump_history = vec![31, 32, 33];
    let mut funded = WorkBudget::new(20_000_000);
    policy
        .prepare_relative_owned(
            relative.clone(),
            PolicySite::Body,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut funded),
        )
        .unwrap();
    for limit in 0..funded.used() {
        let mut denied = WorkBudget::new(limit);
        let refused = policy
            .prepare_relative_owned(
                relative.clone(),
                PolicySite::Body,
                (),
                &mut PolicyContext::new(algebra.borrowed(), &entry, &mut denied),
            )
            .unwrap_err();
        assert_eq!(refused.error, MachineError::Work, "limit={limit}");
        assert_eq!(refused.input, relative, "limit={limit}");
        assert!(denied.exhausted());
    }
}

pub(super) struct CancelInner<'a> {
    pub token: &'a analysis_control::Cancellation,
    pub seen: Cell<usize>,
}
impl<D: EvmNumericAlgebra> MachinePartition<D, ()> for CancelInner<'_> {
    type Key = (crate::analysis::MachineKey, ());
    fn prepare(
        &self,
        input: &EvmMachinePayload<D::Element>,
        _: PolicySite,
        _: (),
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Partitioned<Self::Key, D::Element>, MachineError> {
        assert!(
            input
                .call_stack
                .iter()
                .all(|frame| frame.key.jump_history.is_empty())
        );
        self.seen.set(self.seen.get() + 1);
        self.token.cancel();
        Err(cx.reserve(0).unwrap_err())
    }
    fn prepare_relative(
        &self,
        input: &RelativeState<D::Element>,
        _: PolicySite,
        _: (),
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<PartitionedRelative<Self::Key, D::Element>, MachineError> {
        assert!(
            input
                .frames
                .iter()
                .all(|frame| frame.key.jump_history.is_empty())
        );
        assert!(
            input
                .location
                .frames
                .iter()
                .all(|frame| frame.jump_history.is_empty())
        );
        self.seen.set(self.seen.get() + 1);
        self.token.cancel();
        Err(cx.reserve(0).unwrap_err())
    }
}

#[test]
fn cancelled_inner_restores_histories_and_work_without_an_extra_reservation() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (entry, input) = nested(algebra.borrowed());
    let mut setup = WorkBudget::new(20_000_000);
    let mut relative = RelativeState::project(
        &input,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut setup),
    )
    .unwrap();
    relative.location.frames[0].jump_history = vec![41];
    for captured in [false, true] {
        let token = analysis_control::Cancellation::new();
        let inner = CancelInner {
            token: &token,
            seen: Cell::new(0),
        };
        let policy = Selective {
            inner: &inner,
            regions: &[],
        };
        let mut budget = if captured {
            analysis_control::with_cancellation(&token, || WorkBudget::new(20_000_000))
        } else {
            WorkBudget::new(20_000_000)
        };
        let mut run = || {
            policy
                .prepare_owned(
                    input.clone(),
                    PolicySite::World,
                    (),
                    &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
                )
                .unwrap_err()
        };
        let refused = if captured {
            run()
        } else {
            analysis_control::with_cancellation(&token, run)
        };
        assert_eq!(refused.input, input);
        assert_eq!(
            refused.error,
            MachineError::Relations(crate::domain::relational::QueryReason::Cancelled)
        );
        assert_eq!(inner.seen.get(), 1);
        assert!(budget.used() > 0);
        let token = analysis_control::Cancellation::new();
        let inner = CancelInner {
            token: &token,
            seen: Cell::new(0),
        };
        let policy = Selective {
            inner: &inner,
            regions: &[],
        };
        let mut budget = if captured {
            analysis_control::with_cancellation(&token, || WorkBudget::new(20_000_000))
        } else {
            WorkBudget::new(20_000_000)
        };
        let mut run = || {
            policy
                .prepare_relative_owned(
                    relative.clone(),
                    PolicySite::Body,
                    (),
                    &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
                )
                .unwrap_err()
        };
        let refused = if captured {
            run()
        } else {
            analysis_control::with_cancellation(&token, run)
        };
        assert_eq!(refused.input, relative);
        assert_eq!(
            refused.error,
            MachineError::Relations(crate::domain::relational::QueryReason::Cancelled)
        );
        assert_eq!(inner.seen.get(), 1);
        assert!(budget.used() > 0);
    }
}

#[test]
fn saturated_relative_work_remains_a_conservative_bound() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (entry, input) = nested(algebra.borrowed());
    let mut budget = WorkBudget::new(20_000_000);
    let mut relative = RelativeState::project(
        &input,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
    )
    .unwrap();
    relative.work = usize::MAX;
    let policy = Selective {
        inner: &StructuralPartition,
        regions: &[],
    };
    let prepared = policy
        .prepare_relative_owned(
            relative,
            PolicySite::Body,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap();
    assert_eq!(prepared.payload.work, usize::MAX);
    assert!(prepared.payload.work >= prepared.payload.measure_work());
}

#[test]
fn location_only_erasure_keeps_frame_work_and_restores_the_original_location_on_refusal() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let mut budget = WorkBudget::new(20_000_000);
    let mut relative = RelativeState::project(
        &input,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
    )
    .unwrap();
    relative.location.frames[0].jump_history = vec![51, 52];
    let original = relative.clone();
    let pointer = relative.location.frames[0].jump_history.as_ptr();
    let refused = Selective {
        inner: &StageThenRefuse,
        regions: &[],
    }
    .prepare_relative_owned(
        relative,
        PolicySite::Subscriber,
        (),
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
    )
    .unwrap_err();
    assert_eq!(refused.input, original);
    assert_eq!(
        refused.input.location.frames[0].jump_history.as_ptr(),
        pointer
    );
    let prepared = Selective {
        inner: &StructuralPartition,
        regions: &[],
    }
    .prepare_relative_owned(
        refused.input,
        PolicySite::Subscriber,
        (),
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
    )
    .unwrap();
    assert!(prepared.payload.location.frames[0].jump_history.is_empty());
    assert_eq!(prepared.payload.work, original.work);
    assert_eq!(prepared.payload.work, prepared.payload.measure_work());
}

#[test]
fn selected_history_and_prepared_owned_inputs_remain_unchanged() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (entry, input) = nested(algebra.borrowed());
    let mut regions = vec![root_region(&input)];
    let child = &input.active().key;
    regions.push(HybridRegion {
        source: ProcessSource::Runtime {
            source: child.code_address,
            hash: child.code_hash,
            fork: Fork::Osaka,
        },
        code_address: child.code_address,
        code_hash: child.code_hash,
    });
    let policy = Selective {
        inner: &StructuralPartition,
        regions: &regions,
    };
    let expected = input.clone();
    let history = input.active().key.jump_history.as_ptr();
    let mut budget = WorkBudget::new(20_000_000);
    let prepared = policy
        .prepare_owned(
            input,
            PolicySite::World,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap();
    assert_eq!(prepared.payload, expected);
    assert_eq!(prepared.payload.active().key.jump_history.as_ptr(), history);
    let prepared_again = policy
        .prepare_owned(
            prepared.payload,
            PolicySite::World,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap();
    assert_eq!(prepared_again.key, prepared.key);
    assert_eq!(prepared_again.payload, expected);
    assert_eq!(
        prepared_again.payload.active().key.jump_history.as_ptr(),
        history
    );
}
