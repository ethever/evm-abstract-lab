use super::*;
use crate::{
    analysis::engine::policy::RecentDistinctJumps,
    resource::clone_payment::tests::{
        RECEIPT, Receipt, Word, assert_unobserved, fixture, private_value, reset,
    },
};

#[test]
fn stop_return_revert_refuse_the_actual_upstream_payload_copy() {
    let _scope = RECEIPT.scope();
    for opcode in [opcode::STOP, opcode::RETURN, opcode::REVERT] {
        let (world, entry, config, policy) = fixture(&format!("{opcode:02x}"));
        let mut input = initial(&world, &entry, policy.borrowed()).unwrap();
        // The expensive value survives in a non-argument stack slot. RETURN's
        // zero range makes its independently funded slice contain no private N.
        input.active_mut().stack.push(private_value(usize::MAX));
        if opcode != opcode::STOP {
            input.active_mut().stack.extend([constant(0), constant(0)]);
        }
        input.normalize();
        reset();
        let mut budget = WorkBudget::new(config.max_work);
        let execution = execute(
            &world,
            &entry,
            input,
            &config,
            policy.borrowed(),
            &RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert_eq!(RECEIPT.get().bytes, 0);
        assert_eq!(RECEIPT.get().private_quotes, 1);
        assert!(
            execution.outcomes.is_empty()
                && execution.completed_calls.is_empty()
                && execution.successors.is_empty()
        );
        assert!(
            execution
                .frontiers
                .iter()
                .any(|frontier| frontier.reason == FrontierReason::Work)
        );
        assert_eq!(
            execution.payload.active().stack[0].numeric().private.len(),
            4096
        );
    }
}

#[test]
fn implicit_stop_clone_payment_covers_empty_code_falloff_and_synthetic_continuation() {
    let _scope = RECEIPT.scope();
    for (code, synthetic) in [("", false), ("5b", false), ("00", true)] {
        let (world, entry, config, policy) = fixture(code);
        let mut input = initial(&world, &entry, policy.borrowed()).unwrap();
        input.active_mut().stack.push(private_value(usize::MAX));
        if synthetic {
            input.active_mut().key.basic_block_index =
                input.active().program.as_ref().unwrap().blocks().len();
        }
        input.normalize();
        reset();
        let execution = execute(
            &world,
            &entry,
            input,
            &config,
            policy.borrowed(),
            &RecentDistinctJumps,
            &mut WorkBudget::new(config.max_work),
            StorageReadPolicy::Abstract,
        );
        assert_unobserved();
        assert_eq!(RECEIPT.get().private_quotes, 1);
        assert!(
            execution.outcomes.is_empty()
                && execution.completed_calls.is_empty()
                && execution.successors.is_empty()
        );
        assert_eq!(execution.frontiers[0].reason, FrontierReason::Work);
    }
}

#[test]
fn summary_resume_refuses_before_its_initial_payload_clone() {
    let _scope = RECEIPT.scope();
    let (world, entry, config, policy) = fixture("00");
    let mut input = initial(&world, &entry, policy.borrowed()).unwrap();
    input.active_mut().stack.push(private_value(usize::MAX));
    reset();
    let mut budget = WorkBudget::new(config.max_work);
    let execution = resume_summary(
        input,
        OutcomeKind::Return,
        EvmByteArray::empty(),
        &config,
        policy.borrowed(),
        &mut budget,
    );
    assert_unobserved();
    assert!(execution.outcomes.is_empty() && execution.completed_calls.is_empty());
    assert_eq!(execution.frontiers[0].reason, FrontierReason::Work);
}

#[test]
fn common_memory_failure_clone_payment_refuses_before_copying_unrelated_native_predicate() {
    let _scope = RECEIPT.scope();
    for (code, inputs) in [("51", 1), ("52", 2), ("20", 2)] {
        let (world, entry, config, policy) = fixture(code);
        let mut input = initial(&world, &entry, policy.borrowed()).unwrap();
        input.active_mut().stack.push(private_value(usize::MAX));
        input
            .active_mut()
            .stack
            .extend(std::iter::repeat_with(|| constant(0)).take(inputs));
        input.normalize();
        reset();
        let execution = execute(
            &world,
            &entry,
            input,
            &config,
            policy.borrowed(),
            &RecentDistinctJumps,
            &mut WorkBudget::new(config.max_work),
            StorageReadPolicy::Abstract,
        );
        assert_eq!(RECEIPT.get().bytes, 0);
        assert_eq!(RECEIPT.get().private_quotes, 1);
        assert!(
            execution.outcomes.is_empty()
                && execution.completed_calls.is_empty()
                && execution.successors.is_empty()
        );
        assert_eq!(execution.frontiers[0].reason, FrontierReason::Work);
        assert_eq!(
            execution.payload.active().stack[0].numeric().private.len(),
            4096
        );
    }
}

#[test]
fn common_memory_failure_clone_price_checks_current_and_captured_cancellation() {
    let _scope = RECEIPT.scope();
    for cancel_captured in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let (world, entry, config, policy) = fixture("51");
        let mut input = initial(&world, &entry, policy.borrowed()).unwrap();
        let mut word = Word::expensive(1_000_000);
        word.cancel_price = Some(if cancel_captured {
            captured.clone()
        } else {
            current.clone()
        });
        input
            .active_mut()
            .stack
            .extend([EvmValue::from_numeric(word), constant(0)]);
        input.normalize();
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(config.max_work));
        reset();
        RECEIPT.set(Receipt {
            armed: true,
            ..Receipt::default()
        });
        let execution = analysis_control::with_cancellation(&current, || {
            execute(
                &world,
                &entry,
                input,
                &config,
                policy.borrowed(),
                &RecentDistinctJumps,
                &mut budget,
                StorageReadPolicy::Abstract,
            )
        });
        assert_eq!(RECEIPT.get().bytes, 0);
        assert_eq!(RECEIPT.get().private_quotes, 1);
        assert!(execution.outcomes.is_empty() && execution.completed_calls.is_empty());
        assert_eq!(
            execution.frontiers[0].reason,
            FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
        );
    }
}
