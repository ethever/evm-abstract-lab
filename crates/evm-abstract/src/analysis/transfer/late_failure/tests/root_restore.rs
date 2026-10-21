//! Relative root rollback is owned by the common termination boundary.

use super::super::*;
use crate::domain::ProductAlgebra;
use crate::{
    Fork,
    analysis::{
        EvmMachinePayload, ExecutionConfig, RecentDistinctJumps,
        transfer::{StorageReadPolicy, execute_inner, initial},
    },
    domain::{EvmValue, ValueAlgebra, WordElement, provenance::Origin, symbolic::ExprId},
    resource::clone_payment::tests::{Policy, RECEIPT, Receipt, Word, reset},
    world::{EvmAccount, EvmEntry, EvmSnapshot, EvmWorld},
};

const H: usize = 1_000_003;
type Payload = EvmMachinePayload<Word>;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}
fn value(number: u64) -> EvmValue<Word> {
    EvmValue::constant(U256::from(number))
}

fn fixture(
    code: &str,
    word: Word,
) -> (
    EvmWorld<Word>,
    EvmEntry<Word>,
    ExecutionConfig,
    ValueAlgebra<Policy>,
    Payload,
) {
    let mut world = EvmWorld::new(Fork::Osaka, "common procedure root restore");
    world
        .insert(
            address(0x101),
            EvmAccount::from_hex(code, Fork::Osaka).unwrap(),
        )
        .unwrap();
    world.insert(address(0x303), EvmAccount::empty()).unwrap();
    let entry = EvmEntry::concrete(
        address(0x101),
        address(0x900),
        value(0),
        EvmByteArray::empty(),
    );
    let config = ExecutionConfig::default();
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.enabled = false;
    let policy = ValueAlgebra::new(Policy(ProductAlgebra::from_spec(config_numeric.spec())));
    let mut payload = initial(&world, &entry, policy.borrowed()).unwrap();
    assert_eq!(
        payload.store.touched_in_transaction(address(3)),
        Some(false)
    );
    let mut saved = payload.store.clone();
    saved.write_balance(
        address(0x303),
        value(0)
            .with_numeric(word)
            .with_expression(ExprId::fresh().unwrap())
            .with_origin(Origin::Storage)
            .with_identity(138, 17),
    );
    payload.active_mut().saved_store = EvmSnapshot::from_state(saved);
    (world, entry, config, policy, payload)
}

#[test]
fn non_native_procedure_root_pays_one_actual_restore_and_skips_it_on_return() {
    let _scope = RECEIPT.scope();
    for (code, kind) in [
        ("00", OutcomeKind::Return),
        ("5f5ffd", OutcomeKind::Revert),
        ("fe", OutcomeKind::Failure),
    ] {
        // REVERT's memory operation also retains the ordinary gas-failure
        // alternative. These are two real root restores, each paid once.
        let restores = if kind == OutcomeKind::Revert { 2 } else { 1 };
        let mut invoices = Vec::new();
        for price in [H, H + 7001, usize::MAX] {
            let (world, entry, config, policy, payload) = fixture(code, Word::expensive(price));
            let expected = payload.clone();
            let mut budget = WorkBudget::new(usize::MAX);
            assert!(budget.charge(37));
            reset();
            let execution = execute_inner(
                &world,
                &entry,
                payload,
                &config,
                policy.borrowed(),
                &RecentDistinctJumps,
                &mut budget,
                StorageReadPolicy::Abstract,
                true,
            );
            let receipt = RECEIPT.get();
            assert_eq!(execution.payload.store, expected.store);
            assert_eq!(
                execution.payload.active().saved_store,
                expected.active().saved_store
            );
            assert!(execution.successors.is_empty() && execution.completed_calls.is_empty());
            if kind == OutcomeKind::Return {
                assert_eq!((receipt.bytes, receipt.private_quotes), (0, 0));
                assert!(execution.frontiers.is_empty());
                assert_eq!(execution.outcomes.len(), 1);
                assert_eq!(execution.outcomes[0].kind, kind);
                assert_eq!(execution.outcomes[0].store, expected.store);
            } else if price == usize::MAX {
                assert_eq!((receipt.bytes, receipt.private_quotes), (0, 1));
                assert!(execution.outcomes.is_empty());
                assert_eq!(execution.frontiers[0].reason, FrontierReason::Work);
                assert!(budget.used() > 37);
            } else {
                assert_eq!(receipt.private_quotes, restores);
                assert_eq!(
                    receipt.bytes,
                    if cfg!(feature = "imbl") {
                        0
                    } else {
                        restores * 4096
                    }
                );
                assert!(execution.frontiers.is_empty(), "{:?}", execution.frontiers);
                let kinds = execution
                    .outcomes
                    .iter()
                    .map(|outcome| outcome.kind)
                    .collect::<Vec<_>>();
                assert_eq!(
                    kinds,
                    if kind == OutcomeKind::Revert {
                        vec![OutcomeKind::Failure, OutcomeKind::Revert]
                    } else {
                        vec![OutcomeKind::Failure]
                    }
                );
                for outcome in &execution.outcomes {
                    assert_eq!(&outcome.store, expected.active().saved_store.state());
                }
                let balance = execution.outcomes[0].store.read_balance(address(0x303));
                assert!(!balance.numeric().contains(U256::from(7)));
                assert!(balance.numeric().contains(U256::from(8)));
            }
            if price != usize::MAX {
                invoices.push(budget.used());
            }
        }
        assert_eq!(
            invoices[1] - invoices[0],
            if kind == OutcomeKind::Return {
                0
            } else {
                restores * 7001
            }
        );
    }
}

#[test]
fn common_procedure_root_restore_quote_checks_current_and_captured_cancellation() {
    let _scope = RECEIPT.scope();
    for code in ["5f5ffd", "fe"] {
        for cancel_captured in [false, true] {
            let captured = analysis_control::Cancellation::new();
            let current = analysis_control::Cancellation::new();
            let mut word = Word::expensive(H);
            word.cancel_price = Some(if cancel_captured {
                captured.clone()
            } else {
                current.clone()
            });
            let (world, entry, config, policy, payload) = fixture(code, word);
            let expected = payload.clone();
            let mut budget =
                analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
            assert!(budget.charge(37));
            reset();
            RECEIPT.set(Receipt {
                armed: true,
                ..Receipt::default()
            });
            let execution = analysis_control::with_cancellation(&current, || {
                execute_inner(
                    &world,
                    &entry,
                    payload,
                    &config,
                    policy.borrowed(),
                    &RecentDistinctJumps,
                    &mut budget,
                    StorageReadPolicy::Abstract,
                    true,
                )
            });
            assert_eq!((RECEIPT.get().bytes, RECEIPT.get().private_quotes), (0, 1));
            assert_eq!(execution.payload.store, expected.store);
            assert_eq!(
                execution.payload.active().saved_store,
                expected.active().saved_store
            );
            assert!(
                execution.outcomes.is_empty()
                    && execution.successors.is_empty()
                    && execution.completed_calls.is_empty()
            );
            assert_eq!(
                execution.frontiers[0].reason,
                FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
            );
            assert_eq!(captured.is_cancelled(), cancel_captured);
            assert_eq!(current.is_cancelled(), !cancel_captured);
            assert!(budget.used() > 37);
        }
    }
}
