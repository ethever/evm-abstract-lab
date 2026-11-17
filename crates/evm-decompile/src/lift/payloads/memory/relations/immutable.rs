//! Repeated observations share identity only for source-qualified frame inputs.
//! This replay owns one EVM frame; opaque CALL/CREATE operations do not inline
//! another frame. Dynamic observations and inexact calldata offsets stay fresh.
use super::{Atom, Expr, MAX_TERMS, Operation, Source, State, U256};
use evm_domain::identity::Symbol;

pub(super) fn input(op: &Operation, args: &[Option<Expr>]) -> Option<Expr> {
    let symbol = match (op.opcode, args) {
        (0x35, [Some(offset)]) => Symbol::CalldataWord(offset.exact()?),
        (0x42, []) => Symbol::Timestamp,
        _ => return None,
    };
    Some(Expr::atom(Atom::Immutable(symbol)))
}

impl<P: FnMut(usize) -> bool> Source<'_, P> {
    /// Reuse an already proved predicate, not an ordering guessed from two
    /// unrestricted input intervals. Its guard must survive every reaching
    /// state join, and all operand identities must be immutable in this frame.
    pub(super) fn immutable_predicate(
        &mut self,
        opcode: u8,
        args: &[Expr],
        state: &State,
    ) -> Option<Expr> {
        if !matches!(opcode, 0x10 | 0x11 | 0x14..=0x17)
            || !args
                .iter()
                .all(|arg| arg.terms.keys().all(|a| matches!(a, Atom::Immutable(_))))
        {
            return None;
        }
        // Each stored predicate has at most two bounded affine operands. Quote
        // the complete equality/range scan before reading any predicate record.
        if !self.charge(
            state
                .predicates
                .len()
                .saturating_mul(MAX_TERMS * MAX_TERMS * 4)
                .saturating_add(1),
        ) {
            return None;
        }
        for (atom, predicate) in &state.predicates {
            if predicate.opcode != opcode || predicate.inputs != args {
                continue;
            }
            let previous = Expr::atom(*atom);
            let range = state.range(&previous);
            if range.low == range.high {
                return Some(Expr::constant(range.low));
            }
            if range.low > U256::ZERO {
                // A nonzero bitwise word need not equal the boolean word one.
                return Some(previous);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;
