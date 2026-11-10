//! Source replay and edge-local facts shared by acyclic propagation and induction.
use super::{
    Atom, Block, EntityId, Expr, MAX_CELLS, MAX_TERMS, Operation, Predicate, Range, Source, State,
    U256, ValueKind,
};
impl<P: FnMut(usize) -> bool> Source<'_, P> {
    pub(super) fn execute(
        &mut self,
        block: &Block,
        state: &mut State,
        scope: Option<EntityId>,
        optimistic: bool,
    ) -> bool {
        if !self.charge(block.parameters.len().saturating_add(1)) {
            return false;
        }
        for (index, id) in block.parameters.iter().enumerate() {
            if !matches!(self.values.get(id).map(|v| &v.kind), Some(ValueKind::Parameter { block: b, index: i }) if *b == block.id && *i == index)
            {
                return false;
            }
        }
        if state.memory != Some(block.memory_parameter)
            || state.effect != Some(block.effect_parameter)
        {
            return false;
        }
        if Some(block.id) != self.ir.entry
            && (!matches!(self.values.get(&block.memory_parameter).map(|v| &v.kind), Some(ValueKind::MemoryParameter { block: id }) if *id == block.id)
                || !matches!(self.values.get(&block.effect_parameter).map(|v| &v.kind), Some(ValueKind::EffectParameter { block: id }) if *id == block.id))
        {
            return false;
        }
        match &block.terminator {
            crate::Terminator::Branch {
                condition, target, ..
            } if !block.operations.last().is_some_and(|op| {
                op.opcode == 0x57 && op.inputs.as_slice() == [*target, *condition]
            }) =>
            {
                return false;
            }
            crate::Terminator::Jump { target, .. }
                if !block.operations.last().is_some_and(|op| {
                    (op.opcode == 0x56 && op.inputs.as_slice() == [*target])
                        || (op.opcode == 0x57
                            && op.inputs.first() == Some(target)
                            && op.inputs.len() == 2)
                }) =>
            {
                return false;
            }
            _ => {}
        }
        for op in &block.operations {
            if !self.charge(
                op.inputs
                    .len()
                    .saturating_mul(MAX_TERMS)
                    .saturating_add(MAX_TERMS * MAX_TERMS * 16)
                    .saturating_add(1),
            ) {
                return false;
            }
            if self.arities.get(&op.opcode) != Some(&(op.inputs.len(), op.outputs.len())) {
                return false;
            }
            let effects = crate::effects::effects(op.opcode);
            if op.effects != effects {
                return false;
            }
            if effects.memory_read || effects.memory_write {
                if op.memory_input != state.memory {
                    return false;
                }
            } else if op.memory_input.is_some() {
                return false;
            }
            if effects.memory_write {
                let Some(memory) = op.memory_output else {
                    return false;
                };
                if !matches!(self.values.get(&memory).map(|v| &v.kind), Some(ValueKind::Memory { operation: Some(id) }) if *id == op.id)
                {
                    return false;
                }
            } else if op.memory_output.is_some() {
                return false;
            }
            if !effects.is_pure() {
                if op.effect_input != state.effect {
                    return false;
                }
                let Some(effect) = op.effect_output else {
                    return false;
                };
                if !matches!(self.values.get(&effect).map(|v| &v.kind), Some(ValueKind::Effect { operation: Some(id) }) if *id == op.id)
                {
                    return false;
                }
                state.effect = Some(effect);
            } else if op.effect_input.is_some() || op.effect_output.is_some() {
                return false;
            }
            for (index, output) in op.outputs.iter().enumerate() {
                if !matches!(
                    self.values.get(output).map(|v| &v.kind),
                    Some(ValueKind::Constant(_))
                ) && !matches!(self.values.get(output).map(|v| &v.kind), Some(ValueKind::Operation { operation, index: i }) if *operation == op.id && *i == index)
                {
                    return false;
                }
            }
            let args: Vec<_> = op.inputs.iter().map(|id| self.value(state, *id)).collect();
            if let Some(&output) = op.outputs.first() {
                let atom = scope.map_or(Atom::Source(output), |header| Atom::Local(header, output));
                let mut result = None;
                if op.opcode == 0x51 && args.len() == 1 {
                    if let Some(address) = &args[0] {
                        result = state.cells.get(address).cloned();
                    }
                } else if (0x5f..=0x7f).contains(&op.opcode) {
                    result = self.value(state, output);
                } else if let Some(args) = args.iter().cloned().collect::<Option<Vec<_>>>() {
                    result = self.calculate(op, &args, state, atom);
                }
                let result = result.unwrap_or_else(|| Expr::atom(atom));
                if op.opcode == 0x3d
                    && let Some(high) = self.return_data_size_bound
                {
                    state.atoms.insert(
                        atom,
                        Range {
                            low: U256::ZERO,
                            high,
                        },
                    );
                }
                state.values.insert(output, result);
            }
            if effects.memory_write {
                let range = self.write_range(op, &args);
                if !self.charge(
                    state
                        .cells
                        .len()
                        .saturating_mul(MAX_TERMS * MAX_TERMS * 16)
                        .saturating_add(1),
                ) {
                    return false;
                }
                if let Some((offset, size)) = range {
                    let cells = std::mem::take(&mut state.cells);
                    state.cells = cells
                        .into_iter()
                        .filter(|(address, _)| {
                            if address == &offset && matches!(op.opcode, 0x52 | 0x53) {
                                return false;
                            }
                            self.disjoint(state, &offset, &size, address, 32) || optimistic
                        })
                        .collect();
                    if op.opcode == 0x52
                        && let Some(Some(value)) = args.get(1)
                        && state.cells.len() < MAX_CELLS
                        && offset.terms.len() <= MAX_TERMS
                    {
                        state.cells.insert(offset, value.clone());
                    }
                } else if !optimistic {
                    state.cells.clear();
                }
                state.memory = op.memory_output;
            }
        }
        // Simplification may retain a JUMPI source operation while lowering
        // its known nonzero/zero decision to Jump/Fallthrough. Recheck that
        // decision from the just-replayed operands before admitting its edge.
        if let Some(op) = block.operations.last().filter(|op| op.opcode == 0x57) {
            let condition = self.value(state, *op.inputs.get(1).unwrap());
            match &block.terminator {
                crate::Terminator::Jump { .. }
                    if condition
                        .as_ref()
                        .is_none_or(|c| state.range(c).low == U256::ZERO) =>
                {
                    return false;
                }
                crate::Terminator::Fallthrough { .. }
                    if condition
                        .as_ref()
                        .is_none_or(|c| state.range(c).high != U256::ZERO) =>
                {
                    return false;
                }
                _ => {}
            }
        }
        true
    }
    pub(super) fn calculate(
        &mut self,
        op: &Operation,
        args: &[Expr],
        state: &mut State,
        atom: Atom,
    ) -> Option<Expr> {
        if evm_bytecode::word::arity(op.opcode) != Some(args.len()) {
            return None;
        }
        if !self.charge(evm_bytecode::word::evaluation_work(op.opcode)) {
            return None;
        }
        if let Some(constants) = args.iter().map(Expr::exact).collect::<Option<Vec<_>>>() {
            return evm_bytecode::word::evaluate(op.opcode, &constants)
                .ok()
                .map(Expr::constant);
        }
        let result = match (op.opcode, args) {
            (0x01, [a, b]) => Some(a.add(b)),
            (0x03, [a, b]) => Some(a.sub(b)),
            (0x02, [a, b]) => a
                .exact()
                .map(|factor| b.scale(factor))
                .or_else(|| b.exact().map(|factor| a.scale(factor))),
            (0x1b, [shift, value]) => shift
                .exact()
                .filter(|s| *s < U256::from(256))
                .and_then(|s| usize::try_from(s).ok())
                .map(|s| value.scale(U256::from(1) << s)),
            _ => None,
        };
        if let Some(result) = result {
            if result.terms.len() > MAX_TERMS {
                return None;
            }
            let ordinary = match (op.opcode, args) {
                (0x01, [a, b]) if a.integral_add(b) => {
                    let (ar, br) = (state.ordinary(a), state.ordinary(b));
                    if let Some(delta) = a
                        .exact()
                        .or_else(|| b.exact())
                        .filter(|d| *d > (U256::MAX >> 1))
                    {
                        let other = if a.exact().is_some() { br } else { ar };
                        let magnitude = U256::ZERO.wrapping_sub(delta);
                        other.and_then(|other| {
                            other
                                .low
                                .checked_sub(magnitude)
                                .zip(other.high.checked_sub(magnitude))
                        })
                    } else {
                        ar.zip(br).and_then(|(ar, br)| {
                            ar.low.checked_add(br.low).zip(ar.high.checked_add(br.high))
                        })
                    }
                }
                (0x03, [a, b])
                    if a.integral_add(&b.scale(U256::MAX))
                        && b.exact().is_none_or(|v| v <= (U256::MAX >> 1)) =>
                {
                    state
                        .ordinary(a)
                        .zip(state.ordinary(b))
                        .and_then(|(ar, br)| {
                            ar.low.checked_sub(br.high).zip(ar.high.checked_sub(br.low))
                        })
                }
                (0x02, [a, b])
                    if a.exact().is_some_and(|v| b.integral_scale(v))
                        || b.exact().is_some_and(|v| a.integral_scale(v)) =>
                {
                    state
                        .ordinary(a)
                        .zip(state.ordinary(b))
                        .and_then(|(ar, br)| {
                            ar.low.checked_mul(br.low).zip(ar.high.checked_mul(br.high))
                        })
                }
                _ => None,
            };
            if let Some((low, high)) = ordinary {
                state.real.insert(result.clone(), Range { low, high });
            }
            return Some(result);
        }
        let range = match (op.opcode, args) {
            (0x10..=0x15, _) => Some(Range {
                low: U256::ZERO,
                high: U256::from(1),
            }),
            (0x16, [a, b]) => Some(Range {
                low: U256::ZERO,
                high: state.range(a).high.min(state.range(b).high),
            }),
            (0x1c, [shift, value]) => {
                shift
                    .exact()
                    .and_then(|s| usize::try_from(s).ok())
                    .map(|s| Range {
                        low: U256::ZERO,
                        high: if s >= 256 {
                            U256::ZERO
                        } else {
                            state.range(value).high >> s
                        },
                    })
            }
            _ => None,
        };
        if matches!(op.opcode, 0x10 | 0x11 | 0x14 | 0x15 | 0x16 | 0x17) {
            state.predicates.insert(
                atom,
                Predicate {
                    opcode: op.opcode,
                    inputs: args.to_vec(),
                },
            );
        }
        if let Some(range) = range {
            if range.low == range.high {
                return Some(Expr::constant(range.low));
            }
            state.atoms.insert(atom, range);
        }
        Some(Expr::atom(atom))
    }
    fn write_range(&self, op: &Operation, args: &[Option<Expr>]) -> Option<(Expr, Expr)> {
        let (offset, size) = match op.opcode {
            0x52 => (0, Expr::constant(U256::from(32))),
            0x53 => (0, Expr::constant(U256::from(1))),
            0x37 | 0x39 | 0x3e | 0x5e => (0, args.get(2)?.clone()?),
            0x3c => (1, args.get(3)?.clone()?),
            0xf1 | 0xf2 => (5, args.get(6)?.clone()?),
            0xf4 | 0xfa => (4, args.get(5)?.clone()?),
            _ => return None,
        };
        Some((args.get(offset)?.clone()?, size))
    }
    pub(super) fn disjoint(
        &self,
        state: &State,
        offset: &Expr,
        size: &Expr,
        cell: &Expr,
        count: usize,
    ) -> bool {
        let size_range = state.range(size);
        if size_range.high == U256::ZERO {
            return true;
        }
        let (Some(a), Some(b)) = (state.ordinary(offset), state.ordinary(cell)) else {
            return false;
        };
        if a.high.checked_add(size_range.high).is_none()
            || b.high.checked_add(U256::from(count)).is_none()
        {
            return false;
        }
        if a.high
            .checked_add(size_range.high)
            .is_some_and(|end| end <= b.low)
            || b.high
                .checked_add(U256::from(count))
                .is_some_and(|end| end <= a.low)
        {
            return true;
        }
        let half = U256::MAX >> 1;
        if a.high > half || b.high > half {
            return false;
        }
        offset
            .difference(cell)
            .and_then(|d| state.ordinary(&d))
            .is_some_and(|delta| delta.low >= U256::from(count) && delta.high <= half)
            || a.high
                .checked_add(size_range.high)
                .is_some_and(|end| end <= half)
                && cell
                    .difference(&offset.add(size))
                    .and_then(|d| state.ordinary(&d))
                    .is_some_and(|delta| delta.high <= half)
    }
    pub(super) fn assume(
        &mut self,
        state: &mut State,
        expr: Expr,
        truth: bool,
        optimistic: bool,
    ) -> bool {
        let mut pending = vec![(expr, truth)];
        let mut seen = std::collections::BTreeSet::new();
        while let Some((expr, truth)) = pending.pop() {
            if !self.charge(MAX_TERMS * MAX_TERMS) {
                return true;
            }
            if !seen.insert((expr.clone(), truth)) {
                continue;
            }
            let range = state.range(&expr);
            if (truth && range.high == U256::ZERO) || (!truth && range.low > U256::ZERO) {
                return false;
            }
            let bound = if truth {
                Range {
                    low: U256::from(1),
                    high: U256::MAX,
                }
            } else {
                Range::exact(U256::ZERO)
            };
            if !self.restrict(state, &expr, bound, optimistic) {
                return false;
            }
            let Some(atom) = expr.single() else {
                continue;
            };
            let Some(predicate) = state.predicates.get(&atom).cloned() else {
                continue;
            };
            match (predicate.opcode, predicate.inputs.as_slice()) {
                (0x15, [value]) => pending.push((value.clone(), !truth)),
                (0x17, [a, b]) if !truth => {
                    pending.push((a.clone(), false));
                    pending.push((b.clone(), false));
                }
                (0x16, [a, b])
                    if truth
                        && state.range(a).high <= U256::from(1)
                        && state.range(b).high <= U256::from(1) =>
                {
                    pending.push((a.clone(), true));
                    pending.push((b.clone(), true));
                }
                (0x10 | 0x11 | 0x14, [a, b]) => {
                    let (a, b) = if predicate.opcode == 0x11 {
                        (b, a)
                    } else {
                        (a, b)
                    };
                    let (ar, br) = (state.range(a), state.range(b));
                    if predicate.opcode == 0x14 {
                        if truth {
                            let Some(common) = ar.intersect(br) else {
                                return false;
                            };
                            if !self.restrict(state, a, common, optimistic)
                                || !self.restrict(state, b, common, optimistic)
                            {
                                return false;
                            }
                        }
                    } else if truth {
                        let (Some(high), Some(low)) = (
                            br.high.checked_sub(U256::from(1)),
                            ar.low.checked_add(U256::from(1)),
                        ) else {
                            return false;
                        };
                        if !self.restrict(
                            state,
                            a,
                            Range {
                                low: U256::ZERO,
                                high,
                            },
                            optimistic,
                        ) || !self.restrict(
                            state,
                            b,
                            Range {
                                low,
                                high: U256::MAX,
                            },
                            optimistic,
                        ) {
                            return false;
                        }
                    } else if !self.restrict(
                        state,
                        a,
                        Range {
                            low: br.low,
                            high: U256::MAX,
                        },
                        optimistic,
                    ) || !self.restrict(
                        state,
                        b,
                        Range {
                            low: U256::ZERO,
                            high: ar.high,
                        },
                        optimistic,
                    ) {
                        return false;
                    }
                }
                _ => {}
            }
        }
        true
    }
    pub(super) fn restrict(
        &mut self,
        state: &mut State,
        expr: &Expr,
        bound: Range,
        optimistic: bool,
    ) -> bool {
        let ordinary = state.ordinary(expr).is_some();
        let Some(bound) = state.range(expr).intersect(bound) else {
            return false;
        };
        if ordinary {
            state.real.insert(expr.clone(), bound);
        }
        state.bounds.insert(expr.clone(), bound);
        // These are mathematical affine inequalities only after a no-wrap
        // interpretation is established (or in the untrusted proposal pass).
        if ordinary || optimistic {
            for (&atom, &coefficient) in &expr.terms {
                if !self.charge(MAX_TERMS * MAX_TERMS) {
                    return true;
                }
                let rest = expr.sub(&Expr::atom(atom).scale(coefficient));
                let negative_rest = rest.terms.is_empty() && rest.constant > (U256::MAX >> 1);
                let rest_negative_size = U256::ZERO.wrapping_sub(rest.constant);
                let Some(rest) = rest.range(&state.atoms) else {
                    continue;
                };
                let negative = coefficient > (U256::MAX >> 1);
                let factor = if negative {
                    U256::ZERO.wrapping_sub(coefficient)
                } else {
                    coefficient
                };
                if factor == U256::ZERO {
                    continue;
                }
                let (low, high) = if negative_rest {
                    if negative {
                        continue;
                    }
                    let Some(low) = bound.low.checked_add(rest_negative_size) else {
                        continue;
                    };
                    (
                        low,
                        Some(
                            bound
                                .high
                                .checked_add(rest_negative_size)
                                .unwrap_or(U256::MAX),
                        ),
                    )
                } else if negative {
                    (
                        rest.low.checked_sub(bound.high).unwrap_or(U256::ZERO),
                        rest.high.checked_sub(bound.low),
                    )
                } else {
                    (
                        bound.low.checked_sub(rest.high).unwrap_or(U256::ZERO),
                        bound.high.checked_sub(rest.low),
                    )
                };
                let Some(high) = high else {
                    return false;
                };
                let low = low / factor + U256::from(u8::from(low % factor != U256::ZERO));
                let Some(range) = state
                    .atoms
                    .get(&atom)
                    .copied()
                    .unwrap_or(Range::FULL)
                    .intersect(Range {
                        low,
                        high: high / factor,
                    })
                else {
                    return false;
                };
                state.atoms.insert(atom, range);
            }
        }
        true
    }
}
