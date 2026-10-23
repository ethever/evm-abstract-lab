use super::super::*;
use crate::{
    domain::symbolic::ExprId,
    resource::{
        CloneError, WorkBudget,
        clone_payment::tests::{RECEIPT, Word, assert_unobserved, fixture, private_value, reset},
    },
    world::{EvmByteArray, LogSourceKind},
};
use alloy_primitives::B256;

fn summary(work: usize) -> EvmStoreSummary<Word> {
    let (world, entry, _, policy) = fixture("00");
    let mut out = EvmStore::new(&world, policy.borrowed())
        .summary_projection(policy.borrowed())
        .unwrap();
    let owner = entry.address;
    for plane in [&mut out.persistent, &mut out.transient] {
        plane.slots.insert((owner, U256::ONE), private_value(work));
        plane.defaults.insert(owner, private_value(work));
        plane.global_default = private_value(work);
    }
    out.balances.insert(owner, private_value(work));
    out.balance_default = private_value(work);
    out.nonces.insert(owner, private_value(work));
    out.possible_logs.insert(
        LogKey {
            address: owner,
            code_address: owner,
            code_hash: B256::ZERO,
            source_kind: LogSourceKind::Runtime,
            pc: 0,
        },
        SummaryLog {
            topics: vec![private_value(work)],
            data: EvmByteArray::<Word>::exact(&[1, 2])
                .summary_projection(policy.borrowed())
                .unwrap(),
        },
    );
    out
}

#[test]
fn structural_sizing_never_discovers_native_singletons_or_copies_values() {
    let _scope = RECEIPT.scope();
    let summary = summary(usize::MAX);
    reset();
    assert!(summary.work_size() > 0);
    assert_unobserved();
    assert_eq!(RECEIPT.get().quotes, 0);
}

#[test]
fn clone_invoice_refuses_max_before_any_copy_or_query() {
    let _scope = RECEIPT.scope();
    let summary = summary(usize::MAX);
    let original = summary.clone();
    reset();
    let mut budget = WorkBudget::new(usize::MAX);
    assert_eq!(
        summary.clone_with_payment(&mut budget),
        Err(CloneError::Work)
    );
    assert_unobserved();
    assert!(RECEIPT.get().private_quotes > 0);
    assert_eq!(summary, original);
}

#[test]
fn nested_log_byte_clone_refusal_precedes_the_summary_copy() {
    let _scope = RECEIPT.scope();
    let mut summary = summary(64);
    let (_, _, _, policy) = fixture("00");
    let mut bytes = EvmByteArray::<Word>::exact(&[1]);
    bytes.update_values(&mut |value| {
        *value = value.clone().with_numeric(Word::expensive(usize::MAX));
    });
    let key = summary.possible_logs.keys().next().unwrap().clone();
    summary.possible_logs.get_mut(&key).unwrap().data =
        bytes.summary_projection(policy.borrowed()).unwrap();
    reset();
    assert_eq!(
        summary.clone_with_payment(&mut WorkBudget::new(usize::MAX)),
        Err(CloneError::Work)
    );
    assert_unobserved();
    assert!(RECEIPT.get().private_quotes > 10);
}

#[test]
fn aggregate_native_copy_fee_overflow_refuses_before_every_copy() {
    let _scope = RECEIPT.scope();
    let summary = summary(usize::MAX / 2);
    reset();
    assert_eq!(
        summary.clone_with_payment(&mut WorkBudget::new(usize::MAX)),
        Err(CloneError::Work)
    );
    assert_unobserved();
    assert!(RECEIPT.get().private_quotes >= 2);
}

#[test]
fn clone_invoice_prices_all_native_fields_and_saved_snapshot_identity() {
    let _scope = RECEIPT.scope();
    let summary = summary(100_000);
    let original = summary.clone();
    reset();
    let mut measured = WorkBudget::new(usize::MAX);
    summary.pay_clone_with(&mut measured).unwrap();
    assert_unobserved();
    // Two planes (cell, owner default, global), balance/default, nonce, topic.
    assert_eq!(RECEIPT.get().private_quotes, 10);
    let fee = measured.used();
    assert!(fee > 1_000_000);

    reset();
    let mut refused = WorkBudget::new(fee - 1);
    assert_eq!(
        summary.clone_with_payment(&mut refused),
        Err(CloneError::Work)
    );
    assert_unobserved();
    assert_eq!(summary, original);

    reset();
    let mut budget = WorkBudget::new(fee);
    let copied = summary.clone_with_payment(&mut budget).unwrap();
    assert_eq!(copied, original);
    assert_eq!(budget.used(), fee);
    assert_eq!(RECEIPT.get().queries, 0);
    assert_eq!(RECEIPT.get().exports, 0);
    assert!(
        copied
            .balance_default
            .same_identity(&summary.balance_default)
    );
    assert_eq!(
        copied.balance_default.numeric().private,
        summary.balance_default.numeric().private
    );
}

#[test]
fn clone_quote_obeys_zero_budget_and_cancellation_before_native_tariffs() {
    let _scope = RECEIPT.scope();
    let summary = summary(100_000);
    reset();
    assert_eq!(
        summary.clone_with_payment(&mut WorkBudget::new(0)),
        Err(CloneError::Work)
    );
    assert_unobserved();
    assert_eq!(RECEIPT.get().quotes, 0);

    let cancellation = analysis_control::Cancellation::new();
    let mut budget = WorkBudget::new(usize::MAX);
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            summary.clone_with_payment(&mut budget),
            Err(CloneError::Cancelled)
        );
    });
    assert_unobserved();
    assert_eq!(RECEIPT.get().quotes, 0);
}

#[test]
fn cancellation_during_a_native_quote_prevents_every_copy() {
    let _scope = RECEIPT.scope();
    let mut summary = summary(64);
    let cancellation = analysis_control::Cancellation::new();
    let mut numeric = Word::expensive(64);
    numeric.cancel_price = Some(cancellation.clone());
    summary.balance_default = summary.balance_default.with_numeric(numeric);
    let original = summary.clone();
    let mut budget =
        analysis_control::with_cancellation(&cancellation, || WorkBudget::new(usize::MAX));
    reset();
    RECEIPT.update(|receipt| receipt.armed = true);
    assert_eq!(
        summary.clone_with_payment(&mut budget),
        Err(CloneError::Cancelled)
    );
    assert_unobserved();
    assert!(RECEIPT.get().quotes > 0);
    assert_eq!(summary, original);
}

#[test]
fn scalar_metadata_and_expression_identity_survive_paid_copy() {
    let _scope = RECEIPT.scope();
    let mut summary = summary(64);
    summary.balance_default = summary
        .balance_default
        .with_expression(ExprId::fresh().unwrap());
    let copied = summary
        .clone_with_payment(&mut WorkBudget::new(usize::MAX))
        .unwrap();
    assert_eq!(copied.balance_default, summary.balance_default);
    assert_eq!(
        copied.balance_default.expression(),
        summary.balance_default.expression()
    );
}
