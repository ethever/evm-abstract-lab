//! A frame rule proved from the complete captured runtime instruction stream.
//!
//! The cached proof belongs to a procedure's immutable actual program identity.
//! It says nothing about execution/closure, return data or entry CALL effects.
//! Successful non-creation application keeps the actual post-entry store;
//! failures still restore each invocation's original child checkpoint.

use crate::{
    analysis::{FrameCode, MachineError, OutcomeKind, PolicyContext},
    bytecode::Program,
    domain::EvmNumericAlgebra,
};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum StoreEffect {
    #[default]
    MayChange,
    Unchanged(Program),
}

impl StoreEffect {
    pub(super) const MATCH_WORK: usize = 32;

    /// Creation completion can install code even when its instructions do not
    /// write storage. Check the actual continuation again at each application.
    /// The caller prepays the constant metadata comparisons. Retaining Program
    /// keeps its immutable byte allocation alive, so comparing the borrowed
    /// byte slice identity cannot suffer address reuse. Fork plus identical
    /// source slices prove the same code for this property; empty slices also
    /// describe the same empty code. Independently decoded code may decline.
    /// No externally selected region key or report hash supplies this proof.
    pub(super) fn keeps_store(
        &self,
        code: FrameCode,
        mode: FrameCode,
        program: Option<&Program>,
        kind: OutcomeKind,
        creation: bool,
    ) -> bool {
        let Self::Unchanged(proven) = self else {
            return false;
        };
        !creation
            && kind == OutcomeKind::Return
            && code == FrameCode::Runtime
            && mode == FrameCode::Runtime
            && program.is_some_and(|actual| {
                actual.fork() == proven.fork() && std::ptr::eq(actual.bytes(), proven.bytes())
            })
    }
}

/// Scan once before retaining a new procedure. No observed CFG or numeric
/// equality is a proof: all decoded blocks, including unreachable code, count.
/// A policy may merge code identities. Every application therefore checks the
/// actual invocation against the retained immutable program independently of
/// policy keys and any growth of the procedure's prepared input.
pub(super) fn classify<D: EvmNumericAlgebra>(
    code: FrameCode,
    mode: FrameCode,
    program: Option<&Program>,
    creation: bool,
    cx: &mut PolicyContext<'_, D>,
) -> Result<StoreEffect, MachineError> {
    cx.reserve(1)?;
    if creation || code != FrameCode::Runtime || mode != FrameCode::Runtime {
        return Ok(StoreEffect::MayChange);
    }
    let Some(program) = program else {
        return Ok(StoreEffect::MayChange);
    };
    // Cached byte length bounds the complete decoded scan, including PUSH data.
    // A refusal cannot create a proof or a partially admitted procedure.
    cx.reserve(program.byte_len().saturating_mul(2).saturating_add(1))?;
    let unchanged = program
        .blocks()
        .iter()
        .flat_map(|block| &block.instructions)
        .all(|instruction| instruction.is_valid() && store_free_opcode(instruction.opcode));
    let effect = if unchanged {
        // One shared Program handle and its eventual release; no native scalar
        // is cloned by the proof. The scan fee covers the byte-sized traversal.
        cx.reserve(2)?;
        StoreEffect::Unchanged(program.clone())
    } else {
        StoreEffect::MayChange
    };
    cx.reserve(0)?;
    Ok(effect)
}

fn store_free_opcode(op: u8) -> bool {
    // Positive current vocabulary. SSTORE/TSTORE, logs, lifecycle operations,
    // creation and EVERY child call (including STATICCALL) are excluded. Thus
    // no unknown implementation, callback or nested touch effect is discarded.
    // PUSH immediates are absent from the decoded opcode stream.
    matches!(op,
        0x00..=0x0b       // STOP and arithmetic, through SIGNEXTEND
        | 0x10..=0x1e    // comparison/bit operations, through CLZ
        | 0x20           // KECCAK256
        | 0x30..=0x4a    // current environment/account observations
        | 0x50..=0x54    // POP, memory load/stores, SLOAD
        | 0x56..=0x5c    // control/frame observations and TLOAD
        | 0x5e..=0x9f    // MCOPY, PUSH0..PUSH32, DUP/SWAP
        | 0xf3 | 0xfd    // RETURN / REVERT
    )
}
