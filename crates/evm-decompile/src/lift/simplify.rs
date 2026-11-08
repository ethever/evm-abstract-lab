//! Local total-word identities and explicit word/predicate conversions.
use super::{EntityId, Kind, Projector};
use crate::{LiftedRelation as Relation, LiftedType, U256, WordExpr, WordRewrite};

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
        args: Vec<EntityId>,
    ) -> Option<EntityId> {
        if !matches!(opcode, 0x10..=0x15) {
            return self.simplify_total_word(block, opcode, args);
        }
        // Predicate-producing opcodes keep their explicit Word256 encoding and
        // conversion path. The shared word algebra never rewrites this boundary.
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
        None
    }

    fn simplify_total_word(
        &mut self,
        block: EntityId,
        opcode: u8,
        args: Vec<EntityId>,
    ) -> Option<EntityId> {
        let expressions = &self.result.program.expressions;
        let indices = &self.indices;
        let rewrite = crate::simplify_word(
            opcode,
            &args,
            |id| {
                let expression = expressions.get(*indices.get(&id)?)?;
                if expression.ty != LiftedType::Word256
                    || expression.kind.ty() != LiftedType::Word256
                {
                    return None;
                }
                Some(match &expression.kind {
                    Kind::Word(value) => WordExpr::Literal(*value),
                    Kind::WordOp {
                        opcode: 0x16,
                        arguments,
                    } if arguments.len() == 2 => WordExpr::And {
                        left: arguments[0],
                        right: arguments[1],
                    },
                    _ => WordExpr::Opaque,
                })
            },
            &mut self.pay,
        )
        .ok()?;
        Some(match rewrite {
            WordRewrite::Unchanged => self.expression(
                block,
                Kind::WordOp {
                    opcode,
                    arguments: args,
                },
                [],
            ),
            WordRewrite::Existing(id) => id,
            WordRewrite::Literal(value) => self.expression(block, Kind::Word(value), []),
            WordRewrite::Masked { value, mask } => {
                let mask = self.expression(block, Kind::Word(mask), []);
                self.expression(
                    block,
                    Kind::WordOp {
                        opcode: 0x16,
                        arguments: vec![value, mask],
                    },
                    [],
                )
            }
        })
    }
}
