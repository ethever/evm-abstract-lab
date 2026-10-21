//! Content proofs must survive the native read/identity/store boundary.

use super::*;
use crate::{
    Fork,
    analysis::{ExecutionConfig, OutcomeKind, Status, analyze_world},
    domain::provenance::Origin,
    world::{Account, Entry, World, bytes::zero},
};
use alloy_primitives::Address;
use revm_bytecode::opcode;

#[test]
fn literal_loads_survive_runtime_identification_and_storage_joins() {
    for (load, origin) in [
        (opcode::MLOAD, Origin::Memory),
        (opcode::CALLDATALOAD, Origin::Calldata),
    ] {
        // Both CALLVALUE branches read zero at distinct offsets, then store
        // into slot0. A new unbound Fresh per read loses zero after their join.
        let code = format!("34600b575f{load:02x}5f556011565b6020{load:02x}5f555b00");
        let owner = Address::repeat_byte(0x31);
        let mut world = World::new(Fork::Osaka, "word-proof:runtime-store-join");
        world
            .insert(owner, Account::from_hex(&code, Fork::Osaka).unwrap())
            .unwrap();
        let input = Entry::concrete(
            owner,
            Address::repeat_byte(0x32),
            AbstractValue::top(),
            ByteArray::empty(),
        );
        let graph = analyze_world(
            world,
            input,
            ExecutionConfig {
                root_terminal_coverage: false,
                ..ExecutionConfig::default()
            },
        )
        .unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        assert!(graph.states().iter().any(|state| {
            state.active().basic_block_index == 3
                && state.execution_evidence().unwrap().is_current()
        }));
        let returns: Vec<_> = graph
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .collect();
        assert!(!returns.is_empty());
        for outcome in returns {
            let value = outcome
                .store
                .read_checked(owner, &zero(), Domain::default())
                .unwrap();
            assert_eq!(value.numeric(), zero().numeric());
            assert!(
                value
                    .provenance()
                    .origins()
                    .sources()
                    .unwrap()
                    .contains(&origin)
            );
            assert!(!value.symbolic_limit_reached());
        }
    }
}

#[test]
fn prepared_and_complete_reads_keep_the_computed_literal_certificate() {
    let domain = Domain::default();
    let mut branches = [ByteArray::memory(), ByteArray::memory()];
    for (index, memory) in branches.iter_mut().enumerate() {
        memory
            .write_word(
                &AbstractValue::constant(U256::from(32)),
                &AbstractValue::constant(U256::from(index + 1))
                    .with_expression(ExprId::fresh().unwrap()),
                4096,
                domain,
            )
            .unwrap();
    }
    let memory = branches[0]
        .join_guarded(
            &branches[1],
            domain,
            ExprId::fresh().unwrap(),
            limits(ValueAlgebra::new(&domain)),
        )
        .unwrap();
    let frozen = memory.clone();
    for prepared in [true, false] {
        let mut budget = WorkBudget::new(100_000_000);
        let mut plan = memory
            .prepare_word_read(&zero(), domain, &mut budget)
            .unwrap()
            .unwrap();
        assert!(plan.preparation.is_some());
        if !prepared {
            // Force the complete algorithm, paying its full original fee in
            // addition to the already funded optional preparation.
            assert!(budget.charge(plan.fallback_work));
            plan.preparation = None;
        }
        let value = plan.read_word_budgeted(&mut budget).unwrap().unwrap();
        assert_eq!(value.numeric(), zero().numeric());
        assert_eq!(
            value.expression().and_then(ExprId::as_constant),
            Some(U256::ZERO)
        );
        assert!(!budget.exhausted());
        assert_eq!(memory, frozen);
    }
}

#[test]
fn exact_nonliteral_word_sources_remain_independent() {
    let domain = Domain::default();
    let mut expressions = Vec::new();
    for _ in 0..2 {
        let source = ExprId::fresh().unwrap();
        let input = AbstractValue::constant(U256::from(7))
            .with_origin(Origin::Storage)
            .with_expression(source.clone());
        let mut memory = ByteArray::memory();
        memory.write_word(&zero(), &input, 4096, domain).unwrap();
        let mut budget = WorkBudget::new(100_000_000);
        let value = memory
            .prepare_word_read(&zero(), domain, &mut budget)
            .unwrap()
            .unwrap()
            .read_word_budgeted(&mut budget)
            .unwrap()
            .unwrap();
        assert_eq!(value.numeric(), input.numeric());
        assert_eq!(value.expression(), Some(&source));
        assert!(!value.symbolic_limit_reached());
        expressions.push(value.expression().unwrap().clone());
    }
    assert_ne!(expressions[0], expressions[1]);
}
