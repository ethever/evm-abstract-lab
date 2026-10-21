//! Check execution preservation separately from justified value normalization.

use super::{
    NormalizationInvariant, NormalizationInvariantKind as Kind, NormalizedSsa, build, invariant,
};
use crate::domain::WordElement;
use crate::{
    analysis::{EvmWorldAnalysis, Status},
    ssa::{PartialWorldSsa, SsaError, ValueId},
};
use std::collections::BTreeSet;

pub(super) fn verify<N: WordElement>(
    ir: &NormalizedSsa<N>,
    analysis: &EvmWorldAnalysis<N>,
    budget: &mut crate::resource::WorkBudget,
) -> Result<(), SsaError> {
    crate::ssa::pay(budget, 0)?;
    if ir.complete {
        // Preserve the complete verifier's original bytecode and effect contracts.
        crate::ssa::world::build(analysis, Some(&mut *budget))?;
    }
    let raw = crate::ssa::build_partial_world(analysis, budget)?;
    against_raw(ir, &raw, analysis, budget)
}

pub(super) fn against_raw<N: WordElement>(
    ir: &NormalizedSsa<N>,
    raw: &PartialWorldSsa<N>,
    analysis: &EvmWorldAnalysis<N>,
    budget: &mut crate::resource::WorkBudget,
) -> Result<(), SsaError> {
    crate::ssa::pay(budget, 0)?;
    crate::ssa::work::normalized_verify(ir, budget)?;
    if ir.status != raw.status() || ir.complete != (analysis.status() == Status::Converged) {
        return Err(invariant(Kind::Coverage));
    }
    let definitions = ir
        .blocks
        .iter()
        .flat_map(|block| block.definitions(|step| &step.origin))
        .map(|(value, _)| value)
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
    let expected = build::normalize(raw, analysis, ir.complete, budget)?;
    if ir.value_count != expected.value_count || ir.blocks.len() != expected.blocks.len() {
        return Err(invariant(Kind::ValueNames));
    }
    if ir.effect_count != expected.effect_count {
        return Err(invariant(Kind::Effects));
    }
    if ir.deferred_edges != expected.deferred_edges
        || !crate::ssa::partial::same_frontiers(&ir.frontiers, &expected.frontiers, budget)?
    {
        return Err(invariant(Kind::Coverage));
    }
    for (block, original) in ir.blocks.iter().zip(&expected.blocks) {
        crate::ssa::pay(budget, 0)?;
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
    for fact in ir.facts.iter().chain(&expected.facts) {
        if let Some(provenance) = &fact.provenance {
            crate::ssa::pay(budget, provenance.origins().work_size())?;
        }
        if let Some(numeric) = &fact.numeric {
            crate::ssa::pay(budget, numeric.work_size())?;
        }
    }
    if ir.facts != expected.facts {
        return Err(invariant(Kind::Facts));
    }
    if ir.mappings != expected.mappings {
        return Err(invariant(Kind::Mappings));
    }
    crate::ssa::pay(budget, 0)?;
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
