use super::{
    carrier::ExcludingWord,
    fixtures::*,
    policy::{Calls, Policy},
};
use alloy_primitives::U256;
use evm_abstract::{
    domain::{EvmValue, ValueAlgebra, WordElement, WordObservation},
    resource::WorkBudget,
    world::{EvmByteArray, EvmStore},
};
use revm_bytecode::opcode;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn exclusion_is_semantic_and_constrain_does_not_roundtrip_through_lossy_observation() {
    let original = ExcludingWord::except(U256::from(42));
    assert!(!original.contains(U256::from(42)));
    assert!(
        original.observation().contains(U256::from(42)),
        "the explicit observation boundary is intentionally lossy"
    );
    let constrained = original
        .constrain(&WordObservation::unsigned_range(U256::from(40), U256::from(44)).unwrap())
        .unwrap();
    assert!(!constrained.contains(U256::from(42)));
    assert!(constrained.contains(U256::from(41)));
    assert!(
        constrained
            .constrain(&WordObservation::constant(U256::from(42)))
            .is_err()
    );
    let calls = Calls::default();
    let algebra = ValueAlgebra::new(Policy::new(&calls));
    let value = EvmValue::from_numeric(constrained);
    assert_eq!(
        algebra
            .apply(opcode::EQ, &[word(42), value.clone()])
            .singleton(),
        Some(U256::ZERO)
    );
    let restored = EvmValue::from_numeric(ExcludingWord::from_observation(
        &value.numeric().observation(),
    ));
    assert!(
        restored.contains(U256::from(42)),
        "control must demonstrate genuine information loss at export/import"
    );
}

#[test]
fn actual_custom_values_survive_word_storage_saved_current_and_summary_boundaries() {
    let calls = Calls::default();
    let algebra = ValueAlgebra::new(Policy::new(&calls));
    let view = algebra.borrowed();
    let world = world(&[opcode::STOP]);
    let original = EvmValue::from_numeric(ExcludingWord::except(U256::from(42)));
    let mut memory = EvmByteArray::memory();
    memory
        .write_word_with(&word(0), &original, 65_536, view)
        .unwrap();
    assert!(
        !memory
            .read_word_with(&word(0), view)
            .contains(U256::from(42))
    );
    let memory_summary = memory.summary_projection_with(view).unwrap();
    let memory_copy = memory_summary.instantiate_with(view).unwrap();
    assert!(
        !memory_copy
            .read_word_with(&word(0), view)
            .contains(U256::from(42))
    );
    let mut store = EvmStore::new_with(&world, view);
    store.write_with(address(OWNER), &word(1), &original, view);
    store.write_transient_with(address(OWNER), &word(2), &original, view);
    let checkpoint = store.snapshot();
    store.write_with(address(OWNER), &word(1), &word(42), view);
    assert_eq!(
        store.read_with(address(OWNER), &word(1), view).singleton(),
        Some(U256::from(42))
    );
    store.restore(checkpoint);
    for slot in [0, 1] {
        assert!(
            !store
                .read_with(address(OWNER), &word(slot), view)
                .contains(U256::from(42))
        );
    }
    assert!(
        !store
            .read_transient_with(address(OWNER), &word(2), view)
            .contains(U256::from(42))
    );
    let summary = store.summary_projection_with(view).unwrap();
    let copy = summary.instantiate_with(view).unwrap();
    assert!(
        !copy
            .read_with(address(OWNER), &word(1), view)
            .contains(U256::from(42))
    );
    assert!(
        !copy
            .read_transient_with(address(OWNER), &word(2), view)
            .contains(U256::from(42))
    );
}

#[test]
fn quotation_and_refused_execution_do_not_deep_clone_an_unpaid_custom_element() {
    let calls = Calls::default();
    let algebra = ValueAlgebra::new(Policy::new(&calls));
    let clones = Arc::new(AtomicUsize::new(0));
    let mut element = ExcludingWord::top();
    element.excluded.extend((1..4096u64).map(U256::from));
    element.clone_counter = Some(Arc::clone(&clones));
    let args = [EvmValue::from_numeric(element), word(0)];
    let quote = algebra.operation_work(&args);
    assert!(quote >= args[0].work_size());
    assert_eq!(
        clones.load(Ordering::Relaxed),
        0,
        "D5: work quotation may borrow, never clone N"
    );
    assert_eq!(calls.arithmetic.load(Ordering::Relaxed), 0);
    let mut insufficient = WorkBudget::new(1);
    assert!(
        algebra
            .apply_budgeted(opcode::ADD, &args, &mut insufficient)
            .is_err()
    );
    assert_eq!(
        clones.load(Ordering::Relaxed),
        0,
        "admission failure must precede operand cloning"
    );
    let mut paid = WorkBudget::new(quote.saturating_mul(2));
    let result = algebra
        .apply_budgeted(opcode::ADD, &args, &mut paid)
        .unwrap();
    assert!(!result.value.contains(U256::ONE));
    assert!(
        clones.load(Ordering::Relaxed) > 0,
        "execution cloning is observable after charging"
    );
}
