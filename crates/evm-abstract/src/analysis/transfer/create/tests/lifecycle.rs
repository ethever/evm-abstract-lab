//! Native completion hook sequence for the central process adapter to reuse.
//! Store effects are prepared exit inputs; this does not execute a process body
//! or claim callback instruction evidence. The independent revm witness covers
//! an actually executed callback followed by constructor Return and outer Revert.

use crate::{
    Fork,
    analysis::{
        EvmChildFrame, EvmContinuation, MachineEdgeKind, OutcomeKind,
        testing::{Account, Entry, NumericLimits, TestValue, World, product},
        transfer::{calls, create::create, empty_execution, testing::procedure_context},
    },
    domain::NumericValue,
    resource::WorkBudget,
    world::{AddressInput, EvmByteArray, EvmLog, Existence, LogKey, LogSourceKind},
};
use alloy_primitives::{Address, U256};
use revm_bytecode::opcode;

fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}
fn value(value: u64) -> TestValue {
    TestValue::constant(U256::from(value))
}

#[test]
fn constructor_return_commits_native_store_then_outer_revert_restores_every_plane_with_sticky_touch()
 {
    let owner = address(0x101);
    let destination = owner.create(0);
    let destroyed = address(0x303);
    let ripemd = address(3);
    let mut world = World::new(Fork::Osaka, "constructor completion hook sequence");
    let mut account = Account::from_hex("f0602c6009555f5fa000", world.fork()).unwrap();
    account.balance = value(100);
    account.nonce = value(0);
    world.insert(owner, account).unwrap();
    for target in [destination, destroyed] {
        world.insert(target, Account::absent()).unwrap();
    }
    world.insert(ripemd, Account::empty()).unwrap();
    let domain = product(NumericLimits::default().spec());
    let config = crate::analysis::ExecutionConfig::default();
    let mut budget = WorkBudget::new(100_000_000);
    let input = calls::initial(&world, &Entry::new(owner), domain.borrowed()).unwrap();
    let program = input.active().program.clone().unwrap();
    let mut entered = empty_execution(input);
    create(
        &mut entered,
        &world,
        opcode::CREATE,
        &[value(7), value(0), value(0)],
        &program,
        &mut procedure_context(&config, &mut budget, domain.borrowed()),
        0,
    );
    assert!(entered.frontiers.is_empty(), "{:?}", entered.frontiers);
    let mut constructor = entered
        .successors
        .into_iter()
        .find(|edge| edge.kind == MachineEdgeKind::Call)
        .unwrap()
        .payload;
    assert_eq!(constructor.store.nonce(owner).singleton(), Some(U256::ONE));
    assert_eq!(
        constructor.store.read_balance(owner).singleton(),
        Some(U256::from(93))
    );
    assert_eq!(
        constructor.store.read_balance(destination).singleton(),
        Some(U256::from(7))
    );
    assert_eq!(
        constructor.store.raw_account_code(destination),
        Some(Vec::new())
    );
    let child_checkpoint = constructor.active().saved_store.state();
    assert_eq!(child_checkpoint.nonce(owner).singleton(), Some(U256::ONE));
    assert_eq!(child_checkpoint.existence(destination), Existence::Absent);

    // A prepared callback exit writes the actual creator. Finish it through the
    // shared native completion before committing the constructor itself.
    let mut callback = constructor.call_stack.root().state.clone();
    callback.key.caller = AddressInput::Concrete(destination);
    callback.key.basic_block_index = 1;
    callback.saved_store = constructor.store.snapshot();
    constructor.call_stack.push_child(EvmChildFrame {
        state: callback,
        continuation: EvmContinuation {
            return_block: Some(0),
            output_offset: value(0),
            output_size: value(0),
            creation: None,
        },
    });
    constructor
        .store
        .write(owner, &value(9), &value(44), domain.borrowed());
    constructor.store.touch_account(ripemd);
    let callback_key = LogKey {
        address: owner,
        code_address: owner,
        code_hash: constructor.active().key.code_hash,
        source_kind: LogSourceKind::Runtime,
        pc: 9,
    };
    constructor
        .store
        .emit_log(
            callback_key,
            EvmLog {
                topics: vec![value(0xcc)],
                data: EvmByteArray::exact(&[0xcc]),
            },
            domain.borrowed(),
        )
        .unwrap();
    let mut returned_callback = empty_execution(constructor.clone());
    calls::finish(
        &mut returned_callback,
        constructor,
        OutcomeKind::Return,
        EvmByteArray::empty(),
        &mut procedure_context(&config, &mut budget, domain.borrowed()),
        0,
    );
    assert!(returned_callback.frontiers.is_empty());
    assert_eq!(
        returned_callback.completed_calls[0].output_kind,
        OutcomeKind::Return
    );
    let mut constructor = returned_callback.successors.pop().unwrap().payload;
    assert_eq!(constructor.call_stack.depth(), 2);
    assert_eq!(
        constructor
            .store
            .read(owner, &value(9), domain.borrowed())
            .singleton(),
        Some(U256::from(44))
    );

    // Exit-state fixtures include constructor slots/logs and a transaction-created
    // account whose constructor SELFDESTRUCTed. A later deployment must retain
    // these flags instead of resetting the account or finalizing the transaction.
    constructor
        .store
        .write(destination, &value(0), &value(41), domain.borrowed());
    constructor
        .store
        .write_transient(destination, &value(1), &value(42), domain.borrowed());
    constructor
        .store
        .begin_creation_with_payment(destroyed, domain.borrowed(), &mut budget)
        .unwrap();
    constructor
        .store
        .selfdestruct(destroyed, destroyed, domain.borrowed());
    let init_key = LogKey {
        address: destination,
        code_address: destination,
        code_hash: constructor.active().key.code_hash,
        source_kind: LogSourceKind::Init,
        pc: 0,
    };
    constructor
        .store
        .emit_log(
            init_key,
            EvmLog {
                topics: vec![value(0xdd)],
                data: EvmByteArray::exact(&[0xdd]),
            },
            domain.borrowed(),
        )
        .unwrap();
    let mut deployed = empty_execution(constructor.clone());
    calls::finish(
        &mut deployed,
        constructor,
        OutcomeKind::Return,
        EvmByteArray::exact(&[0]),
        &mut procedure_context(&config, &mut budget, domain.borrowed()),
        0,
    );
    assert!(deployed.frontiers.is_empty(), "{:?}", deployed.frontiers);
    assert_eq!(deployed.completed_calls[0].kind, OutcomeKind::Return);
    assert_eq!(deployed.completed_calls[0].output_kind, OutcomeKind::Return);
    assert_eq!(
        deployed.completed_calls[0].output_data.len().singleton(),
        Some(U256::ZERO)
    );
    let parent = deployed.successors.pop().unwrap().payload;
    assert_eq!(parent.call_stack.depth(), 1);
    assert_eq!(
        parent.active().stack.last().unwrap().singleton(),
        Some(U256::from_be_slice(destination.as_slice()))
    );
    assert_eq!(
        parent.active().returndata.len().singleton(),
        Some(U256::ZERO)
    );
    assert_eq!(parent.store.nonce(owner).singleton(), Some(U256::ONE));
    assert_eq!(parent.store.nonce(destination).singleton(), Some(U256::ONE));
    assert_eq!(
        parent.store.read_balance(owner).singleton(),
        Some(U256::from(93))
    );
    assert_eq!(
        parent.store.read_balance(destination).singleton(),
        Some(U256::from(7))
    );
    assert_eq!(parent.store.raw_account_code(destination), Some(vec![0]));
    assert_eq!(
        parent
            .store
            .read(destination, &value(0), domain.borrowed())
            .singleton(),
        Some(U256::from(41))
    );
    assert_eq!(
        parent
            .store
            .read_transient(destination, &value(1), domain.borrowed())
            .singleton(),
        Some(U256::from(42))
    );
    assert_eq!(parent.store.created_in_transaction(destination), Some(true));
    assert_eq!(parent.store.created_in_transaction(destroyed), Some(true));
    assert_eq!(parent.store.pending_destruction(destroyed), Some(true));
    assert_eq!(parent.store.possible_logs().len(), 2);
    assert_eq!(parent.store.touched_in_transaction(ripemd), Some(true));

    let mut reverted = empty_execution(parent.clone());
    calls::finish(
        &mut reverted,
        parent,
        OutcomeKind::Revert,
        EvmByteArray::<NumericValue>::empty(),
        &mut procedure_context(&config, &mut budget, domain.borrowed()),
        0,
    );
    assert!(reverted.frontiers.is_empty());
    assert_eq!(reverted.outcomes.len(), 1);
    assert_eq!(reverted.outcomes[0].kind, OutcomeKind::Revert);
    let store = &reverted.outcomes[0].store;
    assert_eq!(store.nonce(owner).singleton(), Some(U256::ZERO));
    assert_eq!(store.read_balance(owner).singleton(), Some(U256::from(100)));
    assert_eq!(
        store.read(owner, &value(9), domain.borrowed()).singleton(),
        Some(U256::ZERO)
    );
    for target in [destination, destroyed] {
        assert_eq!(store.existence(target), Existence::Absent);
        assert_eq!(store.nonce(target).singleton(), Some(U256::ZERO));
        assert_eq!(store.read_balance(target).singleton(), Some(U256::ZERO));
        assert_eq!(store.raw_account_code(target), Some(Vec::new()));
        assert_eq!(
            store.read(target, &value(0), domain.borrowed()).singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(
            store
                .read_transient(target, &value(1), domain.borrowed())
                .singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(store.created_in_transaction(target), Some(false));
        assert_eq!(store.pending_destruction(target), Some(false));
    }
    assert!(store.possible_logs().is_empty());
    assert_eq!(
        store.touched_in_transaction(ripemd),
        Some(true),
        "raw snapshot assignment would lose this sticky touch"
    );
}
