//! Regional admission preserves control facts while its coordinate support shrinks.

use super::*;
use crate::{
    Fork,
    analysis::{
        engine::policy::{
            BoundedJoin, FeedbackAcceleration, PolicySite, ProcedureSummaryPolicy,
            RegionalSummaries, SemanticPolicies, StructuralPartition,
        },
        machine::fixtures,
        testing::{ByteArray, TestValue},
        transfer,
    },
    domain::{EvmValue, FiniteSetAlgebra, ProductAlgebra, ValueAlgebra, symbolic::ExprId},
    resource::WorkBudget,
    world::{EvmAccount, EvmEntry, EvmWorld},
};
use alloy_primitives::U256;
use std::collections::BTreeSet;

fn early_known<D: EvmNumericAlgebra>(policy: &impl ProcedureSummaryPolicy<D>) -> bool {
    policy.supports_early_known_admission()
}

fn retained_control_values<D: EvmNumericAlgebra>(numeric: D) {
    let algebra = ValueAlgebra::new(numeric);
    let domain = algebra.borrowed();
    let owner = fixtures::address(0x101);
    let mut world = EvmWorld::<D::Element>::new(Fork::Osaka, "regional control facts");
    world
        .insert(owner, EvmAccount::from_hex("00", world.fork()).unwrap())
        .unwrap();
    let entry = EvmEntry::new(owner);
    let mut left = transfer::initial(&world, &entry, domain).unwrap();
    left.active_mut().stack = vec![
        EvmValue::constant(U256::from(26)),
        EvmValue::constant(U256::ZERO),
    ];
    left.active_mut().call_value = EvmValue::constant(U256::ONE);
    let mut right = left.clone();
    right.active_mut().stack = vec![
        EvmValue::constant(U256::from(49)),
        EvmValue::constant(U256::ONE),
    ];
    right.active_mut().call_value = EvmValue::constant(U256::from(2));
    let mut budget = WorkBudget::new(1_000_000_000);
    let mut cx = PolicyContext::new(domain, &entry, &mut budget);
    let left = EvmPayloadSummary::project(&left, &mut cx).unwrap();
    let right = EvmPayloadSummary::project(&right, &mut cx).unwrap();
    let late = RegionalSummaries::new();
    let early = RegionalSummaries::with_early_known_admission();
    assert!(!early_known::<D>(&late));
    assert!(early_known::<D>(&early));
    for acceleration in [Acceleration::Join, Acceleration::Widen] {
        let normalized = late
            .admit_relative(&left, &left, PolicySite::Procedure, acceleration, &mut cx)
            .unwrap();
        assert_eq!(
            normalized.active().stack[0].singleton(),
            Some(U256::from(26))
        );
        let joined = late
            .admit_relative(&normalized, &right, PolicySite::Body, acceleration, &mut cx)
            .unwrap();
        let early_joined = early
            .admit_relative(&normalized, &right, PolicySite::Body, acceleration, &mut cx)
            .unwrap();
        assert!(joined.semantic_eq(&early_joined, &mut cx).unwrap());
        let live = joined.instantiate(&mut cx).unwrap();
        assert_eq!(
            live.active().stack[0].constants(),
            Some(&BTreeSet::from([U256::from(26), U256::from(49)]))
        );
        assert!(!live.active().stack[0].contains(U256::from(41)));
        assert_eq!(
            live.active().stack[1].constants(),
            Some(&BTreeSet::from([U256::ZERO, U256::ONE]))
        );
        assert_eq!(
            live.active().call_value.constants(),
            Some(&BTreeSet::from([U256::ONE, U256::from(2)]))
        );
    }
}

#[test]
fn finite_jump_and_control_values_survive_native_carriers_and_both_admission_modes() {
    retained_control_values(ProductAlgebra::default());
    retained_control_values(FiniteSetAlgebra::default());
    let assembly = SemanticPolicies::new(
        StructuralPartition,
        BoundedJoin,
        FeedbackAcceleration,
        RegionalSummaries::with_early_known_admission(),
    );
    assert!(early_known::<ProductAlgebra>(&assembly));
    assert!(!early_known::<ProductAlgebra>(&SemanticPolicies::default()));
}

#[test]
fn common_aliases_survive_and_lost_aliases_cannot_return_from_equal_numeric_bounds() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let fresh = || TestValue::top().with_expression(ExprId::fresh().unwrap());
    let a = fresh();
    let b = fresh();
    input.active_mut().stack = vec![a.clone(), a, b.clone(), b];
    let mut budget = WorkBudget::new(1_000_000_000);
    let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
    let left = EvmPayloadSummary::project(&input, &mut cx).unwrap();
    let normalized = left
        .regional_join(&left, Acceleration::Join, &mut cx)
        .unwrap();
    assert!(normalized.active().stack_aliases.same_class(0, 1));
    let c = fresh();
    let d = fresh();
    input.active_mut().stack = vec![c, d.clone(), d.clone(), d];
    let right = EvmPayloadSummary::project(&input, &mut cx).unwrap();
    for acceleration in [Acceleration::Join, Acceleration::Widen] {
        let joined = normalized
            .regional_join(&right, acceleration, &mut cx)
            .unwrap();
        assert!(!joined.active().stack_aliases.same_class(0, 1));
        assert!(!joined.active().stack_aliases.same_class(1, 2));
        assert!(joined.active().stack_aliases.same_class(2, 3));
        assert_eq!(joined.work_size(), joined.measure_work());
        assert_eq!(joined.storage_observations, Default::default());
        let first = joined.instantiate(&mut cx).unwrap();
        let second = joined.instantiate(&mut cx).unwrap();
        assert_eq!(
            first.active().stack[2].expression(),
            first.active().stack[3].expression()
        );
        assert_ne!(
            first.active().stack[0].expression(),
            first.active().stack[1].expression()
        );
        assert_ne!(
            first.active().stack[2].expression(),
            second.active().stack[2].expression()
        );
        let all = fresh();
        input.active_mut().stack = vec![all.clone(), all.clone(), all.clone(), all];
        let all = EvmPayloadSummary::project(&input, &mut cx).unwrap();
        let again = joined.regional_join(&all, acceleration, &mut cx).unwrap();
        assert!(!again.active().stack_aliases.same_class(0, 1));
        assert!(again.active().stack_aliases.same_class(2, 3));
    }
}

#[test]
fn relative_growth_covers_inputs_without_expanding_byte_store_or_rollback_support() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let domain = algebra.borrowed();
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut input = transfer::initial(&world, &entry, domain).unwrap();
    let owner = input.active().key.address;
    input.active_mut().stack = vec![fixtures::word(26)];
    input
        .active_mut()
        .memory
        .write_byte(&fixtures::word(0), &fixtures::word(26), domain)
        .unwrap();
    input
        .store
        .write_checked(owner, &fixtures::word(1), &fixtures::word(26), domain)
        .unwrap();
    input.active_mut().saved_store = input.store.snapshot();
    let mut budget = WorkBudget::new(1_000_000_000);
    let mut cx = PolicyContext::new(domain, &entry, &mut budget);
    let first = EvmPayloadSummary::project(&input, &mut cx).unwrap();
    let mut current = first
        .regional_join(&first, Acceleration::Join, &mut cx)
        .unwrap();
    let mut seen = vec![26u8];
    for (index, target) in [49u8, 73, 97].into_iter().enumerate() {
        seen.push(target);
        let value = fixtures::word(u64::from(target));
        input.active_mut().stack[0] = value.clone();
        input
            .active_mut()
            .memory
            .write_byte(&fixtures::word(0), &value, domain)
            .unwrap();
        input
            .active_mut()
            .memory
            .write_byte(
                &TestValue::constant(U256::from(32 * (index + 1))),
                &value,
                domain,
            )
            .unwrap();
        input
            .store
            .write_checked(owner, &fixtures::word(1), &value, domain)
            .unwrap();
        input
            .store
            .write_checked(
                owner,
                &TestValue::constant(U256::from(10 + index)),
                &value,
                domain,
            )
            .unwrap();
        input.active_mut().saved_store = input.store.snapshot();
        let incoming = EvmPayloadSummary::project(&input, &mut cx).unwrap();
        let joined = current
            .regional_join(&incoming, Acceleration::Join, &mut cx)
            .unwrap();
        assert!(joined.store.covers(&current.store, domain));
        assert!(joined.store.covers(&incoming.store, domain));
        assert_eq!(
            joined.store,
            joined.store.project_to_layout(&current.store, domain)
        );
        for ((frame, old), next) in joined
            .frames
            .iter()
            .zip(&current.frames)
            .zip(&incoming.frames)
        {
            for ((bytes, layout), other) in frame
                .byte_arrays()
                .into_iter()
                .zip(old.byte_arrays())
                .zip(next.byte_arrays())
            {
                assert!(bytes.covers(layout, domain));
                assert!(bytes.covers(other, domain));
                assert_eq!(*bytes, bytes.project_to_layout(layout, domain));
            }
            assert!(frame.saved_store.covers(&old.saved_store, domain));
            assert!(frame.saved_store.covers(&next.saved_store, domain));
            assert_eq!(
                frame.saved_store,
                frame
                    .saved_store
                    .project_to_layout(&old.saved_store, domain)
            );
        }
        for target in &seen {
            assert!(joined.active().stack[0].contains(U256::from(*target)));
        }
        assert_eq!(joined.work_size(), joined.measure_work());
        current = joined;
    }
}

#[test]
fn output_growth_keeps_finite_cells_and_covers_omitted_new_coordinates() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let domain = algebra.borrowed();
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut input = transfer::initial(&world, &entry, domain).unwrap();
    let owner = input.active().key.address;
    input
        .store
        .write_checked(owner, &fixtures::word(1), &fixtures::word(26), domain)
        .unwrap();
    let mut budget = WorkBudget::new(1_000_000_000);
    let mut cx = PolicyContext::new(domain, &entry, &mut budget);
    let first = ProcedureOutput::project(&ByteArray::exact(&[26]), &input.store, &mut cx).unwrap();
    let policy = RegionalSummaries::new();
    let mut current = policy
        .admit_output(None, &first, Acceleration::Join, &mut cx)
        .unwrap();
    let mut seen = vec![26u8];
    for (index, target) in [49u8, 73, 97].into_iter().enumerate() {
        seen.push(target);
        let value = fixtures::word(u64::from(target));
        input
            .store
            .write_checked(owner, &fixtures::word(1), &value, domain)
            .unwrap();
        input
            .store
            .write_checked(
                owner,
                &TestValue::constant(U256::from(10 + index)),
                &value,
                domain,
            )
            .unwrap();
        let data = ByteArray::exact(&vec![target; 32 * (index + 1)]);
        let incoming = ProcedureOutput::project(&data, &input.store, &mut cx).unwrap();
        let joined = policy
            .admit_output(Some(&current), &incoming, Acceleration::Join, &mut cx)
            .unwrap();
        let early = RegionalSummaries::with_early_known_admission()
            .admit_output(Some(&current), &incoming, Acceleration::Join, &mut cx)
            .unwrap();
        assert_eq!(joined, early);
        assert!(joined.data.covers(&current.data, domain));
        assert!(joined.data.covers(&incoming.data, domain));
        assert!(joined.store.covers(&current.store, domain));
        assert!(joined.store.covers(&incoming.store, domain));
        assert_eq!(
            joined.data,
            joined.data.project_to_layout(&current.data, domain)
        );
        assert_eq!(
            joined.store,
            joined.store.project_to_layout(&current.store, domain)
        );
        let live = joined.data.instantiate(domain).unwrap();
        let store = joined.store.instantiate(domain).unwrap();
        for target in &seen {
            assert!(live.byte_at(0, domain).contains(U256::from(*target)));
            assert!(
                store
                    .read_checked(owner, &fixtures::word(1), domain)
                    .unwrap()
                    .contains(U256::from(*target))
            );
        }
        assert!(!live.byte_at(0, domain).contains(U256::from(41)));
        // Unretained positions cover the new content and arbitrary alternatives.
        assert!(live.byte_at(1, domain).contains(U256::from(target)));
        assert!(live.byte_at(1, domain).contains(U256::from(255)));
        assert_eq!(
            joined.work_size(),
            joined.data.work_size() + joined.store.work_size()
        );
        current = joined;
    }
}

#[test]
fn initial_output_bounds_an_exact_buffer_before_its_first_publication() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let domain = algebra.borrowed();
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let input = transfer::initial(&world, &entry, domain).unwrap();
    let mut budget = WorkBudget::new(1_000_000_000);
    let mut cx = PolicyContext::new(domain, &entry, &mut budget);
    let data = ByteArray::exact(&[7; 4096]);
    let raw = ProcedureOutput::project(&data, &input.store, &mut cx).unwrap();
    let admitted = RegionalSummaries::new()
        .admit_output(None, &raw, Acceleration::Join, &mut cx)
        .unwrap();
    assert!(admitted.data.work_size() < raw.data.work_size());
    assert!(admitted.data.covers(&raw.data, domain));
    let live = admitted.data.instantiate(domain).unwrap();
    assert_eq!(live.len().singleton(), Some(U256::from(4096)));
    assert_eq!(live.byte_at(0, domain).singleton(), Some(U256::from(7)));
    assert!(live.byte_at(4095, domain).contains(U256::from(7)));
    assert!(live.byte_at(4095, domain).contains(U256::from(255)));
}
