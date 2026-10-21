use super::*;
use crate::analysis::MachinePayload;
use crate::domain::AbstractValue;
use crate::domain::Domain;
use crate::domain::ValueAlgebra;
use crate::world::Entry;
use crate::world::World;
use crate::{
    Fork,
    analysis::{
        ExecutionConfig,
        transfer::{StorageReadPolicy, WorkBudget, execute},
    },
    domain::symbolic::ExprId,
    world::Account,
};

mod copy_proof;
mod finish_clone;
mod range_proof;

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
    let input = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
    let result = execute(
        &world,
        &entry,
        input.clone(),
        &config,
        ValueAlgebra::new(&domain),
        &crate::analysis::RecentDistinctJumps,
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
    let mut before_call = input.store.clone();
    before_call.touch_account(owner);
    assert_eq!(child.active().saved_store.state(), &before_call);
    (input, child)
}

#[test]
fn zero_call_preserves_complete_balances_and_the_rollback_store() {
    let symbolic_balance = AbstractValue::unsigned_range(U256::from(100), U256::from(200))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    for balance in [word(100), symbolic_balance] {
        let (input, child) = entered_call("5f", word(0), balance);
        let mut expected = input.store.clone();
        expected.touch_account(address(0x101));
        expected.touch_account(address(0x200));
        assert_eq!(
            child.store, expected,
            "zero transfer changes only protocol touch facts"
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

#[test]
fn call_family_touches_only_entered_transfer_recipients_inside_their_checkpoint() {
    for (code, touches_target) in [
        ("5f5f5f5f5f61020062fffffff100", true),
        ("5f5f5f5f61020062fffffffa00", true),
        ("5f5f5f5f5f61020062fffffff200", false),
        ("5f5f5f5f61020062fffffff400", false),
    ] {
        let owner = address(0x101);
        let target = address(0x200);
        let mut world = World::new(Fork::Osaka, "issue134:call-touch");
        world
            .insert(owner, Account::from_hex(code, world.fork()).unwrap())
            .unwrap();
        world.insert(target, Account::empty()).unwrap();
        let mut entry = Entry::new(owner);
        entry.environment.value = word(0);
        let mut config = ExecutionConfig::default();
        config.analysis.relations.enabled = false;
        let domain = config.domain().unwrap();
        let input = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
        let mut before_call = input.store.clone();
        before_call.touch_account(owner);
        let result = execute(
            &world,
            &entry,
            input,
            &config,
            ValueAlgebra::new(&domain),
            &crate::analysis::RecentDistinctJumps,
            &mut WorkBudget::new(config.max_work),
            StorageReadPolicy::Abstract,
        );
        assert!(result.frontiers.is_empty());
        let failed = result
            .successors
            .iter()
            .find(|successor| successor.kind == MachineEdgeKind::Failure)
            .unwrap();
        assert_eq!(
            failed.payload.store, before_call,
            "failure before entry does not touch the callee"
        );
        let child = result
            .successors
            .iter()
            .find(|successor| successor.kind == MachineEdgeKind::Call)
            .unwrap();
        assert_eq!(child.payload.active().saved_store.state(), &before_call);
        let mut entered = before_call.clone();
        if touches_target {
            entered.touch_account(target);
        }
        assert_eq!(child.payload.store, entered);
        let mut reverted = child.payload.store.clone();
        reverted.restore(child.payload.active().saved_store.clone());
        assert_eq!(reverted, before_call);
    }
}

#[test]
fn target_remainder_retains_the_exact_guard_without_a_native_feasibility_query() {
    let (world, entry) = crate::analysis::machine::fixtures::fixture(&[(0x101, "00")]);
    let config = ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let payload = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
    let expression = ExprId::fresh().unwrap();
    let projected = AbstractValue::unknown_address().with_expression(expression.clone());
    let candidate = word(0x301);
    let mut result = super::super::empty_execution(payload.clone());
    let mut budget = WorkBudget::new(config.max_work);
    let mut context = TransferContext {
        config: &config,
        domain: ValueAlgebra::new(&domain),
        budget: &mut budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    };
    let remainder = target_guard(
        &mut result,
        &payload,
        &projected,
        &candidate,
        false,
        &mut context,
        7,
    )
    .expect("bounded retention needs no native feasibility query");
    assert!(result.frontiers.is_empty());
    assert_eq!(remainder.call_stack, payload.call_stack);
    assert_eq!(remainder.store, payload.store);
    assert_eq!(
        remainder
            .relations
            .can_equal(&expression, U256::from(0x301), &config.analysis.relations),
        crate::domain::relational::CheckResult::ProvenImpossible
    );
    assert_eq!(
        remainder
            .relations
            .can_equal(&expression, U256::from(0x302), &config.analysis.relations),
        crate::domain::relational::CheckResult::MayBeFeasible
    );
}
