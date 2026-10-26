//! Fanout copies keep opaque predicates and independently funded locations.
use super::*;

#[test]
fn fanout_location_copy_retains_private_native_data_and_refuses_transactionally() {
    let values = ValueAlgebra::new(Algebra::<64, 4096>);
    let (_, mut input, _) = fixture(&values);
    input.location.frames[0].jump_history = (0..4096).collect();
    let original = input.clone();
    let mut budget = WorkBudget::new(100_000_000);
    let (copied, calls) = measure(|| input.clone_at_block_with_payment(7, &mut budget));
    let mut copied = copied.unwrap();
    let cost = budget.used();
    assert_eq!(copied.frames.last().unwrap().key.basic_block_index, 7);
    assert_eq!(copied.location.frames.last().unwrap().basic_block_index, 7);
    assert_eq!(
        copied.location.frames[0].jump_history,
        original.location.frames[0].jump_history
    );
    assert!(!copied.frames[0].stack[0].contains(U256::from(17)));
    assert!(calls.clones > 0);
    assert_eq!(calls.singletons, 0);
    copied.frames.last_mut().unwrap().key.basic_block_index =
        original.frames.last().unwrap().key.basic_block_index;
    copied.location.frames.last_mut().unwrap().basic_block_index =
        original.location.frames.last().unwrap().basic_block_index;
    assert_eq!(copied, original);
    assert!(matches!(
        input.clone_at_block_with_payment(7, &mut WorkBudget::new(cost - 1)),
        Err(MachineError::Work)
    ));
    assert!(
        input
            .clone_at_block_with_payment(7, &mut WorkBudget::new(cost))
            .is_ok()
    );
    assert_eq!(input, original);
    let owner = analysis_control::Cancellation::new();
    let mut cancelled =
        analysis_control::with_cancellation(&owner, || WorkBudget::new(100_000_000));
    owner.cancel();
    assert!(
        input
            .clone_at_block_with_payment(7, &mut cancelled)
            .is_err()
    );
    let mut active = WorkBudget::new(100_000_000);
    let ambient = analysis_control::Cancellation::new();
    ambient.cancel();
    assert!(
        analysis_control::with_cancellation(&ambient, || input
            .clone_at_block_with_payment(7, &mut active))
        .is_err()
    );
    assert_eq!(active.used(), 0);
    assert_eq!(input, original);
}
