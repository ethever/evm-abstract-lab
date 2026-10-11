//! Check execution preservation separately from justified value normalization.

use super::{
    NormalizationInvariant, NormalizationInvariantKind as Kind, NormalizedSsa, build, invariant,
};
use crate::{
    analysis::{Status, WorldAnalysis},
    ssa::{PartialWorldSsa, SsaError, ValueId},
};
use std::collections::BTreeSet;

pub(super) fn verify(ir: &NormalizedSsa, analysis: &WorldAnalysis) -> Result<(), SsaError> {
    crate::ssa::checkpoint()?;
    if ir.complete {
        // Preserve the complete verifier's original bytecode and effect contracts.
        crate::ssa::build_world(analysis)?;
    }
    let raw = crate::ssa::build_partial_world(analysis)?;
    against_raw(ir, &raw, analysis)
}

pub(super) fn against_raw(
    ir: &NormalizedSsa,
    raw: &PartialWorldSsa,
    analysis: &WorldAnalysis,
) -> Result<(), SsaError> {
    crate::ssa::checkpoint()?;
    if ir.status != raw.status() || ir.complete != (analysis.status() == Status::Converged) {
        return Err(invariant(Kind::Coverage));
    }
    let definitions = ir
        .blocks
        .iter()
        .flat_map(|block| {
            block.phis.iter().map(|phi| phi.result).chain(
                block
                    .instructions
                    .iter()
                    .flat_map(|step| step.origin.results.iter().copied()),
            )
        })
        .chain(
            ir.transitions
                .iter()
                .filter_map(|transition| transition.result),
        )
        .collect::<Vec<_>>();
    let distinct = definitions.iter().copied().collect::<BTreeSet<_>>();
    if definitions.len() != ir.value_count
        || distinct.len() != ir.value_count
        || distinct.into_iter().ne(0..ir.value_count)
    {
        return Err(invariant(Kind::ValueNames));
    }
    // Proofs are replayed from the validated raw receipt, not from the artifact's
    // own facts or altered operations. In particular a State observation cannot
    // certify a Definition rewrite by changing its serialized scope.
    let expected = build::normalize(raw, analysis, ir.complete)?;
    if ir.value_count != expected.value_count || ir.blocks.len() != expected.blocks.len() {
        return Err(invariant(Kind::ValueNames));
    }
    if ir.effect_count != expected.effect_count {
        return Err(invariant(Kind::Effects));
    }
    if ir.deferred_edges != expected.deferred_edges
        || ir.frontiers.len() != expected.frontiers.len()
        || ir.frontiers.iter().zip(&expected.frontiers).any(|(a, b)| {
            a.from != b.from || a.target != b.target || a.pc != b.pc || a.reason != b.reason
        })
    {
        return Err(invariant(Kind::Coverage));
    }
    for (block, original) in ir.blocks.iter().zip(&expected.blocks) {
        crate::ssa::checkpoint()?;
        if block.state != original.state
            || block.coverage != original.coverage
            || block.open_incoming != original.open_incoming
            || block.incoming_complete != original.incoming_complete
            || block.instructions.len() != original.instructions.len()
        {
            return Err(located(Kind::Coverage, Some(block.state), None, None));
        }
        if block.phis != original.phis || block.exit_frames != original.exit_frames {
            return Err(located(Kind::Origin, Some(block.state), None, None));
        }
        if block.effect != original.effect || block.exit_effect != original.exit_effect {
            return Err(located(Kind::Effects, Some(block.state), None, None));
        }
        for (step, source) in block.instructions.iter().zip(&original.instructions) {
            if step.origin != source.origin {
                return Err(located(
                    Kind::Origin,
                    Some(block.state),
                    Some(step.origin.pc),
                    None,
                ));
            }
            if step.progress != source.progress {
                return Err(located(
                    Kind::Coverage,
                    Some(block.state),
                    Some(step.origin.pc),
                    None,
                ));
            }
            if step.effect_input != source.effect_input
                || step.effect_result != source.effect_result
            {
                return Err(located(
                    Kind::Effects,
                    Some(block.state),
                    Some(step.origin.pc),
                    None,
                ));
            }
            if step.operation != source.operation {
                return Err(located(
                    Kind::Operation,
                    Some(block.state),
                    Some(step.origin.pc),
                    step.origin.results.first().copied(),
                ));
            }
        }
    }
    if ir.transitions != expected.transitions {
        return Err(invariant(Kind::Transitions));
    }
    if ir.facts != expected.facts {
        return Err(invariant(Kind::Facts));
    }
    if ir.mappings != expected.mappings {
        return Err(invariant(Kind::Mappings));
    }
    Ok(())
}

fn located(
    kind: Kind,
    state: Option<usize>,
    pc: Option<usize>,
    value: Option<ValueId>,
) -> SsaError {
    SsaError::Normalization(Box::new(NormalizationInvariant {
        kind,
        state,
        pc,
        value,
    }))
}
