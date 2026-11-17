use super::super::{Predicate, Range};
use super::*;
use crate::{EntityId, Ir};
use std::cell::Cell;

fn operation(opcode: u8) -> Operation {
    Operation {
        id: EntityId(1),
        source: None,
        opcode,
        name: String::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        effects: crate::effects::effects(opcode),
        memory_input: None,
        memory_output: None,
        replacement: None,
        effect_input: None,
        effect_output: None,
    }
}

#[test]
fn source_exact_offsets_and_timestamp_have_their_own_fixed_frame_identity() {
    let load = operation(0x35);
    assert_eq!(
        input(&load, &[Some(Expr::constant(U256::from(164)))]),
        Some(Expr::atom(Atom::Immutable(Symbol::CalldataWord(
            U256::from(164)
        ))))
    );
    assert_ne!(
        input(&load, &[Some(Expr::constant(U256::ZERO))]),
        input(&load, &[Some(Expr::constant(U256::from(32)))])
    );
    assert_eq!(
        input(&load, &[Some(Expr::atom(Atom::Source(EntityId(8))))]),
        None
    );
    assert_eq!(
        input(&operation(0x42), &[]),
        Some(Expr::atom(Atom::Immutable(Symbol::Timestamp)))
    );
    for opcode in [0x3d, 0x51, 0x54, 0x59, 0x5a, 0x5c, 0xf1, 0xfa] {
        assert_eq!(input(&operation(opcode), &[]), None);
    }
}

#[test]
fn predicate_truth_requires_an_actual_common_guard_not_two_full_ranges() {
    let ir = Ir::default();
    let mut source = Source::new(&ir, |_| true).unwrap();
    let args = [
        Expr::atom(Atom::Immutable(Symbol::CalldataWord(U256::from(164)))),
        Expr::atom(Atom::Immutable(Symbol::Timestamp)),
    ];
    let atom = Atom::Source(EntityId(10));
    let mut state = State::default();
    state.predicates.insert(
        atom,
        Predicate {
            opcode: 0x10,
            inputs: args.to_vec(),
        },
    );
    assert_eq!(source.immutable_predicate(0x10, &args, &state), None);
    state.atoms.insert(atom, Range::exact(U256::ZERO));
    assert_eq!(
        source.immutable_predicate(0x10, &args, &state),
        Some(Expr::constant(U256::ZERO))
    );
    let mut different = args.clone();
    different[0] = Expr::atom(Atom::Immutable(Symbol::CalldataWord(U256::from(196))));
    assert_eq!(source.immutable_predicate(0x10, &different, &state), None);
    let joined = source.merge(state, &State::default(), None);
    assert_eq!(source.immutable_predicate(0x10, &args, &joined), None);
}

#[test]
fn runtime_operands_do_not_reuse_prior_iteration_predicates() {
    let ir = Ir::default();
    let mut source = Source::new(&ir, |_| true).unwrap();
    for value in [
        Atom::Source(EntityId(3)),
        Atom::Local(EntityId(4), EntityId(5)),
        Atom::Content(EntityId(6), U256::from(64)),
    ] {
        let args = [Expr::atom(value), Expr::constant(U256::from(32))];
        let atom = Atom::Source(EntityId(10));
        let mut state = State::default();
        state.predicates.insert(
            atom,
            Predicate {
                opcode: 0x10,
                inputs: args.to_vec(),
            },
        );
        state.atoms.insert(atom, Range::exact(U256::ZERO));
        assert_eq!(source.immutable_predicate(0x10, &args, &state), None);
    }
}

#[test]
fn predicate_scan_refusal_precedes_lookup_and_no_nonboolean_word_becomes_one() {
    let ir = Ir::default();
    let allow = Cell::new(true);
    let mut source = Source::new(&ir, |_| allow.get()).unwrap();
    let args = [
        Expr::atom(Atom::Immutable(Symbol::Timestamp)),
        Expr::constant(U256::from(255)),
    ];
    let atom = Atom::Source(EntityId(10));
    let mut state = State::default();
    state.predicates.insert(
        atom,
        Predicate {
            opcode: 0x16,
            inputs: args.to_vec(),
        },
    );
    state.atoms.insert(
        atom,
        Range {
            low: U256::from(2),
            high: U256::from(7),
        },
    );
    assert_eq!(
        source.immutable_predicate(0x16, &args, &state),
        Some(Expr::atom(atom))
    );
    allow.set(false);
    assert_eq!(source.immutable_predicate(0x16, &args, &state), None);
    assert!(source.refused);
}
