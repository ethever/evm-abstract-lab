//! Source word identities and unconditional interval equations.
use super::{MAX_DEPTH, Memory, Word};
use crate::{EntityId, FactScope, U256, ValueKind};
impl<P: FnMut(usize) -> bool> Memory<'_, P> {
    pub(super) fn eval(&mut self, id: EntityId, depth: usize) -> Option<Word> {
        if !self.charge(1) || depth > MAX_DEPTH || self.recursion >= MAX_DEPTH {
            return None;
        }
        if let Some(word) = self.certified.get(&id) {
            return Some(*word);
        }
        if let Some(word) = self.words.get(&id) {
            return Some(*word);
        }
        if !self.active_words.insert(id) {
            return None;
        }
        self.recursion += 1;
        let mut result = self.equation(id, depth);
        // An instruction/phi identity in a cycle does not name one value
        // across iterations. Exact constants may cross it; opaque identities
        // require an iteration proof that this bounded analysis does not claim.
        let cyclic = match self.values.get(&id).map(|v| &v.kind) {
            Some(ValueKind::Operation { operation, .. }) => self
                .operation_blocks
                .get(operation)
                .is_some_and(|b| self.cyclic_blocks.contains(b)),
            Some(ValueKind::Parameter { block, .. }) => self.cyclic_blocks.contains(block),
            _ => false,
        };
        if cyclic && result.is_some_and(|word| word.exact().is_none()) {
            result = None;
        }
        self.active_words.remove(&id);
        if let Some(word) = result {
            self.words.insert(id, word);
        }
        self.recursion -= 1;
        result
    }
    fn equation(&mut self, id: EntityId, depth: usize) -> Option<Word> {
        match self.values.get(&id)?.kind.clone() {
            ValueKind::Constant(v) => Some(Word::constant(v)),
            ValueKind::Parameter { block, index } => {
                if self.blocks.get(&block)?.parameters.get(index) != Some(&id) {
                    return None;
                }
                if self.ir.fact_scope != FactScope::CompleteLocalCfg || Some(block) == self.ir.entry
                {
                    return None;
                }
                let edges = self.incoming.get(&block)?.clone();
                if !self.charge(edges.len()) {
                    return None;
                }
                let mut result: Option<Word> = None;
                for edge in edges {
                    let word = self.eval(*edge.arguments.get(index)?, depth + 1)?;
                    let word = self.on_edge(word, edge, depth + 1);
                    result = Some(match result {
                        Some(r) => Word {
                            low: r.low.min(word.low),
                            high: r.high.max(word.high),
                            ..if r.same(word) { r } else { Word::opaque(id) }
                        },
                        None => word,
                    });
                }
                result
            }
            ValueKind::Operation {
                operation,
                index: 0,
            } => {
                let op = *self.operations.get(&operation)?;
                if op.outputs.first() != Some(&id) {
                    return None;
                }
                if op.opcode == 0x51 && op.inputs.len() == 1 {
                    let address = self.eval(op.inputs[0], depth + 1)?.exact()?;
                    // Loads sharing the exact reaching content share identity;
                    // equal numeric ranges never establish that relationship.
                    return self.load(op.memory_input?, address, depth + 1);
                }
                if evm_bytecode::word::arity(op.opcode) != Some(op.inputs.len()) {
                    let mut word = Word::opaque(id);
                    if op.opcode == 0x3d
                        && op.inputs.is_empty()
                        && let Some(high) = self.return_data_size_bound
                    {
                        word.high = high;
                    }
                    return Some(word);
                }
                if !self.charge(evm_bytecode::word::evaluation_work(op.opcode)) {
                    return None;
                }
                let args: Vec<_> = op
                    .inputs
                    .iter()
                    .map(|v| self.eval(*v, depth + 1))
                    .collect::<Option<_>>()?;
                if let Some(values) = args.iter().map(|a| a.exact()).collect::<Option<Vec<_>>>() {
                    return evm_bytecode::word::evaluate(op.opcode, &values)
                        .ok()
                        .map(Word::constant);
                }
                let mut result = Word::opaque(id);
                if matches!(op.opcode, 0x01 | 0x03) && args.len() == 2 {
                    let (a, b) = (args[0], args[1]);
                    if op.opcode == 0x03 && a.base == b.base {
                        return Some(Word::constant(a.offset.wrapping_sub(b.offset)));
                    }
                    let pair = if b.base.is_none() {
                        Some((a, b.offset))
                    } else if op.opcode == 0x01 && a.base.is_none() {
                        Some((b, a.offset))
                    } else {
                        None
                    };
                    if let Some((base, delta)) = pair {
                        result.base = base.base;
                        if op.opcode == 0x01 {
                            result.offset = base.offset.wrapping_add(delta);
                            if let (Some(low), Some(high)) =
                                (base.low.checked_add(delta), base.high.checked_add(delta))
                            {
                                result.low = low;
                                result.high = high;
                            }
                        } else {
                            result.offset = base.offset.wrapping_sub(delta);
                            if let (Some(low), Some(high)) =
                                (base.low.checked_sub(delta), base.high.checked_sub(delta))
                            {
                                result.low = low;
                                result.high = high;
                            }
                        }
                    }
                } else if op.opcode == 0x16 {
                    result.high = args[0].high.min(args[1].high);
                } else if matches!(op.opcode, 0x10..=0x15) {
                    result.high = U256::from(1);
                } else if op.opcode == 0x1c
                    && let Some(shift) = args[0].exact()
                {
                    result.high = if shift >= U256::from(256) {
                        U256::ZERO
                    } else {
                        args[1].high >> usize::try_from(shift).ok()?
                    };
                    if result.high == U256::ZERO {
                        return Some(Word::constant(U256::ZERO));
                    }
                }
                Some(result)
            }
            _ => None,
        }
    }
}
