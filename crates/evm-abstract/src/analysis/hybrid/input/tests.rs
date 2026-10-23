//! Independent native copy prices must be paid before retaining original input.

use super::*;
use crate::{
    Fork,
    domain::EvmValue,
    resource::clone_payment::tests::{RECEIPT, Receipt, Word},
    world::{EvmAccount, EvmEnvironmentValues},
};

fn input(word: Word, in_environment: bool) -> (EvmWorld<Word>, EvmEntry<Word>) {
    let address = crate::Address::repeat_byte(0x11);
    let mut world = EvmWorld::new(Fork::Osaka, "hybrid:private-native-copy-invoice");
    let mut account = EvmAccount::unknown();
    let mut environment = EvmEnvironmentValues::default();
    if in_environment {
        environment.value = EvmValue::from_numeric(word);
    } else {
        account.balance = EvmValue::from_numeric(word);
    }
    world.insert(address, account).unwrap();
    (
        world,
        EvmEntry {
            address,
            environment,
        },
    )
}

#[test]
fn original_world_and_environment_copies_pay_actual_private_native_fee_before_clone() {
    let _scope = RECEIPT.scope();
    for environment in [false, true] {
        let (world, entry) = input(Word::expensive(1_000_000), environment);
        RECEIPT.set(Receipt::default());
        let mut denied = WorkBudget::new(20_000);
        assert!(matches!(
            copy(&world, &entry, &mut denied),
            Err(CloneError::Work)
        ));
        assert_eq!(RECEIPT.get().clones, 0);
        assert_eq!(RECEIPT.get().bytes, 0);
        assert!(denied.used() <= 20_000);
        RECEIPT.set(Receipt::default());
        let mut paid = WorkBudget::new(2_000_000);
        let (copied_world, copied_entry) = copy(&world, &entry, &mut paid).unwrap();
        assert!(paid.used() >= 1_000_000);
        assert_eq!(RECEIPT.get().bytes, 4096);
        let copied = if environment {
            &copied_entry.environment.value
        } else {
            &copied_world.accounts()[&entry.address].balance
        };
        assert_eq!(copied.numeric().private.len(), 4096);
        assert_eq!(copied.numeric().declared_clone, 1_000_000);
    }
}

#[test]
fn original_payer_cancellation_during_private_invoice_prevents_any_copy() {
    let _scope = RECEIPT.scope();
    let original = analysis_control::Cancellation::new();
    let mut word = Word::expensive(1_000_000);
    word.cancel_price = Some(original.clone());
    let (world, entry) = input(word, false);
    let mut budget = analysis_control::with_cancellation(&original, || WorkBudget::new(2_000_000));
    RECEIPT.set(Receipt {
        armed: true,
        ..Receipt::default()
    });
    assert!(matches!(
        copy(&world, &entry, &mut budget),
        Err(CloneError::Cancelled)
    ));
    assert_eq!(RECEIPT.get().clones, 0);
    assert_eq!(RECEIPT.get().bytes, 0);
    assert!(budget.exhausted());
}
