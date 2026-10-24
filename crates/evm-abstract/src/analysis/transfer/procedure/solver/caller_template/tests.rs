use super::*;
use crate::{
    U256,
    analysis::{
        CreationContinuation, EvmChildFrame, EvmContinuation, FrameCode, ProcessFrameKey,
        ProcessOwner, ProcessSource, TransferSemantics, state::payload, transfer,
    },
    domain::EvmValue,
    resource::clone_payment::tests::{
        RECEIPT, Receipt, Word, private_value, reset, summary_fixture,
    },
};

fn snapshot() -> EvmPayloadSummary<Word> {
    let (world, entry, _, algebra) = summary_fixture("00");
    let mut input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let private = private_value(50_000);
    input.active_mut().stack = vec![private.clone(), private.clone()];
    input.active_mut().call_value = private.clone();
    input.active_mut().address_operand = Some(private.clone());
    input.active_mut().caller_operand = Some(private.clone());
    input.active_mut().key.jump_history = (0..17).collect();
    input.store.write_balance(entry.address, private.clone());
    input.active_mut().saved_store = input.store.snapshot();
    let mut child = input.active().clone();
    child.key.jump_history = (0..33).collect();
    child.code = FrameCode::Runtime;
    input.call_stack.push_child(EvmChildFrame {
        state: child,
        continuation: EvmContinuation {
            return_block: Some(3),
            output_offset: private,
            output_size: EvmValue::constant(U256::ZERO),
            creation: None,
        },
    });
    let mut bound = payload::form_normalized(&input, &entry, algebra.borrowed()).unwrap();
    bound.frames[0].semantics = TransferSemantics::Environment;
    bound
}

#[test]
fn final_owned_caller_keeps_every_field_allocation_and_private_predicate_without_copy() {
    let input = snapshot();
    let retained = input.clone();
    let expected = input.clone();
    let frames = input.frames.as_ptr();
    let stack = input.frames[0].stack.as_ptr();
    let private = input.frames[0].stack[0].numeric().private.as_ptr();
    let _scope = RECEIPT.scope();
    reset();
    let mut budget = WorkBudget::new(1);
    let mut owned = into_owned_with_payment(Cow::Owned(input), &mut budget).unwrap();
    assert_eq!(owned, expected);
    assert_eq!(budget.used(), 1);
    assert_eq!(RECEIPT.get(), Receipt::default());
    assert_eq!(owned.frames.as_ptr(), frames);
    assert_eq!(owned.frames[0].stack.as_ptr(), stack);
    assert_eq!(owned.frames[0].stack[0].numeric().private.as_ptr(), private);
    assert!(!owned.frames[0].stack[0].contains(U256::from(7)));
    assert!(owned.frames[0].stack[0].contains(U256::from(8)));
    for position in [0, 1] {
        assert_eq!(
            owned.frames[0].stack[position].identity(),
            expected.frames[0].stack[position].identity()
        );
    }
    assert_eq!(
        owned.frames[0].stack_aliases,
        expected.frames[0].stack_aliases
    );
    // The detached snapshot may mutate; the subscriber failure anchor survives.
    owned.frames.pop();
    owned.continuations.pop();
    owned.location.frames.pop();
    owned.frames[0].stack.clear();
    assert_eq!(retained, expected);
    eprintln!(
        "final caller move: work=1 native_clones=0 native_quotes=0; complete fields and allocations preserved"
    );
}

#[test]
fn prior_borrowed_caller_keeps_its_full_native_copy_and_exact_refusal() {
    let input = snapshot();
    let expected = input.clone();
    let _scope = RECEIPT.scope();
    reset();
    let mut baseline = WorkBudget::new(100_000_000);
    input.clone_with_payment(&mut baseline).unwrap();
    let original_receipt = RECEIPT.get();
    assert!(original_receipt.clones > 0 && original_receipt.bytes >= 4096);
    reset();
    let mut exact = WorkBudget::new(baseline.used());
    assert_eq!(
        into_owned_with_payment(Cow::Borrowed(&input), &mut exact).unwrap(),
        expected
    );
    assert_eq!(exact.used(), baseline.used());
    assert_eq!(RECEIPT.get(), original_receipt);
    reset();
    assert_eq!(
        into_owned_with_payment(
            Cow::Borrowed(&input),
            &mut WorkBudget::new(baseline.used() - 1)
        ),
        Err(MachineError::Work)
    );
    assert_eq!(RECEIPT.get().clones, 0);
    assert_eq!(input, expected);
    let mut no_copy = input.clone();
    no_copy.frames[0].stack[0] = EvmValue::from_numeric(Word::expensive(usize::MAX));
    reset();
    assert_eq!(
        into_owned_with_payment(Cow::Borrowed(&no_copy), &mut WorkBudget::new(usize::MAX)),
        Err(MachineError::Work)
    );
    assert_eq!(RECEIPT.get().clones, 0);
    reset();
    into_owned_with_payment(Cow::Owned(no_copy), &mut WorkBudget::new(1)).unwrap();
    assert_eq!(RECEIPT.get(), Receipt::default());
    eprintln!(
        "prior caller copy: work={} native_clones={} private_bytes={}",
        baseline.used(),
        original_receipt.clones,
        original_receipt.bytes
    );
}

#[test]
fn caller_snapshot_observes_both_cancellation_owners_and_borrowed_quote_cancellation() {
    let input = snapshot();
    let expected = input.clone();
    let _scope = RECEIPT.scope();
    for captured_target in [false, true] {
        for owned in [false, true] {
            let copied = input.clone();
            let captured = analysis_control::Cancellation::new();
            let ambient = analysis_control::Cancellation::new();
            let mut budget =
                analysis_control::with_cancellation(&captured, || WorkBudget::new(100_000_000));
            if captured_target {
                captured.cancel();
            } else {
                ambient.cancel();
            }
            reset();
            let template = if owned {
                Cow::Owned(copied)
            } else {
                Cow::Borrowed(&input)
            };
            let error = analysis_control::with_cancellation(&ambient, || {
                into_owned_with_payment(template, &mut budget)
            });
            assert_eq!(
                error,
                Err(MachineError::Relations(
                    crate::domain::relational::QueryReason::Cancelled
                ))
            );
            assert_eq!(budget.used(), 0);
            assert_eq!(RECEIPT.get(), Receipt::default());
            assert_eq!(input, expected);
        }
        let captured = analysis_control::Cancellation::new();
        let ambient = analysis_control::Cancellation::new();
        let mut copied = input.clone();
        let mut word = Word::expensive(50_000);
        word.cancel_price = Some(if captured_target {
            captured.clone()
        } else {
            ambient.clone()
        });
        copied.frames[0].stack[0] = EvmValue::from_numeric(word);
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(100_000_000));
        RECEIPT.set(Receipt {
            armed: true,
            ..Receipt::default()
        });
        let error = analysis_control::with_cancellation(&ambient, || {
            into_owned_with_payment(Cow::Borrowed(&copied), &mut budget)
        });
        assert_eq!(
            error,
            Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled
            ))
        );
        assert!(RECEIPT.get().private_quotes > 0);
        assert_eq!(RECEIPT.get().clones, 0);
        assert_eq!(input, expected);
    }
}

#[test]
fn caller_move_retains_variable_process_and_creation_fields_and_sticky_refusal() {
    let input = snapshot();
    for history in [0, 1, 257] {
        for process in [false, true] {
            let mut owned = input.clone();
            owned.frames[0].key.jump_history = (0..history).collect();
            if process {
                owned.frames[0].key.process = Some(Box::new(ProcessFrameKey {
                    epoch: alloy_primitives::B256::repeat_byte(7),
                    owner: ProcessOwner::Named(owned.frames[0].key.address),
                    source: ProcessSource::AnyRuntime {
                        fork: crate::Fork::Osaka,
                    },
                    is_static: true,
                }));
            }
            owned.location.frames[0] = owned.frames[0].key.clone();
            owned.continuations[0].creation = Some(CreationContinuation::Native {
                destination: owned.frames[0].key.address,
            });
            let expected = owned.clone();
            let moved =
                into_owned_with_payment(Cow::Owned(owned), &mut WorkBudget::new(1)).unwrap();
            assert_eq!(moved, expected);
        }
    }
    let expected = input.clone();
    let mut failed = WorkBudget::new(0);
    assert_eq!(
        into_owned_with_payment(Cow::Owned(input.clone()), &mut failed),
        Err(MachineError::Work)
    );
    assert!(failed.exhausted());
    assert_eq!(
        into_owned_with_payment(Cow::Borrowed(&input), &mut failed),
        Err(MachineError::Work)
    );
    assert_eq!(input, expected);
    let mut malformed = input;
    malformed.continuations.clear();
    assert_eq!(
        into_owned_with_payment(Cow::Owned(malformed), &mut WorkBudget::new(1)),
        Err(MachineError::Shape)
    );
}
