use alloy_primitives::{Address, U256};

use crate::{
    analysis::{OutcomeKind, ProcessAction, ProcessOwner},
    creation::{
        Admission, Collision, CompletionReason, ConstructorCode, CreationError, CreationRequest,
        CreationResult, CreatorNonce, Destination, ParentException, RejectionReason, ReturnedData,
        StoreDisposition, complete, prepare, runtime_is_valid,
    },
    resource::WorkBudget,
};

pub(super) fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

pub(super) fn request(action: ProcessAction) -> CreationRequest {
    CreationRequest {
        action,
        creator: ProcessOwner::Named(address(0x101)),
        is_static: false,
        depth: Admission::Unknown,
        nonce: CreatorNonce::Unknown,
        funding: Admission::Unknown,
        initcode_size: Admission::Unknown,
        code: ConstructorCode::AnyInitCode,
    }
}

pub(super) fn targets() -> [Destination; 5] {
    [
        Destination {
            owner: ProcessOwner::Named(address(0x101)),
            collision: Collision::NonEmptyCode,
        },
        Destination {
            owner: ProcessOwner::Named(address(0x202)),
            collision: Collision::NonEmptyCode,
        },
        Destination {
            owner: ProcessOwner::Named(address(0x303)),
            collision: Collision::NonZeroNonce,
        },
        Destination {
            owner: ProcessOwner::Named(address(0x404)),
            collision: Collision::Available,
        },
        Destination {
            owner: ProcessOwner::Residual,
            collision: Collision::Unknown,
        },
    ]
}

#[test]
fn unknown_inputs_cover_all_collision_free_owner_and_runtime_classes() {
    for action in [ProcessAction::Create, ProcessAction::Create2] {
        let mut budget = WorkBudget::new(100_000);
        let plan = prepare(request(action), &targets(), 17, &mut budget).unwrap();
        assert_eq!(plan.constructors.len(), 2);
        for target in [ProcessOwner::Named(address(0x404)), ProcessOwner::Residual] {
            let entry = *plan
                .constructors
                .iter()
                .find(|entry| entry.destination == target)
                .unwrap();
            assert_eq!(entry.action, action);
            assert_eq!(entry.creator, ProcessOwner::Named(address(0x101)));
            assert_eq!(entry.code, ConstructorCode::AnyInitCode);
            assert_eq!(entry.vocabulary, 17);
            assert!(entry.has_empty_calldata());
            assert!(entry.account_runtime_is_empty());
            for required in [
                ProcessAction::Call,
                ProcessAction::StaticCall,
                ProcessAction::DelegateCall,
                ProcessAction::CallCode,
                ProcessAction::Create,
                ProcessAction::Create2,
            ] {
                assert!(entry.actions.contains(&required));
            }
            let completions = complete(entry, OutcomeKind::Return, &mut budget).unwrap();
            let success = completions[0].unwrap();
            assert_eq!(success.result, CreationResult::Address(target));
            assert_eq!(success.returndata, ReturnedData::Empty);
            assert_eq!(success.store, StoreDisposition::CommitConstructor);
            assert_eq!(success.deployment.unwrap().owner, target);
            assert_eq!(
                completions[1].unwrap().reason,
                CompletionReason::Exceptional
            );
            for runtime in [
                vec![],
                vec![0],
                vec![0xfe],
                vec![0x60, 0x42, 0x00],
                vec![0; 24_576],
            ] {
                assert!(runtime_is_valid(&runtime, &mut budget).unwrap());
            }
            for runtime in [vec![0xef], vec![0xef, 0x01], vec![0; 24_577]] {
                assert!(!runtime_is_valid(&runtime, &mut budget).unwrap());
            }
            let reverted = complete(entry, OutcomeKind::Revert, &mut budget).unwrap()[0].unwrap();
            assert_eq!(reverted.result, CreationResult::Zero);
            assert_eq!(reverted.returndata, ReturnedData::ConstructorRevert);
            assert_eq!(reverted.store, StoreDisposition::RestorePostNonceCheckpoint);
            let failed = complete(entry, OutcomeKind::Failure, &mut budget).unwrap()[0].unwrap();
            assert_eq!(failed.result, CreationResult::Zero);
            assert_eq!(failed.returndata, ReturnedData::Empty);
        }
        for reason in [
            RejectionReason::MaxNonce,
            RejectionReason::InsufficientBalance,
            RejectionReason::Depth,
            RejectionReason::ChildGas,
        ] {
            assert!(
                plan.rejections
                    .iter()
                    .any(|rejection| rejection.reason == reason)
            );
        }
        for target in targets().iter().take(3) {
            assert!(
                !plan
                    .constructors
                    .iter()
                    .any(|entry| entry.destination == target.owner)
            );
            assert!(
                plan.rejections
                    .iter()
                    .any(|rejection| rejection.destination == Some(target.owner)
                        && rejection.reason == RejectionReason::Collision
                        && rejection.increments_nonce)
            );
        }
        assert!(plan.exceptions.contains(&ParentException::Gas));
        assert!(plan.exceptions.contains(&ParentException::InitCodeSize));
        println!(
            "creation-plan action={action:?} constructors={} rejections={} work={}",
            plan.constructors.len(),
            plan.rejections.len(),
            budget.used()
        );
    }
}

#[test]
fn static_and_early_rejections_do_not_enter_or_bump_creator_nonce() {
    let mut budget = WorkBudget::new(100_000);
    for action in [ProcessAction::Create, ProcessAction::Create2] {
        let mut input = request(action);
        input.is_static = true;
        let plan = prepare(input, &targets(), 17, &mut budget).unwrap();
        assert_eq!(plan.exceptions, [ParentException::Static]);
        assert!(plan.rejections.is_empty());
        assert!(plan.constructors.is_empty());
        for (nonce, funding, depth, reason) in [
            (
                CreatorNonce::Exact(u64::MAX),
                Admission::Allowed,
                Admission::Allowed,
                RejectionReason::MaxNonce,
            ),
            (
                CreatorNonce::Exact(0),
                Admission::Rejected,
                Admission::Allowed,
                RejectionReason::InsufficientBalance,
            ),
            (
                CreatorNonce::Exact(0),
                Admission::Allowed,
                Admission::Rejected,
                RejectionReason::Depth,
            ),
        ] {
            input.is_static = false;
            input.nonce = nonce;
            input.funding = funding;
            input.depth = depth;
            let plan = prepare(input, &targets(), 17, &mut budget).unwrap();
            assert!(plan.constructors.is_empty());
            assert!(
                plan.rejections
                    .iter()
                    .any(|rejection| rejection.reason == reason)
            );
            assert!(
                plan.rejections
                    .iter()
                    .all(|rejection| !rejection.increments_nonce)
            );
        }
    }
}

#[test]
fn refusal_is_uncompleted_before_allocation_and_cancellation_stays_bound() {
    let mut budget = WorkBudget::new(1);
    assert_eq!(
        prepare(request(ProcessAction::Create), &targets(), 1, &mut budget),
        Err(CreationError::Uncompleted)
    );
    assert_eq!(budget.used(), 0);
    assert!(budget.exhausted());
    assert_eq!(
        prepare(request(ProcessAction::Create2), &targets(), 1, &mut budget),
        Err(CreationError::Uncompleted)
    );
    let cancellation = analysis_control::Cancellation::new();
    let mut cancelled =
        analysis_control::with_cancellation(&cancellation, || WorkBudget::new(100_000));
    cancellation.cancel();
    assert_eq!(
        prepare(
            request(ProcessAction::Create),
            &targets(),
            1,
            &mut cancelled
        ),
        Err(CreationError::Uncompleted)
    );
    assert_eq!(cancelled.used(), 0);
    let mut paid = WorkBudget::new(100_000);
    assert_eq!(
        prepare(request(ProcessAction::Call), &targets(), 1, &mut paid),
        Err(CreationError::Action(ProcessAction::Call))
    );
    assert_eq!(
        prepare(
            request(ProcessAction::Create),
            &targets()[..4],
            1,
            &mut paid
        ),
        Err(CreationError::Partition)
    );
}
