use super::*;
use crate::analysis::{ProcessOwner, ProcessSource};
use alloy_primitives::{Address, B256};

fn key() -> ProcessFrameKey {
    ProcessFrameKey {
        epoch: B256::ZERO,
        owner: ProcessOwner::Named(Address::repeat_byte(1)),
        source: ProcessSource::Runtime {
            source: Address::repeat_byte(2),
            hash: B256::ZERO,
            fork: crate::Fork::Osaka,
        },
        is_static: false,
        location: ProcessLocation::Block(0),
    }
}

fn region(plan: Plan) -> Arc<ProcessControlRegion> {
    match plan {
        Plan::Region(region) => region,
        Plan::Boundary(_) => panic!("pure loop must compose control"),
    }
}

#[test]
fn immutable_code_shares_control_across_owners_but_checks_source_epoch_and_program() {
    let program = Program::from_hex("5b5f56").unwrap();
    let mut cache = Cache::default();
    let mut budget = WorkBudget::new(1_000_000);
    let mut frame = key();
    let first = region(cache.plan(&program, &frame, &mut budget).unwrap());
    frame.owner = ProcessOwner::Residual;
    frame.is_static = true;
    let other_owner = region(cache.plan(&program, &frame, &mut budget).unwrap());
    assert!(Arc::ptr_eq(&first, &other_owner));
    frame.epoch = B256::repeat_byte(3);
    let epoch = region(cache.plan(&program, &frame, &mut budget).unwrap());
    assert!(!Arc::ptr_eq(&first, &epoch));
    frame.source = ProcessSource::InputProgram {
        hash: B256::ZERO,
        fork: crate::Fork::Osaka,
    };
    let source = region(cache.plan(&program, &frame, &mut budget).unwrap());
    assert!(!Arc::ptr_eq(&epoch, &source));
    // Even an equal caller-supplied descriptor cannot replace the retained
    // immutable Program comparison: different code reaches a real exit.
    let changed = Program::from_hex("5b00").unwrap();
    assert!(matches!(
        cache.plan(&changed, &frame, &mut budget).unwrap(),
        Plan::Boundary(_)
    ));
}

#[test]
fn refused_source_preparation_does_not_install_a_partial_control_relation() {
    let program = Program::from_hex("5b5f56").unwrap();
    let mut cache = Cache::default();
    assert!(
        cache
            .plan(&program, &key(), &mut WorkBudget::new(100))
            .is_err()
    );
    assert!(cache.0.is_empty());
    let plan = cache
        .plan(&program, &key(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    assert!(region(plan).boundaries.is_empty());
}
