use super::*;
use crate::{
    analysis::{transfer, worklist::MachineMeter},
    resource::{
        CloneError,
        clone_payment::tests::{RECEIPT, Word, assert_unobserved, fixture, private_value, reset},
    },
};
use analysis_kernel::Availability;
use std::convert::Infallible;

fn prepared() -> (
    Runtime<Infallible>,
    RunningJob,
    SlotMeta<EvmMachinePayload<Word>>,
    EvmMachinePayload<Word>,
) {
    let (world, entry, _, policy) = fixture("00");
    let mut input = transfer::initial(&world, &entry, policy.borrowed()).unwrap();
    input.active_mut().stack.push(private_value(1_000_000));
    let mut setup = WorkBudget::new(usize::MAX);
    let mut runtime = Runtime::new(&mut setup).unwrap();
    let scope = runtime.world;
    let slot = runtime
        .kernel()
        .slot(scope, &mut MachineMeter(&mut setup))
        .unwrap();
    runtime.register_world(0, 1, &mut setup).unwrap();
    let (_, active) = runtime.next(false, &mut setup).unwrap().unwrap();
    (runtime, active, slot, input)
}

#[test]
fn runtime_input_clone_payment_uses_declared_numeric_copy_price() {
    let _scope = RECEIPT.scope();
    let (mut runtime, mut active, slot, input) = prepared();
    reset();
    let mut measure = WorkBudget::new(usize::MAX);
    let answer = read(&mut runtime, &mut active, &slot, &input, &mut measure).unwrap();
    let fee = measure.used();
    assert!(fee > 1_000_000 && input.work_size() < 50_000);
    assert_eq!(RECEIPT.get().bytes, 4096);
    let Availability::Value(copy) = answer.state else {
        panic!("initialized input must be available")
    };
    assert_eq!(copy, input);
    assert!(copy.active().stack[0].same_identity(&input.active().stack[0]));
    for limit in [fee - 1, fee] {
        let (mut runtime, mut active, slot, input) = prepared();
        reset();
        let mut budget = WorkBudget::new(limit);
        let result = read(&mut runtime, &mut active, &slot, &input, &mut budget);
        if limit < fee {
            assert_eq!(result.unwrap_err(), MachineError::Work);
            assert_unobserved();
            assert!(budget.used() > 0 && budget.used() < fee);
        } else {
            assert_eq!(result.unwrap().version, answer.version);
            assert_eq!(RECEIPT.get().bytes, 4096);
            assert_eq!(budget.used(), fee);
        }
    }
    let (mut runtime, mut active, slot, mut input) = prepared();
    input.active_mut().stack[0] = private_value(usize::MAX);
    reset();
    assert_eq!(
        read(
            &mut runtime,
            &mut active,
            &slot,
            &input,
            &mut WorkBudget::new(usize::MAX)
        )
        .unwrap_err(),
        MachineError::from(CloneError::Work)
    );
    assert_unobserved();
}
