use super::*;
use crate::{
    domain::EvmValue,
    resource::clone_payment::tests::{self as carrier, RECEIPT, Word},
};

#[test]
fn changed_owned_machine_never_quotes_or_copies_expensive_opaque_scalars() {
    let _receipt = RECEIPT.scope();
    let (world, entry, _, algebra) = carrier::fixture("00");
    let mut input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let alias = EvmValue::from_numeric(Word::expensive(usize::MAX))
        .with_expression(ExprId::fresh().unwrap());
    input.active_mut().stack = vec![alias.clone(), alias];
    input.active_mut().key.jump_history = vec![1, 2, 3];
    let expected = input.clone();
    let pointer = input.active().stack[0].numeric().private.as_ptr();
    let policy = Selective {
        inner: &StructuralPartition,
        regions: &[],
    };
    carrier::reset();
    let mut budget = WorkBudget::new(100_000);
    let borrowed = policy.prepare(
        &input,
        PolicySite::World,
        (),
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
    );
    assert!(matches!(borrowed, Err(MachineError::Work)));
    assert_eq!(RECEIPT.get().clones, 0);
    assert!(RECEIPT.get().private_quotes > 0);
    assert_eq!(input, expected);
    carrier::reset();
    let mut budget = WorkBudget::new(100_000);
    let prepared = policy
        .prepare_owned(
            input,
            PolicySite::World,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap();
    assert_eq!(RECEIPT.get(), carrier::Receipt::default());
    assert_eq!(
        prepared.payload.active().stack[0]
            .numeric()
            .private
            .as_ptr(),
        pointer
    );
    assert!(!prepared.payload.active().stack[0].contains(alloy_primitives::U256::from(7)));
    assert_eq!(
        prepared.payload.active().stack[0].expression(),
        prepared.payload.active().stack[1].expression()
    );
    assert!(prepared.payload.active().key.jump_history.is_empty());
    assert!(budget.used() > 0);
}

#[test]
fn changed_owned_relative_moves_opaque_predicates_and_restores_them_on_cancel() {
    let _receipt = RECEIPT.scope();
    let (world, entry, _, algebra) = carrier::summary_fixture("00");
    let mut input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    input.active_mut().stack = vec![EvmValue::from_numeric(Word::expensive(1_000_000))];
    input.active_mut().key.jump_history = vec![11, 12, 13];
    let mut setup = WorkBudget::new(100_000_000);
    let mut relative = RelativeState::project(
        &input,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut setup),
    )
    .unwrap();
    relative.location.frames[0].jump_history = vec![21, 22];
    // The retained scalar itself cannot be lawfully copied within any budget.
    // History preparation has no reason to consult or observe that quotation.
    relative.frames[0].stack[0] = relative.frames[0].stack[0]
        .clone()
        .with_numeric(Word::expensive(usize::MAX));
    let original = relative.clone();
    let pointer = relative.frames[0].stack[0].numeric().private.as_ptr();
    let policy = Selective {
        inner: &StructuralPartition,
        regions: &[],
    };
    carrier::reset();
    let prepared = policy
        .prepare_relative_owned(
            relative,
            PolicySite::Body,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut WorkBudget::new(100_000)),
        )
        .unwrap();
    assert_eq!(RECEIPT.get(), carrier::Receipt::default());
    assert_eq!(
        prepared.payload.frames[0].stack[0]
            .numeric()
            .private
            .as_ptr(),
        pointer
    );
    assert!(!prepared.payload.frames[0].stack[0].contains(alloy_primitives::U256::from(7)));
    assert_eq!(prepared.payload.work, original.work - 3);
    assert_eq!(prepared.payload.work, prepared.payload.measure_work());
    let token = analysis_control::Cancellation::new();
    let inner = super::owned::CancelInner {
        token: &token,
        seen: std::cell::Cell::new(0),
    };
    let policy = Selective {
        inner: &inner,
        regions: &[],
    };
    let input = original.clone();
    let pointer = input.frames[0].stack[0].numeric().private.as_ptr();
    carrier::reset();
    let refusal = analysis_control::with_cancellation(&token, || {
        policy
            .prepare_relative_owned(
                input,
                PolicySite::Body,
                (),
                &mut PolicyContext::new(algebra.borrowed(), &entry, &mut WorkBudget::new(100_000)),
            )
            .unwrap_err()
    });
    assert_eq!(RECEIPT.get(), carrier::Receipt::default());
    assert_eq!(refusal.input, original);
    assert_eq!(
        refusal.input.frames[0].stack[0].numeric().private.as_ptr(),
        pointer
    );
}

#[test]
#[ignore = "scoped operation A/B; run explicitly to retain raw contention timing"]
fn changed_owned_preparation_operation_ab() {
    use std::{hint::black_box, time::Instant};
    let _receipt = RECEIPT.scope();
    let (world, entry, _, algebra) = carrier::summary_fixture("00");
    let mut input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    input.active_mut().stack = (0..64)
        .map(|_| EvmValue::from_numeric(Word::expensive(16_384)))
        .collect();
    input.active_mut().key.jump_history = (0..32).collect();
    let policy = Selective {
        inner: &StructuralPartition,
        regions: &[],
    };
    let mut setup = WorkBudget::new(100_000_000);
    let relative = RelativeState::project(
        &input,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut setup),
    )
    .unwrap();
    const RUNS: usize = 64;
    const ALLOWANCE: usize = 100_000_000;
    // Each input is an independent operation. Keep its allowance fixed and
    // report the sum of all paid work, rather than sharing one run ledger.
    for owned in [false, true] {
        let inputs: Vec<_> = (0..RUNS).map(|_| input.clone()).collect();
        carrier::reset();
        let mut work = 0usize;
        let start = Instant::now();
        for original in inputs {
            let mut budget = WorkBudget::new(ALLOWANCE);
            let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
            let prepared = if owned {
                policy
                    .prepare_owned(original, PolicySite::World, (), &mut cx)
                    .unwrap()
            } else {
                // Reproduce the base3801 changed-owned branch: outer census,
                // change detection, borrowed staging and inner preparation.
                policy.census(original.call_stack.depth(), &mut cx).unwrap();
                assert!(
                    original
                        .call_stack
                        .iter()
                        .any(|frame| policy.changes(&frame.key))
                );
                policy
                    .prepare(&original, PolicySite::World, (), &mut cx)
                    .unwrap()
            };
            assert!(prepared.payload.active().key.jump_history.is_empty());
            work = work.saturating_add(budget.used());
            black_box(prepared);
        }
        eprintln!(
            "owned-history-ab kind=machine owned={owned} runs={RUNS} allowance_per_operation={ALLOWANCE} elapsed_ns={} total_work={} receipt={:?}",
            start.elapsed().as_nanos(),
            work,
            RECEIPT.get()
        );
        if owned {
            assert_eq!(RECEIPT.get(), carrier::Receipt::default());
        } else {
            assert!(RECEIPT.get().bytes >= RUNS * 64 * 4096);
        }
    }
    for owned in [false, true] {
        let inputs: Vec<_> = (0..RUNS).map(|_| relative.clone()).collect();
        carrier::reset();
        let mut work = 0usize;
        let start = Instant::now();
        for original in inputs {
            let mut budget = WorkBudget::new(ALLOWANCE);
            let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
            let prepared = if owned {
                policy
                    .prepare_relative_owned(original, PolicySite::Body, (), &mut cx)
                    .unwrap()
            } else {
                policy
                    .census(
                        original.frames.len() + original.location.frames.len(),
                        &mut cx,
                    )
                    .unwrap();
                assert!(
                    original
                        .frames
                        .iter()
                        .any(|frame| policy.changes(&frame.key))
                        || original
                            .location
                            .frames
                            .iter()
                            .any(|frame| policy.changes(frame))
                );
                policy
                    .prepare_relative(&original, PolicySite::Body, (), &mut cx)
                    .unwrap()
            };
            assert!(prepared.payload.frames[0].key.jump_history.is_empty());
            assert_eq!(prepared.payload.work, prepared.payload.measure_work());
            work = work.saturating_add(budget.used());
            black_box(prepared);
        }
        eprintln!(
            "owned-history-ab kind=relative owned={owned} runs={RUNS} allowance_per_operation={ALLOWANCE} elapsed_ns={} total_work={} receipt={:?}",
            start.elapsed().as_nanos(),
            work,
            RECEIPT.get()
        );
        if owned {
            assert_eq!(RECEIPT.get(), carrier::Receipt::default());
        } else {
            assert!(RECEIPT.get().bytes >= RUNS * 64 * 4096);
        }
    }
}
