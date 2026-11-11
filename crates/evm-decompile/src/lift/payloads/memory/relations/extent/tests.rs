use super::*;
use crate::{Block, FactScope, Ir, Terminator, Value, ValueFact};

fn constant(value: u64) -> Expr {
    Expr::constant(U256::from(value))
}

#[test]
fn unknown_writes_clear_the_bound_but_zero_width_never_touches_memory() {
    let mut state = State {
        extent: Some(Box::new(constant(0))),
        ..State::default()
    };
    state.written(Some(&(constant(96), constant(32))));
    assert_eq!(state.extent, Some(Box::new(constant(128))));
    let unknown = Expr::atom(super::super::Atom::Source(EntityId(1)));
    state.written(Some(&(unknown.clone(), constant(0))));
    assert_eq!(state.extent, Some(Box::new(constant(128))));
    state.written(Some(&(unknown, constant(1))));
    assert!(state.extent.is_none());
    state.written(Some(&(constant(512), constant(32))));
    assert!(
        state.extent.is_none(),
        "a later write does not restore an erased history"
    );
}

#[test]
fn an_unknown_copy_width_and_a_wrapping_destination_have_no_extent() {
    for range in [
        (
            constant(128),
            Expr::atom(super::super::Atom::Source(EntityId(2))),
        ),
        (Expr::constant(U256::MAX - U256::from(15)), constant(32)),
    ] {
        let mut state = State {
            extent: Some(Box::new(constant(96))),
            ..State::default()
        };
        state.written(Some(&range));
        assert!(state.extent.is_none());
    }
}

#[test]
fn a_join_anchor_requires_the_bound_on_each_complete_input() {
    let address = constant(64);
    let mut a = State {
        extent: Some(Box::new(constant(160))),
        cells: BTreeMap::from([(address.clone(), constant(192))]),
        ..State::default()
    };
    let mut b = State {
        extent: Some(Box::new(constant(192))),
        cells: BTreeMap::from([(address.clone(), constant(256))]),
        ..State::default()
    };
    assert_eq!(join_anchors(&a, &b), vec![address]);
    b.extent = None;
    assert!(join_anchors(&a, &b).is_empty());
    b.extent = Some(Box::new(constant(257)));
    assert!(join_anchors(&a, &b).is_empty());
    a.extent = None;
    assert!(join_anchors(&a, &b).is_empty());
}

#[test]
fn only_a_true_empty_entry_memory_seeds_zero_bytes() {
    let mut ir = Ir {
        bytecode_len: 1,
        fact_scope: FactScope::CompleteLocalCfg,
        entry: Some(EntityId(1)),
        blocks: vec![Block {
            id: EntityId(1),
            source: Some(crate::SourceSpan { pc: 0, end_pc: 1 }),
            parameters: vec![],
            memory_parameter: EntityId(2),
            effect_parameter: EntityId(3),
            operations: vec![],
            terminator: Terminator::Exit {
                kind: crate::ExitKind::Stop,
                values: vec![],
            },
        }],
        values: vec![Value {
            id: EntityId(2),
            kind: ValueKind::Memory { operation: None },
            fact: ValueFact::Unknown,
        }],
        ..Ir::default()
    };
    let facts = super::super::analyze(&ir, &mut |_| true).unwrap();
    assert_eq!(facts.extents[&EntityId(2)].exact(), Some(U256::ZERO));
    ir.values[0].kind = ValueKind::MemoryParameter { block: EntityId(1) };
    let facts = super::super::analyze(&ir, &mut |_| true).unwrap();
    assert_eq!(facts.extents[&EntityId(2)].exact(), Some(U256::ZERO));
    ir.values[0].kind = ValueKind::MemoryParameter { block: EntityId(7) };
    let facts = super::super::analyze(&ir, &mut |_| true).unwrap();
    assert!(facts.extents.is_empty());
    ir.values[0].kind = ValueKind::Memory { operation: None };
    ir.blocks[0].source.as_mut().unwrap().pc = 7;
    let facts = super::super::analyze(&ir, &mut |_| true).unwrap();
    assert!(facts.extents.is_empty());
    ir.blocks[0].source.as_mut().unwrap().pc = 0;
    ir.blocks[0].terminator = crate::Terminator::Fallthrough { edge: EntityId(4) };
    ir.edges.push(crate::Edge {
        id: EntityId(4),
        from: EntityId(1),
        to: EntityId(1),
        kind: crate::EdgeKind::Fallthrough,
        arguments: vec![],
        memory: EntityId(2),
        effect: EntityId(3),
    });
    let source = Source::new(&ir, |_| true).unwrap();
    assert!(source.empty_extent(&ir.blocks[0]).is_none());
}
