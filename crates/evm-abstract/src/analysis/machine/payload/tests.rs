use super::*;
use crate::{
    analysis::{EvmChildFrame, EvmContinuation, transfer},
    resource::clone_payment::tests::{RECEIPT, assert_unobserved, fixture, private_value, reset},
    world::EvmSnapshot,
};

#[test]
fn payload_owns_stacks_continuations_and_live_values_but_only_checkpoint_handles() {
    let _scope = RECEIPT.scope();
    let (world, entry, _, policy) = fixture("00");
    let mut input = transfer::initial(&world, &entry, policy.borrowed()).unwrap();
    let mut saved = input.store.clone();
    saved.write_balance(entry.address, private_value(usize::MAX));
    input.active_mut().saved_store = EvmSnapshot::from_state(saved);
    input.active_mut().stack.push(private_value(500_000));
    input.active_mut().address_operand = Some(private_value(500_000));
    let mut child = input.active().clone();
    child.stack.clear();
    child.address_operand = None;
    input.call_stack.push_child(EvmChildFrame {
        state: child,
        continuation: EvmContinuation {
            return_block: None,
            output_offset: private_value(500_000),
            output_size: private_value(500_000),
            creation: None,
        },
    });
    let old = input.clone();
    reset();
    let mut measured = WorkBudget::new(usize::MAX);
    input.pay_clone_with(&mut measured).unwrap();
    assert_unobserved();
    assert_eq!(RECEIPT.get().private_quotes, 4);
    let fee = measured.used();
    assert!(fee > 2_000_000);
    let mut quote = WorkBudget::new(usize::MAX);
    let remaining = input.clone_work_with(&mut quote).unwrap();
    assert_eq!(fee, quote.used() + remaining);
    reset();
    let mut refused = WorkBudget::new(fee - 1);
    assert_eq!(input.pay_clone_with(&mut refused), Err(CloneError::Work));
    assert_unobserved();
    assert!(refused.used() > 0);
    assert_eq!(input, old);
    reset();
    input.pay_clone_with(&mut WorkBudget::new(fee)).unwrap();
    let copied = input.clone();
    assert_eq!(RECEIPT.get().bytes, 4 * 4096);
    assert_eq!(copied, old);
    assert!(std::ptr::eq(
        copied.active().saved_store.state(),
        input.active().saved_store.state()
    ));
    assert!(
        copied.call_stack.root().state.stack[0]
            .same_identity(&input.call_stack.root().state.stack[0])
    );
    // Restoring the shared store has a separate owner and must refuse MAX.
    reset();
    assert_eq!(
        input
            .active()
            .saved_store
            .state()
            .pay_clone_with(&mut WorkBudget::new(usize::MAX)),
        Err(CloneError::Work)
    );
    assert_unobserved();
}

#[test]
fn runtime_metadata_and_policy_copy_pay_declared_clone_beside_retention() {
    use crate::analysis::engine::policy::{PolicyContext, WholeStateMetadata};
    let _scope = RECEIPT.scope();
    let (world, entry, _, policy) = fixture("00");
    let mut input = transfer::initial(&world, &entry, policy.borrowed()).unwrap();
    input.active_mut().stack.push(private_value(1_000_000));
    let retention = input.policy_work_size();
    assert!(retention < 50_000);
    for metadata in [false, true] {
        reset();
        let mut measure = WorkBudget::new(usize::MAX);
        let mut cx = PolicyContext::new(policy.borrowed(), &entry, &mut measure);
        if metadata {
            WholeStateMetadata::initialize(&input, &mut cx).unwrap();
        } else {
            let copied = input.copy_for_policy(&mut cx).unwrap();
            assert_eq!(copied, input);
            assert!(copied.active().stack[0].same_identity(&input.active().stack[0]));
        }
        assert_eq!(RECEIPT.get().bytes, 4096);
        let fee = measure.used();
        assert!(fee > retention + 1_000_000);
        reset();
        let mut budget = WorkBudget::new(fee - 1);
        let mut cx = PolicyContext::new(policy.borrowed(), &entry, &mut budget);
        let error = if metadata {
            WholeStateMetadata::initialize(&input, &mut cx).err()
        } else {
            input.copy_for_policy(&mut cx).err()
        };
        assert_eq!(error, Some(MachineError::Work));
        assert_unobserved();
        assert!(
            budget.used()
                >= if metadata {
                    retention + 1
                } else {
                    retention * 2
                }
        );
    }
    input.active_mut().stack[0] = private_value(usize::MAX);
    for metadata in [false, true] {
        reset();
        let mut budget = WorkBudget::new(usize::MAX);
        let mut cx = PolicyContext::new(policy.borrowed(), &entry, &mut budget);
        let error = if metadata {
            WholeStateMetadata::initialize(&input, &mut cx).err()
        } else {
            input.copy_for_policy(&mut cx).err()
        };
        assert_eq!(error, Some(MachineError::Work));
        assert_unobserved();
        assert_eq!(RECEIPT.get().private_quotes, 1);
    }
}
