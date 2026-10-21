//! Each exceptional completion has one owner for every actual native copy.

use super::super::*;
use crate::domain::ProductAlgebra;
use crate::{
    Fork,
    analysis::{
        EvmContinuation, EvmMachinePayload, ExecutionConfig, MachineEdgeKind,
        transfer::{empty_execution, initial},
    },
    domain::{EvmValue, ValueAlgebra, WordElement, provenance::Origin, symbolic::ExprId},
    resource::clone_payment::tests::{Policy, RECEIPT, Receipt, Word, reset},
    world::{EvmAccount, EvmEntry, EvmWorld},
};

type Payload = EvmMachinePayload<Word>;
const H: usize = 1_000_003;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}
fn value(number: u64) -> EvmValue<Word> {
    EvmValue::constant(U256::from(number))
}

fn child(word: Word) -> (ExecutionConfig, ValueAlgebra<Policy>, Payload) {
    let mut world = EvmWorld::new(Fork::Osaka, "late failure native copy invoice");
    for owner in [0x101, 0x202] {
        world
            .insert(
                address(owner),
                EvmAccount::from_hex("00", Fork::Osaka).unwrap(),
            )
            .unwrap();
    }
    let config = ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let policy = ValueAlgebra::new(Policy(ProductAlgebra::from_spec(config_numeric.spec())));
    let mut payload = initial(
        &world,
        &EvmEntry::concrete(
            address(0x101),
            address(0x900),
            value(0),
            EvmByteArray::empty(),
        ),
        policy.borrowed(),
    )
    .unwrap();
    let state = initial(
        &world,
        &EvmEntry::concrete(
            address(0x202),
            address(0x101),
            value(0),
            EvmByteArray::empty(),
        ),
        policy.borrowed(),
    )
    .unwrap();
    let mut nested = state.call_stack.root().clone().into_child(EvmContinuation {
        return_block: Some(0),
        output_offset: value(0),
        output_size: value(0),
        creation: None,
    });
    nested.state.saved_store = payload.store.snapshot();
    payload.active_mut().stack.push(
        EvmValue::from_numeric(word)
            .with_expression(ExprId::fresh().unwrap())
            .with_origin(Origin::Environment)
            .with_identity(138, 7),
    );
    payload.call_stack.push_child(nested);
    payload.store.touch_account(address(3));
    payload.store.write_balance(address(0x202), value(9));
    (config, policy, payload)
}

fn run(
    payload: Payload,
    candidate: Option<&Payload>,
    config: &ExecutionConfig,
    policy: &ValueAlgebra<Policy>,
    budget: &mut WorkBudget,
) -> (bool, EvmExecution<Word>) {
    let mut result = empty_execution(payload);
    let mut context =
        crate::analysis::transfer::testing::context(config, budget, policy.borrowed());
    let accepted = if let Some(candidate) = candidate {
        after_selfdestruct(&mut result, candidate, &mut context, 7)
    } else {
        at_block_entry(&mut result, &mut context)
    };
    (accepted, result)
}

fn assert_single_copy_invoice(selfdestruct: bool) {
    let _scope = RECEIPT.scope();
    let mut invoices = Vec::new();
    let mut ancillary_quotes = Vec::new();
    let mut copy_quotes = Vec::new();
    for price in [H, H + 7001] {
        let (config, policy, payload) = child(Word::expensive(price));
        // Ancillary reservation must not quote or invoice the private N.
        let mut ancillary = WorkBudget::new(usize::MAX);
        reset();
        reserve_failure(&payload, false, &mut ancillary).unwrap();
        ancillary_quotes.push(RECEIPT.get().private_quotes);
        let working = payload.clone();
        let mut budget = WorkBudget::new(usize::MAX);
        reset();
        let (accepted, result) = run(
            working,
            selfdestruct.then_some(&payload),
            &config,
            &policy,
            &mut budget,
        );
        let receipt = RECEIPT.get();
        assert_eq!(receipt.bytes, 2 * 4096);
        copy_quotes.push(receipt.private_quotes);
        assert!(accepted);
        assert!(result.frontiers.is_empty());
        assert_eq!(result.payload, payload);
        assert_eq!(result.completed_calls.len(), 1);
        assert_eq!(result.successors.len(), 1);
        assert!(result.outcomes.is_empty());
        let completed = &result.completed_calls[0];
        assert_eq!(completed.payload, payload);
        assert_eq!(completed.kind, OutcomeKind::Failure);
        assert_eq!(completed.output_kind, OutcomeKind::Failure);
        assert_eq!(
            completed.output_store.touched_in_transaction(address(3)),
            Some(true)
        );
        assert_eq!(
            completed
                .output_store
                .read_balance(address(0x202))
                .singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(
            completed.data.exact_bytes(policy.borrowed()),
            Some(Vec::new())
        );
        assert_eq!(
            completed.output_data.exact_bytes(policy.borrowed()),
            Some(Vec::new())
        );
        let successor = &result.successors[0];
        assert_eq!(successor.kind, MachineEdgeKind::Failure);
        assert_eq!(successor.payload.call_stack.depth(), 1);
        assert_eq!(successor.payload.store, completed.output_store);
        assert_eq!(
            successor.payload.active().stack[0].numeric(),
            payload.call_stack.root().state.stack[0].numeric()
        );
        assert_eq!(
            successor.payload.active().stack[0].provenance(),
            payload.call_stack.root().state.stack[0].provenance()
        );
        assert!(
            !successor.payload.active().stack[0]
                .numeric()
                .contains(U256::from(7))
        );
        assert_eq!(
            successor.payload.active().stack.last().unwrap().singleton(),
            Some(U256::ZERO)
        );
        invoices.push(budget.used());
    }
    // All other fees and container sizes are identical. The price slope
    // independently derives the invoice from the observed two copies.
    eprintln!(
        "selfdestruct={selfdestruct}: physical_private_copies=2; invoices={invoices:?}; price_delta={}; ancillary_quotes={ancillary_quotes:?}; copy_quotes={copy_quotes:?}",
        invoices[1] - invoices[0]
    );
    assert_eq!(invoices[1] - invoices[0], 2 * 7001);
    assert_eq!(ancillary_quotes, [0, 0]);
    assert_eq!(copy_quotes, [2, 2]);
}

#[test]
fn block_entry_parent_predicate_is_invoiced_once_for_each_physical_copy() {
    assert_single_copy_invoice(false);
}

#[test]
fn selfdestruct_parent_predicate_is_invoiced_once_for_each_physical_copy() {
    assert_single_copy_invoice(true);
}

#[test]
fn late_failure_input_quote_honors_both_cancellation_owners_before_private_copy() {
    let _scope = RECEIPT.scope();
    for selfdestruct in [false, true] {
        for cancel_captured in [false, true] {
            let captured = analysis_control::Cancellation::new();
            let current = analysis_control::Cancellation::new();
            let mut word = Word::expensive(H);
            word.cancel_price = Some(if cancel_captured {
                captured.clone()
            } else {
                current.clone()
            });
            let (config, policy, payload) = child(word);
            let expected = payload.clone();
            let mut budget =
                analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
            reset();
            RECEIPT.set(Receipt {
                armed: true,
                ..Receipt::default()
            });
            let (accepted, result) = analysis_control::with_cancellation(&current, || {
                run(
                    payload,
                    selfdestruct.then_some(&expected),
                    &config,
                    &policy,
                    &mut budget,
                )
            });
            assert!(!accepted);
            assert_eq!(RECEIPT.get().bytes, 0);
            assert_eq!(RECEIPT.get().private_quotes, 1);
            assert_eq!(result.payload, expected);
            assert!(
                result.successors.is_empty()
                    && result.completed_calls.is_empty()
                    && result.outcomes.is_empty()
            );
            assert_eq!(
                result.frontiers[0].reason,
                FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
            );
            assert_eq!(captured.is_cancelled(), cancel_captured);
            assert_eq!(current.is_cancelled(), !cancel_captured);
        }
    }
}

#[test]
fn late_failure_keeps_paid_prefixes_and_refuses_before_an_unfunded_input_copy() {
    let _scope = RECEIPT.scope();
    for selfdestruct in [false, true] {
        let (config, policy, payload) = child(Word::expensive(H));
        let mut pilot = WorkBudget::new(usize::MAX);
        let (accepted, result) = run(
            payload.clone(),
            selfdestruct.then_some(&payload),
            &config,
            &policy,
            &mut pilot,
        );
        assert!(accepted && result.frontiers.is_empty());
        let fee = pilot.used();
        for available in [H / 2, fee - 1, fee] {
            let working = payload.clone();
            let mut budget = WorkBudget::new(37 + available);
            assert!(budget.charge(37));
            reset();
            let (accepted, result) = run(
                working,
                selfdestruct.then_some(&payload),
                &config,
                &policy,
                &mut budget,
            );
            let receipt = RECEIPT.get();
            assert_eq!(result.payload, payload);
            assert_eq!(accepted, available == fee);
            if accepted {
                assert_eq!(budget.used(), 37 + fee);
                assert_eq!(
                    (result.successors.len(), result.completed_calls.len()),
                    (1, 1)
                );
                assert_eq!(receipt.bytes, 2 * 4096);
                assert_eq!(receipt.private_quotes, 2);
            } else {
                assert!(
                    result.successors.is_empty()
                        && result.completed_calls.is_empty()
                        && result.outcomes.is_empty()
                );
                assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
                assert!(budget.used() > 37 && budget.used() < 37 + available);
                if available == H / 2 {
                    assert_eq!((receipt.bytes, receipt.private_quotes), (0, 1));
                }
            }
        }
    }
}

#[test]
fn relative_root_retains_the_saved_store_copy_owner_missing_from_child_finish() {
    let _scope = RECEIPT.scope();
    for price in [H, usize::MAX] {
        let (config, policy, mut payload) = child(Word::top());
        payload.call_stack.pop_child().unwrap();
        payload.active_mut().stack.clear();
        let mut saved = payload.store.clone();
        saved.write_balance(
            address(0x303),
            EvmValue::from_numeric(Word::expensive(price)),
        );
        payload.active_mut().saved_store = crate::world::EvmSnapshot::from_state(saved);
        let working = payload.clone();
        let mut result = empty_execution(working);
        let mut budget = WorkBudget::new(usize::MAX);
        reset();
        let accepted = at_block_entry(
            &mut result,
            &mut crate::analysis::transfer::testing::procedure_context(
                &config,
                &mut budget,
                policy.borrowed(),
            ),
        );
        let receipt = RECEIPT.get();
        assert_eq!(receipt.private_quotes, 1);
        assert_eq!(result.payload, payload);
        if price == usize::MAX {
            assert!(!accepted);
            assert_eq!(receipt.bytes, 0);
            assert!(
                result.outcomes.is_empty()
                    && result.successors.is_empty()
                    && result.completed_calls.is_empty()
            );
            assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
        } else {
            assert!(accepted && result.frontiers.is_empty());
            // imbl keeps the saved balance-map entries shared; std owns them.
            assert_eq!(receipt.bytes, if cfg!(feature = "imbl") { 0 } else { 4096 });
            assert_eq!(result.outcomes.len(), 1);
            assert_eq!(result.outcomes[0].kind, OutcomeKind::Failure);
            assert_eq!(
                &result.outcomes[0].store,
                payload.active().saved_store.state()
            );
        }
    }
}
