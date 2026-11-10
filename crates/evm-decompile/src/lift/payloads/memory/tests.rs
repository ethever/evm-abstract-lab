//! Semantic bounds remain explicit and observe the same payer as other words.
use super::*;
use crate::{Block, ExecutionDomain, Fork, Terminator, ValueFact};
use std::cell::Cell;

fn return_size(domain: ExecutionDomain, fork: Option<Fork>) -> Ir {
    Ir {
        execution_domain: domain,
        execution_fork: fork,
        fact_scope: FactScope::CompleteLocalCfg,
        entry: Some(EntityId(1)),
        blocks: vec![Block {
            id: EntityId(1),
            source: None,
            parameters: vec![],
            memory_parameter: EntityId(2),
            effect_parameter: EntityId(3),
            operations: vec![Operation {
                id: EntityId(4),
                source: None,
                opcode: 0x3d,
                name: "RETURNDATASIZE".into(),
                inputs: vec![],
                outputs: vec![EntityId(5)],
                effects: crate::effects::effects(0x3d),
                memory_input: None,
                memory_output: None,
                replacement: None,
                effect_input: Some(EntityId(3)),
                effect_output: Some(EntityId(6)),
            }],
            terminator: Terminator::Exit {
                kind: crate::ExitKind::EndOfCode,
                values: vec![],
            },
        }],
        values: [
            (2, ValueKind::Memory { operation: None }),
            (3, ValueKind::Effect { operation: None }),
            (
                5,
                ValueKind::Operation {
                    operation: EntityId(4),
                    index: 0,
                },
            ),
            (
                6,
                ValueKind::Effect {
                    operation: Some(EntityId(4)),
                },
            ),
        ]
        .into_iter()
        .map(|(id, kind)| Value {
            id: EntityId(id),
            kind,
            fact: ValueFact::Unknown,
        })
        .collect(),
        ..Ir::default()
    }
}

#[test]
fn only_the_complete_declared_transaction_scope_bounds_return_size() {
    let generic = return_size(ExecutionDomain::Generic, None);
    let bounded = return_size(ExecutionDomain::EthereumTransactions, Some(Fork::Osaka));
    for ir in [&generic, &bounded] {
        let mut memory = Memory::new(ir, |_| true).unwrap();
        let result = memory.word(EntityId(5)).unwrap();
        let expected = ir
            .execution_fork
            .and_then(|fork| ir.execution_domain.return_data_size_bound(fork))
            .unwrap_or(U256::MAX);
        assert_eq!(result.high, expected);
        assert!(result.exact().is_none());
    }
    for ir in [
        return_size(ExecutionDomain::Generic, Some(Fork::Osaka)),
        return_size(ExecutionDomain::EthereumTransactions, None),
    ] {
        assert!(Memory::new(&ir, |_| true).is_none());
    }
}

#[test]
fn a_declared_bound_does_not_bypass_payment_or_cancellation() {
    let ir = return_size(ExecutionDomain::EthereumTransactions, Some(Fork::Osaka));
    let attempts = Cell::new(0);
    assert!(
        Memory::new(&ir, |_| {
            attempts.set(attempts.get() + 1);
            false
        })
        .is_none()
    );
    assert_eq!(attempts.get(), 1);
    let active = Cell::new(true);
    let mut memory = Memory::new(&ir, |_| active.get()).unwrap();
    active.set(false);
    assert!(memory.word(EntityId(5)).is_none());
    assert!(memory.refused);
}
