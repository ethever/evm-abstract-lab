use super::*;
use crate::{
    Fork,
    analysis::{
        ExecutionConfig,
        transfer::{StorageReadPolicy, WorkBudget, execute},
    },
    domain::symbolic::ExprId,
    world::Account,
};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

fn entered_call(
    value_code: &str,
    value: AbstractValue,
    balance: AbstractValue,
) -> (MachinePayload, MachinePayload) {
    let owner = address(0x101);
    let target = address(0x200);
    let code = format!("5f5f5f5f{value_code}610200620f4240f100");
    let mut world = World::new(Fork::Osaka, "call balance identity");
    let mut caller = Account::from_hex(&code, world.fork()).unwrap();
    caller.balance = balance;
    world.insert(owner, caller).unwrap();
    let mut callee = Account::from_hex("00", world.fork()).unwrap();
    callee.balance = word(50);
    world.insert(target, callee).unwrap();
    let mut entry = Entry::new(owner);
    entry.environment.value = value;
    let config = ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let input = initial(&world, &entry, domain).unwrap();
    let result = execute(
        &world,
        &entry,
        input.clone(),
        &config,
        domain,
        &mut WorkBudget::new(config.max_work),
        StorageReadPolicy::Abstract,
    );
    assert!(result.frontiers.is_empty());
    let child = result
        .successors
        .into_iter()
        .find(|successor| successor.kind == MachineEdgeKind::Call)
        .expect("CALL must actually enter the known callee")
        .payload;
    assert_eq!(child.call_stack.depth(), 2);
    assert!(child.active().saved_store.state() == &input.store);
    (input, child)
}

#[test]
fn zero_call_preserves_complete_balances_and_the_rollback_store() {
    let symbolic_balance = AbstractValue::unsigned_range(U256::from(100), U256::from(200))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    for balance in [word(100), symbolic_balance] {
        let (input, child) = entered_call("5f", word(0), balance);
        assert!(
            child.store == input.store,
            "zero transfer must preserve the full store, including source and expression metadata"
        );
        for account in [address(0x101), address(0x200)] {
            assert_eq!(
                child.store.read_balance(account),
                input.store.read_balance(account)
            );
        }
    }
}

#[test]
fn positive_and_uncertain_call_values_still_move_balances() {
    let (input, child) = entered_call("6003", word(0), word(100));
    assert_ne!(child.store, input.store);
    assert_eq!(
        child.store.read_balance(address(0x101)).singleton(),
        Some(U256::from(97))
    );
    assert_eq!(
        child.store.read_balance(address(0x200)).singleton(),
        Some(U256::from(53))
    );
    let domain = Domain::default();
    let amount = domain.join(&word(0), &word(3));
    let (_, child) = entered_call("34", amount, word(100));
    assert_eq!(
        child.store.read_balance(address(0x101)).constants(),
        domain.join(&word(97), &word(100)).constants()
    );
    assert_eq!(
        child.store.read_balance(address(0x200)).constants(),
        domain.join(&word(50), &word(53)).constants()
    );
}
