//! Dead content history must not multiply retained edge-state memberships.
use super::super::{Flow, Predicate, Range};
use super::*;
use crate::{
    Cancellation, Config, EntityId, Fork, Input, Ir, Phase, U256, decompile_with_progress,
};
use std::cell::Cell;
use std::collections::BTreeMap;

fn source() -> Source<'static, impl FnMut(usize) -> bool> {
    static IR: std::sync::OnceLock<Ir> = std::sync::OnceLock::new();
    Source::new(IR.get_or_init(Ir::default), |_| true).unwrap()
}

#[test]
fn source_predicate_support_and_shifted_anchors_keep_their_live_content() {
    let mut source = source();
    let content = Atom::Content(EntityId(10), U256::from(64));
    let dead = Atom::Content(EntityId(11), U256::from(64));
    let first = Atom::Source(EntityId(20));
    let second = Atom::Source(EntityId(21));
    let live_expr = Expr::atom(content);
    let shifted = live_expr.add(&Expr::constant(U256::from(32)));
    let mut state = State::default();
    state.values.insert(EntityId(30), Expr::atom(second));
    // Neither Boolean key nor the content has to have a numeric atoms entry.
    state.predicates.insert(
        second,
        Predicate {
            opcode: 0x15,
            inputs: vec![Expr::atom(first)],
        },
    );
    state.predicates.insert(
        first,
        Predicate {
            opcode: 0x10,
            inputs: vec![live_expr.clone(), Expr::constant(U256::from(256))],
        },
    );
    state.real.insert(
        shifted.clone(),
        Range {
            low: U256::from(160),
            high: U256::from(224),
        },
    );
    state.atoms.insert(dead, Range::exact(U256::from(128)));
    state
        .bounds
        .insert(Expr::atom(dead), Range::exact(U256::from(128)));
    state
        .real
        .insert(Expr::atom(dead), Range::exact(U256::from(128)));
    state.predicates.insert(
        Atom::Source(EntityId(22)),
        Predicate {
            opcode: 0x15,
            inputs: vec![Expr::atom(dead)],
        },
    );
    assert!(source.retain_contents(&mut state));
    assert!(!state.atoms.contains_key(&dead));
    assert!(!state.bounds.contains_key(&Expr::atom(dead)));
    assert!(!state.real.contains_key(&Expr::atom(dead)));
    assert!(!state.predicates.contains_key(&Atom::Source(EntityId(22))));
    assert!(state.predicates.contains_key(&first) && state.predicates.contains_key(&second));
    assert!(state.real.contains_key(&shifted));
    assert_eq!(
        state.ordinary(&live_expr.add(&Expr::constant(U256::from(36)))),
        Some(Range {
            low: U256::from(164),
            high: U256::from(228)
        })
    );
}

#[test]
fn loop_initial_content_and_iteration_constraints_remain_live() {
    let mut source = source();
    let content = Atom::Content(EntityId(10), U256::from(64));
    let iteration = Atom::Iteration(EntityId(20));
    let value = Expr::atom(content).add(&Expr::atom(iteration).scale(U256::from(32)));
    let mut state = State::default();
    state.values.insert(EntityId(30), value.clone());
    state.atoms.insert(
        content,
        Range {
            low: U256::from(128),
            high: U256::from(160),
        },
    );
    state.atoms.insert(
        iteration,
        Range {
            low: U256::ZERO,
            high: U256::from(3),
        },
    );
    state.real.insert(
        value.clone(),
        Range {
            low: U256::from(128),
            high: U256::from(256),
        },
    );
    assert!(source.retain_contents(&mut state));
    assert!(state.atoms.contains_key(&content));
    assert!(state.atoms.contains_key(&iteration));
    assert!(state.real.contains_key(&value));
}

#[test]
fn cleanup_refusal_precedes_allocation_and_leaves_the_state_unmodified() {
    let ir = Ir::default();
    let active = Cell::new(true);
    let calls = Cell::new(0);
    let mut source = Source::new(&ir, |_| {
        calls.set(calls.get() + 1);
        active.get()
    })
    .unwrap();
    let dead = Atom::Content(EntityId(10), U256::from(64));
    let mut state = State::default();
    state.atoms.insert(dead, Range::exact(U256::from(128)));
    let before = state.atoms.clone();
    let previous = calls.get();
    active.set(false);
    assert!(!source.retain_contents(&mut state));
    assert!(source.refused);
    assert_eq!(calls.get(), previous + 1);
    assert_eq!(state.atoms, before);
}

fn diamonds(count: usize) -> Input {
    let mut bytes = vec![0x60, 128, 0x60, 64, 0x52, 0x60, 0, 0x35];
    for _ in 0..count {
        bytes.extend([0x80, 0x61, 0, 0, 0x57]);
        let left = bytes.len() - 3;
        bytes.extend([0x60, 128, 0x60, 64, 0x52, 0x61, 0, 0, 0x56]);
        let join = bytes.len() - 3;
        let target = u16::try_from(bytes.len()).unwrap().to_be_bytes();
        bytes[left..left + 2].copy_from_slice(&target);
        bytes.extend([0x5b, 0x60, 192, 0x60, 64, 0x52]);
        let target = u16::try_from(bytes.len()).unwrap().to_be_bytes();
        bytes[join..join + 2].copy_from_slice(&target);
        bytes.push(0x5b);
    }
    bytes.extend([0x50, 0]);
    Input::new(bytes, Fork::Osaka)
}

#[test]
fn serial_overwrite_joins_keep_a_bounded_fact_set_in_each_retained_flow() {
    let input = diamonds(64);
    let cancel = Cancellation::new();
    let artifact = decompile_with_progress(&input, &Config::default(), &cancel, |p| {
        if p.phase == Phase::Structure {
            cancel.cancel();
        }
    });
    let ir = &artifact.current().unwrap().ir;
    let mut source = Source::new(ir, |_| true).unwrap();
    let mut flows = BTreeMap::new();
    for component in super::super::super::cycles::components(ir, &mut |_| true).unwrap() {
        assert_eq!(component.len(), 1);
        let block = source.blocks[&component[0]];
        let flow = if Some(block.id) == ir.entry {
            Flow::Known(State {
                memory: Some(block.memory_parameter),
                effect: Some(block.effect_parameter),
                ..State::default()
            })
        } else {
            source.join(block, &flows, None)
        };
        source.process(block, flow, None, false, &mut flows, false);
    }
    assert!(!source.refused);
    let mut facts = 0_usize;
    let mut known = 0_usize;
    for flow in flows.values() {
        if let Flow::Known(state) = flow {
            known += 1;
            let contents = state
                .atoms
                .keys()
                .filter(|atom| matches!(atom, Atom::Content(..)))
                .count();
            assert!(
                contents <= 1,
                "overwritten contents survived an edge: {contents}"
            );
            let size =
                state.atoms.len() + state.bounds.len() + state.real.len() + state.predicates.len();
            assert!(
                size <= 12,
                "serial overwrites retained an increasing history: {size}"
            );
            facts += size;
        }
    }
    assert!(known >= 64);
    assert!(facts <= known * 12);
}
