//! Paid snapshot Clone against a complete private-carrier derived-Clone oracle.
use super::numeric::{CLONE_RECEIPT, CloneReceipt};
use super::*;
use crate::analysis::{CreationContinuation, ProcessFrameKey, ProcessOwner, ProcessSource};
use crate::resource::WorkBudget;

fn bound() -> EvmPayloadSummary<ExcludingWord> {
    let algebra = ValueAlgebra::new(Algebra::new());
    let domain = algebra.borrowed();
    let (entry, mut input) = fixture(domain);
    let owner = input.active().key.address;
    input.active_mut().key.jump_history = (0..17).collect();
    input.active_mut().saved_store = input.store.snapshot();
    input.store.write_balance(owner, value(17));
    let mut child = input.active().clone();
    child.key.caller = fixtures::address(0x202).into();
    child.key.jump_history = (0..33).collect();
    child.saved_store = input.store.snapshot();
    input.call_stack.push_child(EvmChildFrame {
        state: child,
        continuation: EvmContinuation {
            return_block: Some(3),
            output_offset: value(23),
            output_size: value(29),
            creation: Some(CreationContinuation::Native { destination: owner }),
        },
    });
    input.store.write_balance(owner, value(19));
    form_normalized(&input, &entry, domain).unwrap()
}

#[test]
fn private_payload_clone_preserves_complete_fields_and_pays_every_native_tariff() {
    let original = bound();
    let expected = original.clone();
    let _receipt = CLONE_RECEIPT.scope();
    let mut totals = Vec::new();
    for fee in [64, 4096] {
        CLONE_RECEIPT.set(CloneReceipt::new(fee));
        let mut budget = WorkBudget::new(100_000_000);
        let copied = original.clone_with_payment(&mut budget).unwrap();
        let receipt = CLONE_RECEIPT.get();
        assert_eq!(copied, expected);
        assert!(receipt.clones > 0);
        assert!(receipt.quotes >= receipt.clones);
        assert_eq!(
            receipt.exports, 0,
            "Clone may not reconstruct private N through observations"
        );
        totals.push((budget.used(), receipt.quotes));
        excludes(&copied.frames[0].stack[0], 13);
        excludes(&copied.continuations[0].output_offset, 23);
        excludes(&copied.continuations[0].output_size, 29);
    }
    eprintln!(
        "private snapshot clone: low_work={} high_work={} native_quotes={}",
        totals[0].0, totals[1].0, totals[0].1
    );
    assert_eq!(totals[0].1, totals[1].1);
    assert_eq!(totals[1].0 - totals[0].0, totals[0].1 * (4096 - 64));
    assert_eq!(original, expected);
}

#[test]
fn private_payload_clone_exact_minus_one_and_max_refuse_before_copying() {
    let original = bound();
    let expected = original.clone();
    let _receipt = CLONE_RECEIPT.scope();
    CLONE_RECEIPT.set(CloneReceipt::new(64));
    let mut measure = WorkBudget::new(100_000_000);
    original.clone_with_payment(&mut measure).unwrap();
    let exact = measure.used();
    for budget in [0, exact - 1] {
        CLONE_RECEIPT.set(CloneReceipt::new(64));
        let mut denied = WorkBudget::new(budget);
        assert_eq!(
            original.clone_with_payment(&mut denied),
            Err(MachineError::Work)
        );
        assert_eq!(CLONE_RECEIPT.get().clones, 0);
        assert_eq!(CLONE_RECEIPT.get().exports, 0);
        assert_eq!(original, expected);
        assert!(denied.exhausted());
        let before = CLONE_RECEIPT.get();
        assert!(original.clone_with_payment(&mut denied).is_err());
        assert_eq!(CLONE_RECEIPT.get().clones, before.clones);
    }
    CLONE_RECEIPT.set(CloneReceipt::new(64));
    let mut funded = WorkBudget::new(exact);
    assert_eq!(original.clone_with_payment(&mut funded).unwrap(), expected);
    assert_eq!(funded.used(), exact);
    CLONE_RECEIPT.set(CloneReceipt::new(usize::MAX));
    assert_eq!(
        original.clone_with_payment(&mut WorkBudget::new(usize::MAX)),
        Err(MachineError::Work)
    );
    assert_eq!(CLONE_RECEIPT.get().clones, 0);
    assert_eq!(CLONE_RECEIPT.get().exports, 0);
    assert_eq!(original, expected);
}

#[test]
fn private_payload_clone_observes_both_cancellation_owners_and_mid_operation_stops() {
    let original = bound();
    let expected = original.clone();
    let _receipt = CLONE_RECEIPT.scope();
    for captured in [false, true] {
        let token = analysis_control::Cancellation::new();
        let mut budget = if captured {
            analysis_control::with_cancellation(&token, || WorkBudget::new(100_000_000))
        } else {
            WorkBudget::new(100_000_000)
        };
        token.cancel();
        CLONE_RECEIPT.set(CloneReceipt::new(64));
        let result = if captured {
            original.clone_with_payment(&mut budget)
        } else {
            analysis_control::with_cancellation(&token, || original.clone_with_payment(&mut budget))
        };
        assert_eq!(
            result,
            Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled
            ))
        );
        assert_eq!(budget.used(), 0);
        assert_eq!(CLONE_RECEIPT.get().quotes, 0);
        assert_eq!(CLONE_RECEIPT.get().clones, 0);
    }
    for at_clone in [false, true] {
        let token = analysis_control::Cancellation::new();
        let mut receipt = CloneReceipt::new(64);
        receipt.cancel_quote = !at_clone;
        receipt.cancel_clone = at_clone;
        CLONE_RECEIPT.set(receipt);
        let result = analysis_control::with_cancellation(&token, || {
            original.clone_with_payment(&mut WorkBudget::new(100_000_000))
        });
        assert_eq!(
            result,
            Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled
            ))
        );
        assert_eq!(CLONE_RECEIPT.get().clones > 0, at_clone);
        assert_eq!(CLONE_RECEIPT.get().exports, 0);
        assert_eq!(original, expected);
    }
}

#[test]
fn private_payload_clone_keeps_variable_keys_process_fields_and_shape_errors() {
    let original = bound();
    let owner = original.frames[0].key.address;
    for history in [0, 1, 257] {
        let mut input = original.clone();
        input.frames[0].key.jump_history = (0..history).collect();
        input.location.frames = input.frames.iter().map(|frame| frame.key.clone()).collect();
        for process in [false, true] {
            let mut input = input.clone();
            if process {
                input.frames[0].key.process = Some(Box::new(ProcessFrameKey {
                    epoch: alloy_primitives::B256::repeat_byte(7),
                    owner: ProcessOwner::Named(owner),
                    source: ProcessSource::AnyRuntime { fork: Fork::Osaka },
                    is_static: true,
                }));
                input.location.frames[0] = input.frames[0].key.clone();
            }
            let expected = input.clone();
            let copied = input
                .clone_with_payment(&mut WorkBudget::new(100_000_000))
                .unwrap();
            assert_eq!(copied, expected);
            assert_eq!(input, expected);
        }
    }
    let mut malformed = original;
    malformed.continuations.clear();
    let expected = malformed.clone();
    let _receipt = CLONE_RECEIPT.scope();
    assert_eq!(
        malformed.clone_with_payment(&mut WorkBudget::new(100_000_000)),
        Err(MachineError::Shape)
    );
    assert_eq!(CLONE_RECEIPT.get().clones, 0);
    assert_eq!(CLONE_RECEIPT.get().quotes, 0);
    assert_eq!(malformed, expected);
}
