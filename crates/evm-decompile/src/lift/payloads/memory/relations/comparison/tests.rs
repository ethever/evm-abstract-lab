use super::*;
use crate::EntityId;
use crate::lift::payloads::memory::relations::{Atom, Range};

fn state(a: (u64, u64), b: (u64, u64)) -> (State, [Expr; 2]) {
    let atoms = [Atom::Source(EntityId(1)), Atom::Source(EntityId(2))];
    let mut state = State::default();
    for (atom, (low, high)) in atoms.into_iter().zip([a, b]) {
        state.atoms.insert(
            atom,
            Range {
                low: U256::from(low),
                high: U256::from(high),
            },
        );
    }
    (state, atoms.map(Expr::atom))
}

#[test]
fn every_small_interval_decision_agrees_with_all_concrete_operands() {
    for low_a in 0..8 {
        for high_a in low_a..8 {
            for low_b in 0..8 {
                for high_b in low_b..8 {
                    let (state, args) = state((low_a, high_a), (low_b, high_b));
                    for opcode in [0x10, 0x11, 0x14] {
                        if let Some(truth) = constant(&state, opcode, &args) {
                            for a in low_a..=high_a {
                                for b in low_b..=high_b {
                                    assert_eq!(
                                        truth,
                                        match opcode {
                                            0x10 => a < b,
                                            0x11 => a > b,
                                            _ => a == b,
                                        }
                                    );
                                }
                            }
                        }
                    }
                    if let Some(truth) = constant(&state, 0x15, &args[..1]) {
                        for a in low_a..=high_a {
                            assert_eq!(truth, a == 0);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn positive_zero_and_signed_ambiguity_remain_distinct() {
    let (mut state, args) = state((1, 7), (0, 0));
    assert_eq!(constant(&state, 0x15, &args[..1]), Some(false));
    assert_eq!(constant(&state, 0x15, &args[1..]), Some(true));
    assert_eq!(
        constant(&state, 0x14, &[args[0].clone(), args[0].clone()]),
        Some(true)
    );
    state.atoms.insert(
        args[0].single().unwrap(),
        Range {
            low: U256::ZERO,
            high: U256::MAX,
        },
    );
    assert_eq!(constant(&state, 0x15, &args[..1]), None);
    for opcode in [0x12, 0x13] {
        assert_eq!(constant(&state, opcode, &args), None);
    }
}

#[test]
fn one_guarded_incoming_state_cannot_constrain_an_unrestricted_alternative() {
    let (guarded, args) = state((1, 7), (0, 0));
    let mut unrestricted = State::default();
    unrestricted.values.insert(EntityId(1), args[0].clone());
    let ir = crate::Ir::default();
    let source = super::super::Source::new(&ir, |_| true).unwrap();
    let joined = source.merge(guarded, &unrestricted, None);
    assert_eq!(constant(&joined, 0x15, &args[..1]), None);
    assert_eq!(constant(&joined, 0x10, &args), None);
}
