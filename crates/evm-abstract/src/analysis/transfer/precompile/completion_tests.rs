use crate::domain::ProductAlgebra;
//! Native CALL completion pays for each owned input, independently of retention.

use super::*;
use crate::domain::EvmNumericAlgebra as _;
use crate::{
    analysis::{
        EvmMachinePayload, ExecutionConfig, FrameCode, MachineEdgeKind, RecentDistinctJumps,
        transfer::{StorageReadPolicy, WorkBudget, initial},
    },
    domain::{ValueAlgebra, WordElement, provenance::Origin},
    resource::clone_payment::tests::{Policy, RECEIPT, Receipt, Word, reset},
    world::{EvmAccount, EvmEntry, EvmWorld},
};

type Payload = EvmMachinePayload<Word>;
type Bytes = EvmByteArray<Word>;
const H: usize = 1_000_003;
const PRIVATE_BYTES: usize = 4096;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn value(number: u64) -> EvmValue<Word> {
    EvmValue::constant(U256::from(number))
}

fn child(
    symbolic: bool,
    word: Word,
) -> (
    EvmWorld<Word>,
    EvmEntry<Word>,
    ExecutionConfig,
    ValueAlgebra<Policy>,
    Payload,
) {
    let mut world = EvmWorld::new(Fork::Osaka, "paid native child input");
    // Copy one input byte; write caller storage; CALL identity with value 3,
    // one input byte, and 32 requested output bytes at offset 32.
    let mut caller = EvmAccount::from_hex(
        "60015f5f3760075f556020602060015f600360045af100",
        Fork::Osaka,
    )
    .unwrap();
    caller.balance = value(100);
    world.insert(address(0x101), caller).unwrap();
    let mut callee = EvmAccount::empty();
    callee.balance = value(50);
    world.insert(address(4), callee).unwrap();
    let entry = EvmEntry::concrete(
        address(0x101),
        address(0x900),
        value(0),
        if symbolic {
            Bytes::unknown_checked().unwrap()
        } else {
            Bytes::exact(&[0x2a])
        },
    );
    let mut config = ExecutionConfig::default();
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.enabled = false;
    let policy = ValueAlgebra::new(Policy(ProductAlgebra::from_spec(config_numeric.spec())));
    let input = initial(&world, &entry, policy.borrowed()).unwrap();
    let entered = super::super::execute(
        &world,
        &entry,
        input,
        &config,
        policy.borrowed(),
        &RecentDistinctJumps,
        &mut WorkBudget::new(usize::MAX),
        StorageReadPolicy::Abstract,
    );
    assert!(entered.frontiers.is_empty(), "{:?}", entered.frontiers);
    let mut payload = entered
        .successors
        .into_iter()
        .find(|successor| successor.kind == MachineEdgeKind::Call)
        .unwrap()
        .payload;
    assert_eq!(payload.active().code, FrameCode::Precompile(address(4)));
    assert_eq!(payload.call_stack.depth(), 2);
    assert_eq!(
        payload.store.touched_in_transaction(address(3)),
        Some(false)
    );
    assert!(!root::applicable(&payload));
    let parent = payload.call_stack.iter_mut().next().unwrap();
    parent.stack.push(
        EvmValue::from_numeric(word)
            .with_expression(ExprId::fresh().unwrap())
            .with_origin(Origin::Environment)
            .with_identity(138, 7),
    );
    parent.key.stack_height = parent.stack.len();
    (world, entry, config, policy, payload)
}

fn run(
    world: &EvmWorld<Word>,
    entry: &EvmEntry<Word>,
    config: &ExecutionConfig,
    policy: &ValueAlgebra<Policy>,
    payload: Payload,
    budget: &mut WorkBudget,
) -> EvmExecution<Word> {
    super::super::execute(
        world,
        entry,
        payload,
        config,
        policy.borrowed(),
        &RecentDistinctJumps,
        budget,
        StorageReadPolicy::Abstract,
    )
}

fn assert_private(payload: &Payload, expected: &Payload) {
    let actual = &payload.call_stack.root().state.stack[0];
    let original = &expected.call_stack.root().state.stack[0];
    assert_eq!(actual.numeric(), original.numeric());
    assert_eq!(actual.expression(), original.expression());
    assert_eq!(actual.provenance(), original.provenance());
    assert!(!actual.numeric().contains(U256::from(7)));
    assert!(actual.numeric().contains(U256::from(8)));
}

#[test]
fn finite_native_child_refuses_next_input_copy_after_an_independently_paid_incoming_copy() {
    let _scope = RECEIPT.scope();
    for symbolic in [false, true] {
        let (world, entry, config, policy, payload) = child(symbolic, Word::expensive(H));
        let mut invoice = WorkBudget::new(usize::MAX);
        payload.pay_clone_with(&mut invoice).unwrap();
        let incoming_fee = invoice.used();
        assert!(incoming_fee >= H);
        // A finite H can pass an upstream input read. Its payment is never
        // credit for the next native-input copy; MAX would fail upstream.
        let remaining = H / 2;
        let prior = 37;
        let mut budget = WorkBudget::new(prior + incoming_fee + remaining);
        assert!(budget.charge(prior));
        reset();
        payload.pay_clone_with(&mut budget).unwrap();
        let owned_input = payload.clone();
        assert_eq!(RECEIPT.get().bytes, PRIVATE_BYTES);
        let execution = run(&world, &entry, &config, &policy, owned_input, &mut budget);
        let receipt = RECEIPT.get();
        assert_eq!(receipt.bytes, PRIVATE_BYTES, "symbolic={symbolic}");
        assert_eq!(
            receipt.private_quotes, 2,
            "the actual next input price was inspected"
        );
        assert_eq!(execution.payload, payload);
        assert!(
            execution.outcomes.is_empty()
                && execution.successors.is_empty()
                && execution.completed_calls.is_empty()
        );
        assert_eq!(execution.frontiers[0].reason, FrontierReason::Work);
        assert!(budget.used() > prior + incoming_fee);
        assert!(budget.used() < prior + incoming_fee + remaining);
    }
}

#[test]
fn native_input_price_checks_current_and_captured_cancellation_before_private_copy() {
    let _scope = RECEIPT.scope();
    for symbolic in [false, true] {
        for cancel_captured in [false, true] {
            let captured = analysis_control::Cancellation::new();
            let current = analysis_control::Cancellation::new();
            let mut word = Word::expensive(H);
            word.cancel_price = Some(if cancel_captured {
                captured.clone()
            } else {
                current.clone()
            });
            let (world, entry, config, policy, payload) = child(symbolic, word);
            let expected = payload.clone();
            let mut budget =
                analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
            reset();
            RECEIPT.set(Receipt {
                armed: true,
                ..Receipt::default()
            });
            let execution = analysis_control::with_cancellation(&current, || {
                run(&world, &entry, &config, &policy, payload, &mut budget)
            });
            let receipt = RECEIPT.get();
            assert_eq!((receipt.bytes, receipt.private_quotes), (0, 1));
            assert_eq!(captured.is_cancelled(), cancel_captured);
            assert_eq!(current.is_cancelled(), !cancel_captured);
            assert_eq!(execution.payload, expected);
            assert!(
                execution.outcomes.is_empty()
                    && execution.successors.is_empty()
                    && execution.completed_calls.is_empty()
            );
            assert_eq!(
                execution.frontiers[0].reason,
                FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
            );
        }
    }
}

#[test]
fn native_child_full_fee_tracks_actual_copies_and_one_short_publishes_no_partial_branch() {
    let _scope = RECEIPT.scope();
    for symbolic in [false, true] {
        let mut invoices = Vec::new();
        let alternatives = if symbolic { 3 } else { 1 };
        for price in [H, H + 7001] {
            let (world, entry, config, policy, payload) = child(symbolic, Word::expensive(price));
            let mut pilot = WorkBudget::new(usize::MAX);
            let full = run(
                &world,
                &entry,
                &config,
                &policy,
                payload.clone(),
                &mut pilot,
            );
            assert!(full.frontiers.is_empty(), "{:?}", full.frontiers);
            let fee = pilot.used();
            invoices.push(fee);
            for funded in [false, true] {
                let mut budget = WorkBudget::new(37 + fee - usize::from(!funded));
                assert!(budget.charge(37));
                let owned = payload.clone();
                reset();
                let execution = run(&world, &entry, &config, &policy, owned, &mut budget);
                let receipt = RECEIPT.get();
                assert_eq!(execution.payload, payload);
                if !funded {
                    // Earlier complete alternatives remain valid. The refused
                    // alternative publishes neither a receipt nor a successor.
                    assert_eq!(execution.completed_calls.len(), alternatives - 1);
                    assert_eq!(execution.successors.len(), alternatives - 1);
                    assert_eq!(execution.frontiers[0].reason, FrontierReason::Work);
                    assert!(budget.used() > 37 && budget.used() < 37 + fee);
                    continue;
                }
                assert_eq!(budget.used(), 37 + fee);
                assert!(execution.frontiers.is_empty());
                assert_eq!(execution.completed_calls.len(), alternatives);
                assert_eq!(execution.successors.len(), alternatives);
                assert_eq!(receipt.bytes, 2 * alternatives * PRIVATE_BYTES);
                assert_eq!(receipt.private_quotes, 2 * alternatives);
                assert!(execution.outcomes.is_empty());
                for completed in &execution.completed_calls {
                    assert_eq!(
                        completed.payload, payload,
                        "raw callee input remains captured"
                    );
                    assert_eq!(completed.payload.active().key.code_address, address(4));
                    assert_eq!(completed.kind, completed.output_kind);
                    assert_eq!(completed.data, completed.output_data);
                    if completed.kind == OutcomeKind::Return {
                        assert_eq!(completed.output_store, payload.store);
                        if symbolic {
                            assert_eq!(completed.data, payload.active().calldata);
                        } else {
                            assert_eq!(
                                completed.data.exact_bytes(policy.borrowed()),
                                Some(vec![0x2a])
                            );
                        }
                    } else {
                        assert_eq!(
                            &completed.output_store,
                            payload.active().saved_store.state()
                        );
                        if completed.kind == OutcomeKind::Failure {
                            assert_eq!(
                                completed.data.exact_bytes(policy.borrowed()),
                                Some(Vec::new())
                            );
                        }
                    }
                }
                for successor in &execution.successors {
                    assert_eq!(successor.payload.call_stack.depth(), 1);
                    assert_eq!(successor.payload.active().key.code_address, address(0x101));
                    assert_eq!(successor.payload.active().key.basic_block_index, 1);
                    assert_private(&successor.payload, &payload);
                }
            }
        }
        // Same retained predicate and containers, only the declared price
        // changes: one input + one completed capture per native alternative.
        assert_eq!(invoices[1] - invoices[0], 2 * alternatives * 7001);
    }
}

#[test]
fn identity_success_buffer_pays_its_own_native_calldata_copy_before_completion_inputs() {
    let _scope = RECEIPT.scope();
    for cancel_captured in [None, Some(false), Some(true)] {
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let (world, entry, config, policy, mut payload) = child(true, Word::top());
        let mut word = Word::constant(U256::MAX).with_predicate(H);
        word.cancel_price = cancel_captured.map(|captured_owner| {
            if captured_owner {
                captured.clone()
            } else {
                current.clone()
            }
        });
        let size = EvmValue::from_numeric(word).with_expression(ExprId::constant(U256::MAX));
        // A retained literal proves a length that cannot be materialized.
        // Its private native predicate survives in the owned calldata metadata;
        // reaching the summary requires no expensive singleton discovery.
        payload.active_mut().calldata = Bytes::unknown_checked()
            .unwrap()
            .slice(&value(0), &size, policy.borrowed())
            .unwrap();
        let expected = payload.clone();
        let mut budget = analysis_control::with_cancellation(&captured, || {
            WorkBudget::new(if cancel_captured.is_some() {
                usize::MAX
            } else {
                H / 2
            })
        });
        reset();
        RECEIPT.set(Receipt {
            armed: true,
            ..Receipt::default()
        });
        let execution = analysis_control::with_cancellation(&current, || {
            run(&world, &entry, &config, &policy, payload, &mut budget)
        });
        assert_eq!(RECEIPT.get().bytes, 0);
        if cancel_captured.is_some() {
            assert_eq!(RECEIPT.get().private_quotes, 1);
        } else {
            assert!(RECEIPT.get().private_quotes > 0);
        }
        assert_eq!(execution.payload, expected);
        assert!(
            execution.outcomes.is_empty()
                && execution.completed_calls.is_empty()
                && execution.successors.is_empty()
        );
        assert_eq!(
            execution.frontiers[0].reason,
            if cancel_captured.is_some() {
                FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
            } else {
                FrontierReason::Work
            }
        );
    }
}

#[test]
fn exact_native_relative_root_pays_only_an_actual_saved_store_rollback() {
    let _scope = RECEIPT.scope();
    for target in [4, 9] {
        for price in [H, usize::MAX] {
            let mut world = EvmWorld::new(Fork::Osaka, "native relative root rollback");
            world.insert(address(0x303), EvmAccount::empty()).unwrap();
            let entry =
                EvmEntry::concrete(address(target), address(0x900), value(0), Bytes::empty());
            let config = ExecutionConfig::default();
            let mut config_numeric = crate::analysis::testing::NumericLimits::default();
            let policy =
                ValueAlgebra::new(Policy(ProductAlgebra::from_spec(config_numeric.spec())));
            let mut payload = initial(&world, &entry, policy.borrowed()).unwrap();
            let mut saved = payload.store.clone();
            saved.write_balance(
                address(0x303),
                EvmValue::from_numeric(Word::expensive(price)),
            );
            payload.active_mut().saved_store = crate::world::EvmSnapshot::from_state(saved);
            let expected = payload.clone();
            let mut budget = WorkBudget::new(usize::MAX);
            reset();
            let execution = super::super::execute_inner(
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
            assert_eq!(execution.payload, expected);
            assert!(execution.successors.is_empty() && execution.completed_calls.is_empty());
            if target == 4 {
                // Identity succeeds with empty output: the historical private
                // checkpoint remains a handle and has no native copy invoice.
                assert_eq!((receipt.bytes, receipt.private_quotes), (0, 0));
                assert!(execution.frontiers.is_empty());
                assert_eq!(execution.outcomes.len(), 1);
                assert_eq!(execution.outcomes[0].kind, OutcomeKind::Return);
                assert_eq!(execution.outcomes[0].store, expected.store);
            } else if price == usize::MAX {
                // Empty Blake2F input really halts. Unlike a private value in
                // the input payload, a saved-only MAX price passes handle copies.
                assert_eq!((receipt.bytes, receipt.private_quotes), (0, 1));
                assert!(execution.outcomes.is_empty());
                assert_eq!(execution.frontiers[0].reason, FrontierReason::Work);
            } else {
                assert_eq!(receipt.private_quotes, 1);
                assert_eq!(receipt.bytes, if cfg!(feature = "imbl") { 0 } else { 4096 });
                assert!(execution.frontiers.is_empty());
                assert_eq!(execution.outcomes.len(), 1);
                assert_eq!(execution.outcomes[0].kind, OutcomeKind::Failure);
                assert_eq!(
                    &execution.outcomes[0].store,
                    expected.active().saved_store.state()
                );
                assert_eq!(
                    execution.outcomes[0].data.exact_bytes(policy.borrowed()),
                    Some(Vec::new())
                );
            }
        }
    }
}

#[test]
fn symbolic_native_relative_root_restores_are_paid_only_by_the_common_owner() {
    let _scope = RECEIPT.scope();
    let mut invoices = Vec::new();
    for price in [H, H + 7001, usize::MAX] {
        let mut world = EvmWorld::new(Fork::Osaka, "symbolic native common restore");
        world.insert(address(0x303), EvmAccount::empty()).unwrap();
        let entry = EvmEntry::concrete(address(6), address(0x900), value(0), Bytes::unknown());
        let config = ExecutionConfig::default();
        let mut config_numeric = crate::analysis::testing::NumericLimits::default();
        let policy = ValueAlgebra::new(Policy(ProductAlgebra::from_spec(config_numeric.spec())));
        let mut payload = initial(&world, &entry, policy.borrowed()).unwrap();
        let mut saved = payload.active().saved_store.state().clone();
        saved.write_balance(
            address(0x303),
            EvmValue::from_numeric(Word::expensive(price)),
        );
        payload.active_mut().saved_store = crate::world::EvmSnapshot::from_state(saved);
        let expected = payload.clone();
        let mut budget = WorkBudget::new(usize::MAX);
        reset();
        let execution = super::super::execute_inner(
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
        assert_eq!(execution.payload, expected);
        assert!(execution.successors.is_empty() && execution.completed_calls.is_empty());
        assert_eq!(execution.outcomes[0].kind, OutcomeKind::Return);
        assert_eq!(execution.outcomes[0].store, expected.store);
        if price == usize::MAX {
            assert_eq!((receipt.bytes, receipt.private_quotes), (0, 1));
            assert_eq!(execution.outcomes.len(), 1);
            assert_eq!(execution.frontiers[0].reason, FrontierReason::Work);
        } else {
            assert_eq!(receipt.private_quotes, 2);
            assert_eq!(
                receipt.bytes,
                if cfg!(feature = "imbl") { 0 } else { 2 * 4096 }
            );
            assert!(execution.frontiers.is_empty());
            assert_eq!(
                execution
                    .outcomes
                    .iter()
                    .map(|outcome| outcome.kind)
                    .collect::<Vec<_>>(),
                [
                    OutcomeKind::Return,
                    OutcomeKind::Revert,
                    OutcomeKind::Failure
                ]
            );
            for outcome in &execution.outcomes[1..] {
                assert_eq!(&outcome.store, expected.active().saved_store.state());
            }
            invoices.push(budget.used());
        }
    }
    assert_eq!(invoices[1] - invoices[0], 2 * 7001);
}
