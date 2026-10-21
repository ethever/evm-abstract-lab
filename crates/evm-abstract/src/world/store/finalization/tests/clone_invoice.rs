//! Ordinary root staging/restore prices are distinct from retained footprints.

use super::super::*;
use crate::{
    Fork,
    domain::{EvmValue, ValueAlgebra, WordElement, provenance::Origin, symbolic::ExprId},
    resource::clone_payment::tests::{Policy, RECEIPT, Receipt, Word, reset},
    world::{EvmAccount, EvmWorld},
};
use alloy_primitives::U256;

const H: usize = 1_000_003;
fn owner() -> Address {
    Address::repeat_byte(0x31)
}

fn fixture(
    saved_only: bool,
    word: Word,
) -> (ValueAlgebra<Policy>, EvmStore<Word>, EvmSnapshot<Word>) {
    let policy = ValueAlgebra::new(Policy(crate::domain::Domain::default()));
    let mut world = EvmWorld::new(Fork::Osaka, "ordinary finalizer declared copy price");
    world
        .insert(owner(), EvmAccount::from_hex("00", Fork::Osaka).unwrap())
        .unwrap();
    let mut current = EvmStore::new_checked_with(&world, policy.borrowed()).unwrap();
    let mut saved = current.clone();
    let value = EvmValue::constant(U256::ZERO)
        .with_numeric(word)
        .with_expression(ExprId::fresh().unwrap())
        .with_origin(Origin::Storage)
        .with_identity(138, 27);
    if saved_only {
        saved.write_balance(owner(), value);
    } else {
        current.write_balance(owner(), value);
    }
    // Known untouched/nonempty owners avoid optional deletion/joins and unlisted
    // cleanup. Only the selected ordinary staging/restore copy owns private N.
    assert_eq!(current.touched_in_transaction(owner()), Some(false));
    assert_eq!(saved.touched_in_transaction(owner()), Some(false));
    (policy, current, EvmSnapshot::from_state(saved))
}

fn assert_invoice(saved_only: bool) {
    let _scope = RECEIPT.scope();
    let mut invoices = Vec::new();
    let mut quotes = Vec::new();
    for price in [H, H + 7001] {
        let (policy, input, saved) = fixture(saved_only, Word::expensive(price));
        let mut current = input.clone();
        let mut budget = WorkBudget::new(usize::MAX);
        assert!(budget.charge(37));
        reset();
        current
            .finalize_transaction_checked_with(
                policy.borrowed(),
                saved_only.then_some(&saved),
                &mut budget,
            )
            .unwrap();
        let receipt = RECEIPT.get();
        eprintln!(
            "ordinary saved_only={saved_only} price={price} private_bytes={} private_quotes={} fee={}",
            receipt.bytes,
            receipt.private_quotes,
            budget.used()
        );
        assert_eq!(receipt.bytes, if cfg!(feature = "imbl") { 0 } else { 4096 });
        let expected = if saved_only { saved.state() } else { &input };
        assert_eq!(
            current.balances.get(&owner()),
            expected.balances.get(&owner())
        );
        assert_eq!(current.code(owner()), expected.code(owner()));
        invoices.push(budget.used());
        quotes.push(receipt.private_quotes);
    }
    // Same retained representation and cleanup route, one changed declared
    // price and one physical std copy: retention cannot be a second owner.
    assert_eq!(invoices[1] - invoices[0], 7001);
    assert_eq!(quotes, [1, 1]);
}

#[test]
fn ordinary_staging_copy_has_one_declared_price_owner() {
    assert_invoice(false);
}
#[test]
fn ordinary_saved_restore_copy_has_one_declared_price_owner() {
    assert_invoice(true);
}

fn retained_prefix(current: &EvmStore<Word>, rollback: Option<&EvmSnapshot<Word>>) -> usize {
    // Independent original admission envelope, before either native copy. Its
    // complete payment remains consumed when a later declared clone refuses.
    let mut budget = WorkBudget::new(usize::MAX);
    assert!(budget.charge(37));
    let live = Footprint::measure(current, &mut budget).unwrap();
    let saved = rollback.map_or(live, |saved| {
        Footprint::measure(saved.state(), &mut budget).unwrap()
    });
    reserve(
        &mut budget,
        tree_work(live.fields + saved.fields + 2) * 8
            + if rollback.is_some() {
                live.code_bytes * 4
            } else {
                0
            },
    )
    .unwrap();
    budget.used()
}

fn assert_unfunded_copy(saved_only: bool) {
    let _scope = RECEIPT.scope();
    let (policy, input, saved) = fixture(saved_only, Word::expensive(H));
    let rollback = saved_only.then_some(&saved);
    let prefix = retained_prefix(&input, rollback);
    let mut current = input.clone();
    let mut budget = WorkBudget::new(prefix + H - 1);
    assert!(budget.charge(37));
    reset();
    let result =
        current.finalize_transaction_checked_with(policy.borrowed(), rollback, &mut budget);
    let receipt = RECEIPT.get();
    eprintln!(
        "ordinary finite deficit saved_only={saved_only} prefix={prefix} remaining_before_copy={} actual_private_bytes={} private_quotes={} used={} result={result:?}",
        H - 1,
        receipt.bytes,
        receipt.private_quotes,
        budget.used()
    );
    assert_eq!(receipt.bytes, 0);
    assert_eq!(receipt.private_quotes, 1);
    assert_eq!(result, Err(FinalizationError::Work));
    assert_eq!(current, input);
    assert!(budget.used() > prefix && budget.used() < prefix + H - 1);
}

#[test]
fn ordinary_staging_refuses_finite_copy_after_retention_is_paid() {
    assert_unfunded_copy(false);
}
#[test]
fn ordinary_saved_restore_refuses_finite_copy_after_retention_is_paid() {
    assert_unfunded_copy(true);
}

#[test]
fn ordinary_copy_prices_check_both_cancellation_owners_before_private_copy() {
    let _scope = RECEIPT.scope();
    for saved_only in [false, true] {
        for cancel_captured in [false, true] {
            let captured = analysis_control::Cancellation::new();
            let current_cancel = analysis_control::Cancellation::new();
            let mut word = Word::expensive(H);
            word.cancel_price = Some(if cancel_captured {
                captured.clone()
            } else {
                current_cancel.clone()
            });
            let (policy, input, saved) = fixture(saved_only, word);
            let mut current = input.clone();
            let mut budget =
                analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
            assert!(budget.charge(37));
            reset();
            RECEIPT.set(Receipt {
                armed: true,
                ..Receipt::default()
            });
            let result = analysis_control::with_cancellation(&current_cancel, || {
                current.finalize_transaction_checked_with(
                    policy.borrowed(),
                    saved_only.then_some(&saved),
                    &mut budget,
                )
            });
            assert_eq!((RECEIPT.get().bytes, RECEIPT.get().private_quotes), (0, 1));
            assert_eq!(result, Err(FinalizationError::Cancelled));
            assert_eq!(current, input);
            assert_eq!(captured.is_cancelled(), cancel_captured);
            assert_eq!(current_cancel.is_cancelled(), !cancel_captured);
            assert!(budget.used() > 37);
        }
    }
}

#[test]
fn ordinary_full_fee_minus_one_and_max_prices_keep_the_live_store_atomic() {
    let _scope = RECEIPT.scope();
    for saved_only in [false, true] {
        let (policy, input, saved) = fixture(saved_only, Word::expensive(H));
        let rollback = saved_only.then_some(&saved);
        let mut pilot = input.clone();
        let mut full = WorkBudget::new(usize::MAX);
        pilot
            .finalize_transaction_checked_with(policy.borrowed(), rollback, &mut full)
            .unwrap();
        let fee = full.used();
        for available in [fee - 1, fee] {
            let mut current = input.clone();
            let mut budget = WorkBudget::new(37 + available);
            assert!(budget.charge(37));
            reset();
            let result =
                current.finalize_transaction_checked_with(policy.borrowed(), rollback, &mut budget);
            if available == fee {
                assert_eq!(result, Ok(()));
                assert_eq!(current, pilot);
                assert_eq!(budget.used(), 37 + fee);
            } else {
                assert_eq!(result, Err(FinalizationError::Work));
                assert_eq!(current, input);
                assert!(budget.used() > 37 && budget.used() < 37 + available);
            }
        }
        let (policy, input, saved) = fixture(saved_only, Word::expensive(usize::MAX));
        let mut current = input.clone();
        reset();
        let result = current.finalize_transaction_checked_with(
            policy.borrowed(),
            saved_only.then_some(&saved),
            &mut WorkBudget::new(usize::MAX),
        );
        assert_eq!((RECEIPT.get().bytes, RECEIPT.get().private_quotes), (0, 1));
        assert_eq!(result, Err(FinalizationError::Work));
        assert_eq!(current, input);
    }
}
