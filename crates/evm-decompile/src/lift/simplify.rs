//! Local total-word identities and explicit word/predicate conversions.
use super::{EntityId, Kind, Projector};
use crate::{LiftedRelation as Relation, U256};

impl<P: FnMut(usize) -> bool> Projector<'_, P> {
    fn literal(&self, id: EntityId) -> Option<U256> {
        if let Kind::Word(v) = self.kind(id) {
            Some(*v)
        } else {
            None
        }
    }
    pub(super) fn nonzero(&mut self, block: EntityId, word: EntityId) -> EntityId {
        // One witness anchor is enough for this conversion; the consuming raw
        // instruction is attached separately. Copying the accumulated ancestry
        // here would make long alternating ISZERO chains quadratic.
        let parents = self
            .result
            .parents
            .get(&word)
            .and_then(|p| p.first().copied());
        match self.kind(word).clone() {
            Kind::Word(v) => self.expression(block, Kind::Predicate(v != U256::ZERO), parents),
            Kind::WordFromPredicate(predicate) => predicate,
            _ => self.expression(block, Kind::NonZero(word), parents),
        }
    }
    fn negate(&mut self, block: EntityId, predicate: EntityId) -> EntityId {
        let kind = match self.kind(predicate).clone() {
            Kind::Predicate(v) => Kind::Predicate(!v),
            Kind::Not(v) => return v,
            Kind::Compare {
                relation,
                left,
                right,
            } => Kind::Compare {
                relation: relation.complement(),
                left,
                right,
            },
            _ => Kind::Not(predicate),
        };
        let parents = self
            .result
            .parents
            .get(&predicate)
            .and_then(|p| p.first().copied());
        self.expression(block, kind, parents)
    }
    pub(super) fn simplify(
        &mut self,
        block: EntityId,
        opcode: u8,
        mut args: Vec<EntityId>,
    ) -> Option<EntityId> {
        if let Some(words) = args
            .iter()
            .map(|id| self.literal(*id))
            .collect::<Option<Vec<_>>>()
        {
            if !(self.pay)(evm_bytecode::word::evaluation_work(opcode)) {
                return None;
            }
            let result = evm_bytecode::word::evaluate(opcode, &words).ok()?;
            return Some(self.expression(block, Kind::Word(result), []));
        }
        if opcode == 0x15 {
            let truth = self.nonzero(block, args[0]);
            let predicate = self.negate(block, truth);
            return Some(self.expression(block, Kind::WordFromPredicate(predicate), []));
        }
        let relation = match opcode {
            0x10 => Some(Relation::Ult),
            0x11 => Some(Relation::Ugt),
            0x12 => Some(Relation::Slt),
            0x13 => Some(Relation::Sgt),
            0x14 => Some(Relation::Eq),
            _ => None,
        };
        if let Some(relation) = relation {
            let predicate = if args[0] == args[1] {
                self.expression(
                    block,
                    Kind::Predicate(relation.evaluate(U256::ZERO, U256::ZERO)),
                    [],
                )
            } else {
                self.expression(
                    block,
                    Kind::Compare {
                        relation,
                        left: args[0],
                        right: args[1],
                    },
                    [],
                )
            };
            return Some(self.expression(block, Kind::WordFromPredicate(predicate), []));
        }
        let zero = |p: &Self, id| p.literal(id) == Some(U256::ZERO);
        let one = |p: &Self, id| p.literal(id) == Some(U256::from(1));
        let all = |p: &Self, id| p.literal(id) == Some(U256::MAX);
        if args.len() == 2 {
            let (a, b) = (args[0], args[1]);
            let identity = match opcode {
                0x01 | 0x17 | 0x18 if zero(self, a) => Some(b),
                0x01 | 0x03 | 0x17 | 0x18 if zero(self, b) => Some(a),
                0x02 if one(self, a) => Some(b),
                0x02 | 0x04 if one(self, b) => Some(a),
                0x16 if all(self, a) => Some(b),
                0x16 if all(self, b) => Some(a),
                0x16 | 0x17 if a == b => Some(a),
                0x1b..=0x1d if zero(self, a) => Some(b),
                _ => None,
            };
            if identity.is_some() {
                return identity;
            }
            let is_zero = match opcode {
                0x02 | 0x16 => zero(self, a) || zero(self, b),
                0x03 | 0x18 => a == b,
                // EVM division and remainder by zero are defined as zero.
                0x04..=0x07 => zero(self, a) || zero(self, b),
                _ => false,
            };
            if is_zero {
                return Some(self.expression(block, Kind::Word(U256::ZERO), []));
            }
            if opcode == 0x16 {
                // (x & mask) & mask and nested literal masks, in either order.
                for (nested, other) in [(a, b), (b, a)] {
                    if let Kind::WordOp {
                        opcode: 0x16,
                        arguments,
                    } = self.kind(nested).clone()
                    {
                        if arguments.contains(&other) {
                            return Some(nested);
                        }
                        if let Some(mask) = self.literal(other) {
                            for (i, inner) in arguments.iter().enumerate() {
                                if let Some(inner_mask) = self.literal(*inner) {
                                    let combined =
                                        self.expression(block, Kind::Word(mask & inner_mask), []);
                                    args = vec![arguments[1 - i], combined];
                                    if mask & inner_mask == U256::ZERO {
                                        return Some(combined);
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        Some(self.expression(
            block,
            Kind::WordOp {
                opcode,
                arguments: args,
            },
            [],
        ))
    }
}
