use super::*;
use crate::{
    Fork,
    analysis::{ExecutionConfig, control::Control, progress::Observer, transfer},
    bytecode::Program,
    domain::Domain,
    world::{Account, Code, Entry, World},
};
use alloy_primitives::Address;

fn fixture(value: Option<u64>) -> (World, Entry, Arc<CodeCatalog>) {
    let program = Program::from_hex_with_fork("60026003016008565b00", Fork::Osaka).unwrap();
    let (catalog, _) = CodeCatalog::new(Fork::Osaka)
        .with_runtime(program.clone())
        .unwrap();
    let address = Address::repeat_byte(0x22);
    let mut account = Account::unknown();
    account.code = Code::Runtime(program);
    if let Some(value) = value {
        account
            .storage
            .insert(U256::ZERO, AbstractValue::constant(U256::from(value)));
    }
    let mut world = World::offline(Fork::Osaka, "memo state", "test");
    world.insert(address, account).unwrap();
    (world, Entry::new(address), Arc::new(catalog))
}

#[test]
fn hit_keeps_new_store_and_all_saved_checkpoints() {
    let (old_world, entry, catalog) = fixture(None);
    let config = ExecutionConfig::default();
    let domain = Domain::default();
    let input = transfer::initial(&old_world, &entry, domain).unwrap();
    let mut ledger = WorkBudget::new(config.max_work);
    let execution = transfer::execute(
        &old_world,
        &entry,
        input.clone(),
        &config,
        domain,
        &mut ledger,
        transfer::StorageReadPolicy::Abstract,
    );
    let mut reuse = ExecutionReuse::new(
        catalog,
        config.analysis.max_states,
        config.max_work,
        config.max_memory_bytes,
    );
    reuse.remember(&input, &execution, &mut ledger);
    assert_eq!(reuse.stats().entries, 1);
    let (new_world, _, _) = fixture(Some(3));
    let new = transfer::initial(&new_world, &entry, domain).unwrap();
    assert_ne!(input.store, new.store);
    let hit = reuse.lookup(&new, &mut ledger).unwrap().unwrap();
    assert_eq!(hit.payload.store, new.store);
    assert_eq!(hit.payload.active().saved_store, new.active().saved_store);
    assert_eq!(
        hit.payload.active().stack.last().unwrap().singleton(),
        Some(U256::from(5))
    );
    for successor in hit.successors {
        assert_eq!(successor.payload.store, new.store);
        assert_eq!(
            successor.payload.active().saved_store,
            new.active().saved_store
        );
    }
    let mut ordinary = transfer::execute(
        &new_world,
        &entry,
        new.clone(),
        &config,
        domain,
        &mut ledger,
        transfer::StorageReadPolicy::Abstract,
    );
    ordinary.payload.normalize();
    let cached = reuse.lookup(&new, &mut ledger).unwrap().unwrap();
    assert_eq!(cached.payload, ordinary.payload);
    assert_eq!(cached.executed_pcs, ordinary.executed_pcs);
    assert_eq!(cached.successors.len(), ordinary.successors.len());
    for (cached, mut ordinary) in cached.successors.into_iter().zip(ordinary.successors) {
        ordinary.payload.normalize();
        assert_eq!(cached.kind, ordinary.kind);
        assert_eq!(cached.payload, ordinary.payload);
    }
    assert!(reuse.retained_bytes <= reuse.max_retained_bytes);
}

#[test]
fn budget_and_current_cancellation_refuse_reuse_without_partial_effects() {
    let (world, entry, catalog) = fixture(None);
    let config = ExecutionConfig::default();
    let input = transfer::initial(&world, &entry, Domain::default()).unwrap();
    let before = input.clone();
    let mut reuse = ExecutionReuse::new(catalog, 4, config.max_work, config.max_memory_bytes);
    let mut preparation = WorkBudget::new(config.max_work);
    let execution = transfer::execute(
        &world,
        &entry,
        input.clone(),
        &config,
        Domain::default(),
        &mut preparation,
        transfer::StorageReadPolicy::Abstract,
    );
    reuse.remember(&input, &execution, &mut preparation);
    assert_eq!(reuse.stats().entries, 1);
    let before_stats = reuse.stats();
    let mut refused = WorkBudget::new(0);
    assert!(reuse.lookup(&input, &mut refused).is_err());
    assert_eq!(refused.used(), 0);
    assert_eq!(reuse.stats(), before_stats);
    let cancellation = Control::new(Observer::default());
    cancellation.cancellation().cancel();
    let mut ledger = WorkBudget::new(config.max_work);
    cancellation.scope(|| assert!(reuse.lookup(&input, &mut ledger).is_err()));
    assert_eq!(ledger.used(), 0);
    let captured = Control::new(Observer::default());
    let mut captured_ledger = captured.scope(|| WorkBudget::new(config.max_work));
    captured.cancellation().cancel();
    assert!(reuse.lookup(&input, &mut captured_ledger).is_err());
    assert_eq!(captured_ledger.used(), 0);
    assert_eq!(reuse.stats(), before_stats);
    assert_eq!(input, before);
}

#[test]
fn optional_retention_cap_refuses_entry_without_changing_execution() {
    let (world, entry, catalog) = fixture(None);
    let config = ExecutionConfig::default();
    let input = transfer::initial(&world, &entry, Domain::default()).unwrap();
    let mut ledger = WorkBudget::new(config.max_work);
    let execution = transfer::execute(
        &world,
        &entry,
        input.clone(),
        &config,
        Domain::default(),
        &mut ledger,
        transfer::StorageReadPolicy::Abstract,
    );
    let original_stack = execution.payload.active().stack.clone();
    let mut reuse = ExecutionReuse::new(catalog, 4, config.max_work, 0);
    reuse.remember(&input, &execution, &mut ledger);
    assert_eq!(reuse.stats().entries, 0);
    assert_eq!(reuse.retained_bytes, 0);
    assert_eq!(execution.payload.active().stack, original_stack);
    assert!(!ledger.exhausted());
}
