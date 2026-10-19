//! Allocate final IDs, project scoped facts, and normalize retained definitions.

use super::{
    FactScope, FactSource, FactValidity, NormalizationInvariantKind as Kind, NormalizedBlock,
    NormalizedInstruction, NormalizedOperation, NormalizedSsa, ValueFact, ValueMapping,
    ValueOperand, ValueOrigin, ValueTransformation, invariant, simplify,
};
use crate::{
    analysis::WorldAnalysis,
    domain::{Domain, NumericValue, provenance::Provenance},
    ssa::{PartialBlockCoverage, PartialWorldSsa, SsaError, ValueId},
};

pub(super) fn normalize(
    raw: &PartialWorldSsa,
    analysis: &WorldAnalysis,
    complete: bool,
) -> Result<NormalizedSsa, SsaError> {
    crate::ssa::checkpoint()?;
    let (names, mut mappings) = allocate(raw)?;
    let mut blocks = Vec::with_capacity(raw.blocks().len());
    for block in raw.blocks() {
        crate::ssa::checkpoint()?;
        let mut block = block.try_map_instructions(|_, step| {
            let mut origin = step.instruction.clone();
            remap(origin.values_mut(), &names);
            let operation = NormalizedOperation::Opcode {
                opcode: origin.opcode,
                operands: origin
                    .operands
                    .iter()
                    .copied()
                    .map(ValueOperand::Value)
                    .collect(),
            };
            Ok::<_, SsaError>(NormalizedInstruction {
                origin,
                progress: step.progress,
                effect_input: step.effect_input,
                effect_result: step.effect_result,
                operation,
            })
        })?;
        remap(block.frame_values_mut(), &names);
        blocks.push(block);
    }
    let mut transitions = raw.transitions().to_vec();
    for transition in &mut transitions {
        remap(transition.values_mut(), &names);
    }
    let mut facts = observed_facts(&blocks, &transitions, analysis, complete)?;
    let mut known = vec![None; mappings.len()];
    for fact in &facts {
        if fact.scope == FactScope::Definition && fact.validity == FactValidity::Current {
            known[fact.value] = fact.constant;
        }
    }
    let domain = Domain::from_spec(analysis.domain_spec());
    for block in &mut blocks {
        crate::ssa::checkpoint()?;
        for phi in &block.phis {
            mappings[phi.result].dependencies =
                phi.inputs.iter().map(|(_, value)| *value).collect();
        }
        for instruction in &mut block.instructions {
            crate::ssa::checkpoint()?;
            let result = simplify::instruction(instruction, &known, &mappings, domain);
            instruction.operation = result.operation;
            for value in &instruction.origin.results {
                let mapping = &mut mappings[*value];
                mapping.transformation = result.transformation;
                mapping.equivalent = result.equivalent;
                mapping.dependencies = instruction.origin.operands.clone();
                if let Some((constant, source)) = result.derived {
                    known[*value] = Some(constant);
                    facts.push(ValueFact {
                        value: *value,
                        scope: FactScope::Definition,
                        validity: FactValidity::Current,
                        source,
                        numeric: Some(NumericValue::constant(constant)),
                        // A numeric proof must not upgrade an observed source's scope.
                        provenance: (source == FactSource::Literal).then(Provenance::constant),
                        constant: Some(constant),
                    });
                }
            }
        }
    }
    for transition in &transitions {
        if let Some(value) = transition.result {
            mappings[value].dependencies = transition.operands.clone();
        }
    }
    Ok(NormalizedSsa {
        status: raw.status(),
        complete,
        blocks,
        transitions,
        deferred_edges: raw.deferred_edges().to_vec(),
        frontiers: raw.frontiers().to_vec(),
        value_count: mappings.len(),
        effect_count: raw.effect_count(),
        facts,
        mappings,
    })
}

fn allocate(raw: &PartialWorldSsa) -> Result<(Vec<ValueId>, Vec<ValueMapping>), SsaError> {
    let mut names = vec![usize::MAX; raw.value_count()];
    let mut mappings = Vec::with_capacity(raw.value_count());
    for block in raw.blocks() {
        crate::ssa::checkpoint()?;
        for (value, origin) in block.definitions(|step| &step.instruction) {
            register(&mut names, &mut mappings, value, origin)?;
        }
    }
    for transition in raw.transitions() {
        if let Some(value) = transition.result {
            register(
                &mut names,
                &mut mappings,
                value,
                ValueOrigin::Transition {
                    edge: transition.edge,
                },
            )?;
        }
    }
    if mappings.len() != raw.value_count() || names.contains(&usize::MAX) {
        return Err(invariant(Kind::ValueNames));
    }
    Ok((names, mappings))
}

fn register(
    names: &mut [ValueId],
    mappings: &mut Vec<ValueMapping>,
    original_value: ValueId,
    origin: ValueOrigin,
) -> Result<(), SsaError> {
    let name = names
        .get_mut(original_value)
        .ok_or_else(|| invariant(Kind::ValueNames))?;
    if *name != usize::MAX {
        return Err(invariant(Kind::ValueNames));
    }
    *name = mappings.len();
    mappings.push(ValueMapping {
        value: *name,
        original_value,
        origin,
        equivalent: None,
        dependencies: Vec::new(),
        transformation: ValueTransformation::Original,
    });
    Ok(())
}

fn remap<'a>(values: impl IntoIterator<Item = &'a mut ValueId>, names: &[ValueId]) {
    for value in values {
        *value = names[*value];
    }
}

fn observed_facts(
    blocks: &[NormalizedBlock],
    transitions: &[crate::ssa::Transition],
    analysis: &WorldAnalysis,
    complete: bool,
) -> Result<Vec<ValueFact>, SsaError> {
    let mut facts = Vec::new();
    for block in blocks {
        crate::ssa::checkpoint()?;
        let state = &analysis.states()[block.state];
        let validity = validity(block.coverage);
        let scope = scope(block.state, complete, block.coverage);
        let frames = state.entry.call_stack.iter().collect::<Vec<_>>();
        for phi in &block.phis {
            let value = &frames[phi.frame].stack[phi.slot];
            facts.push(observation(
                phi.result,
                scope,
                validity,
                FactSource::Entry,
                Some(value.numeric().clone()),
                Some(value.provenance().clone()),
            ));
        }
        let captured = state
            .execution_evidence()
            .and_then(|evidence| evidence.instruction_values());
        for (index, step) in block.instructions.iter().enumerate() {
            let snapshot = captured.and_then(|values| {
                values
                    .binary_search_by_key(&index, |value| value.instruction_index())
                    .ok()
                    .map(|index| &values[index])
            });
            for result in &step.origin.results {
                facts.push(observation(
                    *result,
                    scope,
                    if snapshot.is_some() {
                        validity
                    } else {
                        FactValidity::Uncollected
                    },
                    FactSource::Instruction,
                    snapshot.map(|value| value.numeric().clone()),
                    snapshot.map(|value| value.provenance().clone()),
                ));
            }
        }
    }
    for transition in transitions {
        crate::ssa::checkpoint()?;
        if let Some(result) = transition.result {
            let target = analysis.edges()[transition.edge].to;
            let destination = &analysis.states()[target];
            let value = destination.entry.active().stack.last();
            facts.push(observation(
                result,
                scope(target, complete, blocks[target].coverage),
                validity(blocks[target].coverage),
                FactSource::Transition,
                value.map(|value| value.numeric().clone()),
                value.map(|value| value.provenance().clone()),
            ));
        }
    }
    Ok(facts)
}

fn observation(
    value: ValueId,
    scope: FactScope,
    validity: FactValidity,
    source: FactSource,
    numeric: Option<NumericValue>,
    provenance: Option<Provenance>,
) -> ValueFact {
    let constant = numeric.as_ref().and_then(NumericValue::singleton);
    ValueFact {
        value,
        scope,
        validity,
        source,
        numeric,
        provenance,
        constant,
    }
}

fn validity(coverage: PartialBlockCoverage) -> FactValidity {
    match coverage {
        PartialBlockCoverage::Current => FactValidity::Current,
        PartialBlockCoverage::Stale => FactValidity::Stale,
        PartialBlockCoverage::Unexecuted => FactValidity::Uncollected,
    }
}

fn scope(state: usize, complete: bool, coverage: PartialBlockCoverage) -> FactScope {
    if complete && coverage == PartialBlockCoverage::Current {
        FactScope::Definition
    } else {
        FactScope::State { state }
    }
}
