//! Conditional bounds are applied only on the source edge that establishes them.
//! They are never cached as unconditional facts about the predecessor value.
use super::{Memory, Word};
use crate::{Edge, EdgeKind, Terminator, U256, ValueKind};
impl<P: FnMut(usize) -> bool> Memory<'_, P> {
    pub(super) fn on_edge(&mut self, mut word: Word, edge: &Edge, depth: usize) -> Word {
        let Some(block) = self.blocks.get(&edge.from).copied() else {
            return word;
        };
        let Terminator::Branch {
            condition,
            then_edges,
            else_edge,
            unresolved: false,
            invalid_targets,
            ..
        } = &block.terminator
        else {
            return word;
        };
        if !invalid_targets.is_empty() {
            return word;
        }
        let mut truth = if then_edges.contains(&edge.id) && edge.kind == EdgeKind::True {
            true
        } else if *else_edge == Some(edge.id) && edge.kind == EdgeKind::False {
            false
        } else {
            return word;
        };
        let mut condition = *condition;
        for _ in 0..32 {
            if !self.charge(1) {
                return word;
            }
            let Some(ValueKind::Operation {
                operation,
                index: 0,
            }) = self.values.get(&condition).map(|v| &v.kind)
            else {
                return word;
            };
            let Some(op) = self.operations.get(operation).copied() else {
                return word;
            };
            if op.outputs.as_slice() != [condition] || !op.effects.is_pure() {
                return word;
            }
            if op.opcode == 0x15 && op.inputs.len() == 1 {
                truth = !truth;
                condition = op.inputs[0];
                continue;
            }
            if !matches!(op.opcode, 0x10 | 0x11 | 0x14) || op.inputs.len() != 2 {
                return word;
            }
            let (Some(a), Some(b)) = (
                self.eval(op.inputs[0], depth + 1),
                self.eval(op.inputs[1], depth + 1),
            ) else {
                return word;
            };
            let mut relation = op.opcode;
            let constant = if a.same(word) {
                b.exact()
            } else if b.same(word) {
                relation = match relation {
                    0x10 => 0x11,
                    0x11 => 0x10,
                    other => other,
                };
                a.exact()
            } else {
                return word;
            };
            let Some(bound) = constant else {
                return word;
            };
            match (relation, truth) {
                (0x10, true) if bound > U256::ZERO => {
                    word.high = word.high.min(bound - U256::from(1))
                }
                (0x10, false) => word.low = word.low.max(bound),
                (0x11, true) if bound < U256::MAX => word.low = word.low.max(bound + U256::from(1)),
                (0x11, false) => word.high = word.high.min(bound),
                (0x14, true) => {
                    word.low = word.low.max(bound);
                    word.high = word.high.min(bound);
                }
                (0x14, false) if bound == U256::ZERO => word.low = word.low.max(U256::from(1)),
                _ => {}
            }
            if word.low > word.high {
                return Word {
                    low: U256::ZERO,
                    high: U256::MAX,
                    ..word
                };
            }
            if word.low == word.high {
                return Word::constant(word.low);
            }
            return word;
        }
        word
    }
}
