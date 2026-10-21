use crate::domain::EvmNumericAlgebra as _;

use super::super::{form, instantiate, join, semantic_eq};
use super::*;
use crate::analysis::MachinePayload;
use crate::analysis::machine::fixtures::{fixture, word};
#[cfg(test)]
use crate::analysis::testing::TestValue;
use crate::domain::ValueAlgebra;
use crate::domain::{NumericValue, ProductAlgebra, symbolic::ExprId};
use alloy_primitives::U256;

#[test]
fn optional_key_evidence_uses_retained_literals_and_nodes_without_discovery() {
    use crate::analysis::state::query::tests::carrier::{Probe, Word};
    use std::rc::Rc;

    for fee in [61, usize::MAX] {
        let probe = Rc::new(Probe::new(fee));
        let numeric = EvmValue::from_numeric(Word::constant(U256::from(7)).watched(&probe));
        let seven = ExprId::constant(U256::from(7));
        let nine = ExprId::constant(U256::from(9));
        assert_eq!(key_work(&numeric), None);
        assert!(key_expression(&numeric).is_none());
        assert!(!same_key(&seven, &numeric));

        // Two independently retained literals suffice even if the carrier's
        // numeric view is Top; retaining this source is also valid evidence.
        let literal = EvmValue::from_numeric(Word::top().watched(&probe))
            .with_expression(ExprId::constant(U256::from(7)));
        assert_eq!(key_work(&literal), Some(1));
        assert_eq!(key_expression(&literal), Some(seven.clone()));
        assert!(same_key(&seven, &literal));
        assert!(!same_key(&nine, &literal));

        let fresh = ExprId::fresh().unwrap();
        let same_node = numeric.clone().with_expression(fresh.clone());
        assert!(same_key(&fresh, &same_node));
        assert!(!same_key(&ExprId::fresh().unwrap(), &same_node));
        assert!(!same_key(&seven, &same_node));
        assert_eq!(probe.counts(), (0, 0, 0));
    }
}

fn written_input() -> (
    crate::analysis::testing::World,
    crate::analysis::testing::Entry,
    MachinePayload,
    crate::domain::ValueAlgebra<ProductAlgebra>,
) {
    let algebra = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = algebra.borrowed();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input = crate::analysis::transfer::initial(&world, &entry, domain).unwrap();
    let key = TestValue::top().with_expression(ExprId::fresh().unwrap());
    input.active_mut().stack = vec![key.clone()];
    let owner = input.active().key.address;
    input
        .store
        .write_checked(owner, &key, &word(7), domain)
        .unwrap();
    (world, entry, input, algebra)
}

#[test]
fn missing_observation_drops_on_join_and_changes_semantics() {
    let (_, entry, input, algebra) = written_input();
let domain = algebra.borrowed();
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
    let (_, entry, input, algebra) = written_input();
let domain = algebra.borrowed();
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
fn equal_alias_observations_install_once_but_keep_every_position_and_numeric_fee() {
    let (_, entry, mut input, algebra) = written_input();
let domain = algebra.borrowed();
    let key = input.active().stack[0].clone();
    input.active_mut().stack = vec![key; MAX_POSITIONS];
    let summary = form(&input, &entry, domain).unwrap();
    let observations = &summary.storage_observations;
    let frame = summary.active();
    assert_eq!(observations.0.len(), MAX_POSITIONS);
    for index in 0..MAX_POSITIONS {
        assert_eq!(
            observations.needs_install(index, &frame.stack_aliases),
            index == 0
        );
        assert_eq!(observations.0[index].stack_position, index);
    }
    let single = EvmStorageObservations(vec![observations.0[0].clone()]);
    let numeric_fee = observations.0[0]
        .numeric
        .work_size()
        .saturating_add(1)
        .saturating_mul(MAX_POSITIONS + 2);
    assert_eq!(
        observations.work_size(&frame.stack, &frame.stack_aliases),
        single.work_size(&frame.stack, &frame.stack_aliases) + (MAX_POSITIONS - 1) * numeric_fee
    );
    let restored = instantiate(&summary, domain).unwrap();
    assert_eq!(restored.relations.len(), 1);
    assert_eq!(
        form(&restored, &entry, domain)
            .unwrap()
            .storage_observations,
        summary.storage_observations
    );
}

#[test]
fn same_alias_with_different_compatible_read_bounds_installs_both() {
    let (_, entry, mut input, algebra) = written_input();
let domain = algebra.borrowed();
    let key = input.active().stack[0].clone();
    input.active_mut().stack.push(key);
    let mut summary = form(&input, &entry, domain).unwrap();
    for (observation, (lower, upper)) in summary
        .storage_observations
        .0
        .iter_mut()
        .zip([(2, 9), (5, 12)])
    {
        observation.numeric =
            NumericValue::unsigned_range(U256::from(lower), U256::from(upper)).unwrap();
        assert!(observation.numeric.contains(U256::from(7)));
    }
    summary.work = summary.measure_work();
    let observations = &summary.storage_observations;
    assert_eq!(observations.0.len(), 2);
    assert!(summary.active().stack_aliases.same_class(0, 1));
    assert!(observations.needs_install(0, &summary.active().stack_aliases));
    assert!(observations.needs_install(1, &summary.active().stack_aliases));
    let restored = instantiate(&summary, domain).unwrap();
    let owner = restored.active().key.address;
    let root = restored.store.slot_root(owner, false).unwrap();
    let key = &restored.active().stack[0];
    for (lower, upper) in [(2, 9), (5, 12)] {
        assert!(restored.relations.constraints().any(|constraint| {
            matches!(constraint, Constraint::Range {
                expression, lower: actual_lower, upper: actual_upper,
            } if *actual_lower == U256::from(lower) && *actual_upper == U256::from(upper)
                && matches_read(expression, &root, key))
        }));
    }
}

#[test]
fn joined_alias_split_and_new_leader_use_the_current_partition() {
    let (_, entry, mut input, algebra) = written_input();
let domain = algebra.borrowed();
    let key = input.active().stack[0].clone();
    input.active_mut().stack = vec![key.clone(); 4];
    let left = form(&input, &entry, domain).unwrap();
    let other = TestValue::top().with_expression(ExprId::fresh().unwrap());
    input.active_mut().stack = vec![key, other.clone(), other.clone(), other];
    let mut right = form(&input, &entry, domain).unwrap();
    // Supply compatible per-position read guarantees independently of aliases.
    right.storage_observations = left.storage_observations.clone();
    right.work = right.measure_work();
    for widen in [false, true] {
        let joined = join(&left, &right, domain, widen).unwrap();
        let observations = &joined.storage_observations;
        let aliases = &joined.active().stack_aliases;
        assert_eq!(observations.0.len(), 4);
        assert!(!aliases.same_class(0, 1));
        assert!(aliases.same_class(1, 3));
        assert_eq!(
            [0, 1, 2, 3].map(|index| observations.needs_install(index, aliases)),
            [true, true, false, false]
        );
        assert!(
            observations.work_size(&joined.active().stack, aliases)
                > left
                    .storage_observations
                    .work_size(&left.active().stack, &left.active().stack_aliases,)
        );
        assert_eq!(instantiate(&joined, domain).unwrap().relations.len(), 2);

        let mut missing = right.clone();
        missing.storage_observations.0.remove(0);
        let dropped = join(&left, &missing, domain, widen).unwrap();
        assert!(!semantic_eq(&joined, &dropped));
        assert_eq!(dropped.storage_observations.0.len(), 3);
        assert_eq!(dropped.storage_observations.0[0].stack_position, 1);
        assert_eq!(instantiate(&dropped, domain).unwrap().relations.len(), 1);
    }
}

#[test]
fn independent_equal_numeric_keys_and_mixed_bound_alias_splits_install_separately() {
    let (_, entry, mut input, algebra) = written_input();
let domain = algebra.borrowed();
    let key = input.active().stack[0].clone();
    input.active_mut().stack.push(key.clone());
    let aliased = form(&input, &entry, domain).unwrap();
    let independent = TestValue::top().with_expression(ExprId::fresh().unwrap());
    let narrow = key
        .clone()
        .with_numeric(NumericValue::unsigned_range(U256::from(2), U256::from(9)).unwrap());
    for other in [independent, narrow] {
        input.active_mut().stack = vec![key.clone(), other];
        let mut summary = form(&input, &entry, domain).unwrap();
        summary.storage_observations = aliased.storage_observations.clone();
        summary.work = summary.measure_work();
        let aliases = &summary.active().stack_aliases;
        assert!(!aliases.same_class(0, 1));
        assert!(summary.storage_observations.needs_install(0, aliases));
        assert!(summary.storage_observations.needs_install(1, aliases));
        assert_eq!(instantiate(&summary, domain).unwrap().relations.len(), 2);
    }
    let mut owners = aliased.storage_observations.clone();
    owners.0[1].owner = crate::analysis::machine::fixtures::address(0x202);
    assert!(owners.needs_install(1, &aliased.active().stack_aliases));
}

#[test]
fn observation_cost_ignores_code_and_caps_optional_root_work() {
    let (_, _, mut input, algebra) = written_input();
let domain = algebra.borrowed();
    let fee = form_work(&input, domain);
    let store_work = input.store.work_size();
    input.store.deploy_code(
        crate::analysis::machine::fixtures::address(0x202),
        crate::bytecode::Program::from_hex(&"00".repeat(24_576)).unwrap(),
    );
    assert!(input.store.work_size() >= store_work + 24_576);
    assert_eq!(form_work(&input, domain), fee);
    let key = input.active().stack[0].clone();
    input.active_mut().stack = vec![TestValue::top(); MAX_POSITIONS + 2];
    input.active_mut().stack[0] = key.clone();
    input.active_mut().stack[MAX_POSITIONS + 1] = key;
    let observations = StorageObservations::form(&input, domain).unwrap();
    assert_eq!(observations.0.len(), 1);
    assert_eq!(observations.0[0].stack_position, MAX_POSITIONS + 1);
    for _ in 0..32 {
        let key = TestValue::top().with_expression(ExprId::fresh().unwrap());
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
fn symbolic_logical_owner_is_outside_current_scope() {
    let (_, _, mut input, algebra) = written_input();
let domain = algebra.borrowed();
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
    let (_, entry, input, algebra) = written_input();
let domain = algebra.borrowed();
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
    let (_, _, mut input, algebra) = written_input();
let domain = algebra.borrowed();
    let original = input.active().stack[0].clone();
    let other = TestValue::top().with_expression(ExprId::fresh().unwrap());
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
