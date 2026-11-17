//! Reprove decided branches from source ranges, never published numeric facts.
//! Range queries have the same bounded shape as the existing AND/SHR transfers
//! and are funded by source replay's per-operation relation reservation.
use super::{Expr, State, U256};

pub(super) fn constant(state: &State, opcode: u8, args: &[Expr]) -> Option<bool> {
    match (opcode, args) {
        (0x15, [value]) => {
            let range = state.range(value);
            if range.high == U256::ZERO {
                Some(true)
            } else if range.low > U256::ZERO {
                Some(false)
            } else {
                None
            }
        }
        (0x10 | 0x11 | 0x14, [left, right]) => {
            let (a, b) = (state.range(left), state.range(right));
            match opcode {
                0x10 if a.high < b.low => Some(true),
                0x10 if a.low >= b.high => Some(false),
                0x11 if a.low > b.high => Some(true),
                0x11 if a.high <= b.low => Some(false),
                0x14 if left == right || a.low == a.high && a == b => Some(true),
                0x14 if a.high < b.low || b.high < a.low => Some(false),
                _ => None,
            }
        }
        // An unsigned interval is insufficient for a signed ordering claim.
        _ => None,
    }
}

#[cfg(test)]
mod tests;
