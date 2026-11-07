//! Bound inline expansion while retaining the original primitive as a named
//! binding. Costs follow the typed DAG once; shared children count at each use.
use super::{EntityId, Kind, Projector};

// Leave space for primitive signatures with up to seven operands under the
// unchanged row ceiling. Wide transfers keep their existing aggregate bound.
const INLINE_BUDGET: usize = crate::MAX_LIFTED_EXPRESSION_TEXT / 16;

impl<P: FnMut(usize) -> bool> Projector<'_, P> {
    pub(super) fn inline_weight(&self, kind: &Kind) -> usize {
        let overhead: usize = match kind {
            Kind::Word(_) => 66, // Full hexadecimal Word256 literal.
            Kind::Predicate(_) => 5,
            Kind::Value(_) => 21, // v followed by any u64 source identity.
            Kind::WordFromPredicate(_) => 6,
            Kind::NonZero(_) => 7,
            Kind::Not(_) => 3,
            Kind::Compare { relation, .. } => match relation {
                crate::LiftedRelation::Slt
                | crate::LiftedRelation::Sle
                | crate::LiftedRelation::Sgt
                | crate::LiftedRelation::Sge => 40,
                _ => 6,
            },
            Kind::WordOp { opcode, arguments } => {
                if arguments.len() == 2 && matches!(opcode, 0x01..=0x04 | 0x06 | 0x16..=0x18) {
                    5
                } else {
                    64
                }
            }
        };
        kind.operands()
            .into_iter()
            .fold(overhead, |weight, child| {
                weight.saturating_add(self.inline_weights[self.indices[&child]])
            })
            .min(INLINE_BUDGET + 1)
    }
    pub(super) fn needs_named_binding(&self, expression: EntityId) -> bool {
        self.inline_weights[self.indices[&expression]] > INLINE_BUDGET
    }
}
