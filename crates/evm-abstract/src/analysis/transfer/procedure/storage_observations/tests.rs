use super::super::{
    Counters, Solver, StorageReadPolicy, Table, WorkBudget, form, instantiate, join, semantic_eq,
};
use super::*;
use crate::analysis::{
    ExecutionConfig,
    machine::fixtures::{fixture, word},
};
use crate::domain::symbolic::ExprId;
use alloy_primitives::U256;

fn written_input() -> (
    crate::world::World,
    crate::world::Entry,
    MachinePayload,
    Domain,
) {
    let domain = Domain::default();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input = super::super::super::initial(&world, &entry, domain).unwrap();
    let key = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    input.active_mut().stack = vec![key.clone()];
    let owner = input.active().key.address;
    input
        .store
        .write_checked(owner, &key, &word(7), domain)
        .unwrap();
    (world, entry, input, domain)
}

#[test]
fn missing_observation_drops_on_join_and_changes_semantics() {
    let (_, entry, input, domain) = written_input();
    let left = form(&input, &entry, domain).unwrap();
    assert_eq!(left.storage_observations.0.len(), 1);
    assert_eq!(
        left.storage_observations.0[0].numeric.singleton(),
        Some(U256::from(7))
    );
    let mut missing = left.clone();
    missing.storage_observations = Default::default();
    assert!(!semantic_eq(&left, &missing));
    for widen in [false, true] {
        let joined = join(&left, &missing, domain, widen).unwrap();
        assert!(joined.storage_observations.0.is_empty());
        assert!(!semantic_eq(&left, &joined));
        assert!(semantic_eq(&missing, &joined));
        assert!(
            join(&missing, &left, domain, widen)
                .unwrap()
                .storage_observations
                .0
                .is_empty()
        );
    }
}

#[test]
fn joined_finite_read_is_recovered_from_the_exact_instantiated_root() {
    let (_, entry, input, domain) = written_input();
    let left = form(&input, &entry, domain).unwrap();
    let mut next = input.clone();
    let key = next.active().stack[0].clone();
    next.store
        .write_checked(next.active().key.address, &key, &word(9), domain)
        .unwrap();
    let right = form(&next, &entry, domain).unwrap();
    let joined = join(&left, &right, domain, false).unwrap();
    assert_eq!(
        joined.storage_observations.0[0].numeric.constants(),
        Some(&[U256::from(7), U256::from(9)].into())
    );
    let restored = instantiate(&joined, domain).unwrap();
    let again = form(&restored, &entry, domain).unwrap();
    assert_eq!(again.storage_observations, joined.storage_observations);
}

#[test]
fn observation_cost_ignores_code_and_caps_optional_root_work() {
    let (_, _, mut input, domain) = written_input();
    let fee = form_work(&input, domain);
    let store_work = input.store.work_size();
    input.store.deploy_code(
        crate::analysis::machine::fixtures::address(0x202),
        crate::bytecode::Program::from_hex(&"00".repeat(24_576)).unwrap(),
    );
    assert!(input.store.work_size() >= store_work + 24_576);
    assert_eq!(form_work(&input, domain), fee);
    let key = input.active().stack[0].clone();
    input.active_mut().stack = vec![AbstractValue::top(); MAX_POSITIONS + 2];
    input.active_mut().stack[0] = key.clone();
    input.active_mut().stack[MAX_POSITIONS + 1] = key;
    let observations = StorageObservations::form(&input, domain).unwrap();
    assert_eq!(observations.0.len(), 1);
    assert_eq!(observations.0[0].stack_position, MAX_POSITIONS + 1);
    for _ in 0..32 {
        let key = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
        input
            .store
            .write_checked(input.active().key.address, &key, &word(9), domain)
            .unwrap();
    }
    assert!(
        input
            .store
            .slot_root(input.active().key.address, false)
            .unwrap()
            .nodes()
            > MAX_ROOT_WORK
    );
    assert_eq!(form_work(&input, domain), 0);
    assert!(
        StorageObservations::form(&input, domain)
            .unwrap()
            .0
            .is_empty()
    );
}

#[test]
fn symbolic_logical_owner_is_outside_this_prototype_scope() {
    let (_, _, mut input, domain) = written_input();
    input.active_mut().key.address_value = crate::world::AddressInput::unknown_to();
    assert_eq!(form_work(&input, domain), 0);
    assert!(
        StorageObservations::form(&input, domain)
            .unwrap()
            .0
            .is_empty()
    );
}

#[test]
fn repeated_projection_preserves_only_current_snapshot_guarantees() {
    let (_, entry, input, domain) = written_input();
    let summary = form(&input, &entry, domain).unwrap();
    let mut restored = instantiate(&summary, domain).unwrap();
    let owner = restored.active().key.address;
    let key = restored.active().stack[0].clone();
    let old_read = restored.store.read_checked(owner, &key, domain).unwrap();
    assert!(
        restored
            .relations
            .has_direct_numeric_guarantee(old_read.expression().unwrap())
    );
    let again = form(&restored, &entry, domain).unwrap();
    assert_eq!(again.storage_observations, summary.storage_observations);
    // This is a replacement Store, as on a completed child call. The old read's
    // immutable fact must not attach to the new current snapshot.
    let old_root = restored.store.slot_root(owner, false).unwrap();
    let mut replacement = input.store.clone();
    replacement
        .write_checked(owner, &key, &word(9), domain)
        .unwrap();
    restored.store = replacement;
    assert_ne!(old_root, restored.store.slot_root(owner, false).unwrap());
    let new_read = restored.store.read_checked(owner, &key, domain).unwrap();
    assert_eq!(new_read.singleton(), Some(U256::from(9)));
    assert_ne!(old_read.expression(), new_read.expression());
    assert!(
        new_read
            .expression()
            .is_none_or(|expression| !restored.relations.has_direct_numeric_guarantee(expression))
    );
    assert!(
        restored
            .relations
            .has_direct_numeric_guarantee(old_read.expression().unwrap())
    );
    let changed = form(&restored, &entry, domain).unwrap();
    assert_eq!(
        changed.storage_observations.0[0].numeric.singleton(),
        Some(U256::from(9))
    );
}

#[test]
fn independent_key_and_later_maybe_aliasing_write_do_not_reuse_the_old_read() {
    let (_, _, mut input, domain) = written_input();
    let original = input.active().stack[0].clone();
    let other = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    input.active_mut().stack[0] = other.clone();
    assert!(
        StorageObservations::form(&input, domain)
            .unwrap()
            .0
            .is_empty()
    );
    input
        .store
        .write_checked(input.active().key.address, &other, &word(9), domain)
        .unwrap();
    // The first key can alias the later write. It no longer has a direct latest
    // literal proof, and this optional extractor must not walk back to the 7.
    input.active_mut().stack[0] = original;
    assert!(
        StorageObservations::form(&input, domain)
            .unwrap()
            .0
            .is_empty()
    );
}

#[test]
fn low_work_and_cancel_do_not_install_a_partial_summary() {
    let (world, entry, input, domain) = written_input();
    let config = ExecutionConfig::default();
    let fee = form_work(&input, domain);
    println!(
        "STORAGE_OBSERVATION_FORM_FEE work={fee} store_work={}",
        input.store.work_size()
    );
    assert!(fee > 0);
    let mut table = Table::default();
    let mut budget = WorkBudget::new(fee - 1);
    let mut counters = Counters::default();
    let result = Solver {
        table: &mut table,
        world: &world,
        entry: &entry,
        config: &config,
        domain,
        budget: &mut budget,
        counters: &mut counters,
        storage_reads: StorageReadPolicy::Abstract,
    }
    .procedure(input.clone());
    assert_eq!(
        result.unwrap_err().frontier_reason(),
        crate::analysis::FrontierReason::Work
    );
    assert!(table.nodes.is_empty() && table.procedures.is_empty() && table.subscribers.is_empty());
    let control = crate::analysis::control::Control::new(Default::default());
    control.cancellation().cancel();
    control.scope(|| {
        assert!(StorageObservations::form(&input, domain).is_err());
        let mut budget = WorkBudget::new(config.max_work);
        let result = Solver {
            table: &mut table,
            world: &world,
            entry: &entry,
            config: &config,
            domain,
            budget: &mut budget,
            counters: &mut counters,
            storage_reads: StorageReadPolicy::Abstract,
        }
        .procedure(input.clone());
        assert!(result.is_err());
        assert!(
            table.nodes.is_empty() && table.procedures.is_empty() && table.subscribers.is_empty()
        );
    });
}
