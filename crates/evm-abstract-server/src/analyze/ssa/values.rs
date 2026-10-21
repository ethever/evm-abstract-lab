//! Lossless projection of final SSA definitions and their independently scoped
//! facts. Original evidence, equality and computation dependencies stay distinct.

use evm_abstract::{
    analysis::{EvmWorldAnalysis, control::Control},
    domain::WordElement,
    ssa,
};
use evm_abstract_protocol as api;

use super::super::{checkpoint, symbolic::Pool, value};

fn operand(source: &ssa::ValueOperand) -> api::SsaOperand {
    match source {
        ssa::ValueOperand::Value(id) => api::SsaOperand::Value(*id),
        ssa::ValueOperand::Constant(word) => api::SsaOperand::Constant(value::word(*word)),
    }
}

pub(super) fn operation(source: &ssa::NormalizedOperation, name: &str) -> api::SsaOperation {
    match source {
        ssa::NormalizedOperation::Opcode { opcode, operands } => {
            api::SsaOperation::Opcode(api::SsaOpcodeOperation {
                opcode: *opcode,
                name: name.into(),
                operands: operands.iter().map(operand).collect(),
            })
        }
        ssa::NormalizedOperation::Constant { value: word } => {
            api::SsaOperation::Constant(value::word(*word))
        }
        ssa::NormalizedOperation::Copy { value } => api::SsaOperation::Copy(operand(value)),
    }
}

pub(super) fn report<N: WordElement>(
    graph: &EvmWorldAnalysis<N>,
    ir: &ssa::NormalizedSsa<N>,
    control: &Control,
    pool: &mut Pool<'_>,
) -> Result<Vec<api::SsaValue>, api::ApiError> {
    // Group once: real programs contain many definitions, so filtering every
    // fact for every value would make conversion quadratic.
    pool.charge(ir.value_count())?;
    let mut facts = vec![Vec::new(); ir.value_count()];
    for source in ir.facts() {
        checkpoint(control)?;
        pool.charge(1)?;
        facts[source.value].push(fact(source, pool)?);
    }
    ir.mappings()
        .iter()
        .map(|mapping| {
            checkpoint(control)?;
            pool.charge(mapping.dependencies.len().saturating_add(1))?;
            let origin = match mapping.origin {
                ssa::ValueOrigin::Phi { state, frame, slot } => {
                    api::SsaValueOrigin::Phi(api::SsaPhiOrigin { state, frame, slot })
                }
                ssa::ValueOrigin::Instruction {
                    state, index, pc, ..
                } => api::SsaValueOrigin::Instruction(api::SsaInstructionOrigin {
                    state,
                    instruction_index: index,
                    pc,
                }),
                ssa::ValueOrigin::Transition { edge } => {
                    let native = &graph.edges()[edge];
                    api::SsaValueOrigin::Transition(api::SsaTransitionOrigin {
                        edge,
                        from: native.from,
                        to: native.to,
                    })
                }
            };
            let transformation = match mapping.transformation {
                ssa::ValueTransformation::Original => api::SsaTransformation::Original,
                ssa::ValueTransformation::Literal => api::SsaTransformation::Literal,
                ssa::ValueTransformation::ConstantFold => api::SsaTransformation::ConstantFold,
                ssa::ValueTransformation::AlgebraicIdentity => {
                    api::SsaTransformation::AlgebraicIdentity
                }
                ssa::ValueTransformation::AnalyzedConstant => {
                    api::SsaTransformation::AnalyzedConstant
                }
            };
            let mut retained = std::mem::take(&mut facts[mapping.value]);
            if matches!(origin, api::SsaValueOrigin::Phi(ref phi)
                if ir.blocks()[phi.state].coverage == ssa::PartialBlockCoverage::Unexecuted)
            {
                for fact in &mut retained {
                    if fact.status == api::SsaFactStatus::Uncollected {
                        fact.status = api::SsaFactStatus::Unexecuted;
                    }
                }
            }
            Ok(api::SsaValue {
                id: mapping.value,
                original_value: mapping.original_value,
                origin,
                facts: retained,
                transformation,
                equivalent: mapping.equivalent,
                dependencies: mapping.dependencies.clone(),
            })
        })
        .collect()
}

fn fact<N: WordElement>(
    source: &ssa::ValueFact<N>,
    pool: &mut Pool<'_>,
) -> Result<api::SsaValueFacts, api::ApiError> {
    let value = source
        .numeric
        .as_ref()
        .map(|numeric| {
            pool.charge(numeric.work_size())?;
            let mut info = value::observation(&numeric.observation());
            info.summary = numeric.to_string();
            if let Some(provenance) = &source.provenance {
                info.origins = provenance
                    .origins()
                    .sources()
                    .map(|origins| origins.iter().copied().map(value::origin).collect());
                info.code_address_role = provenance.is_code_address();
            }
            Ok(info)
        })
        .transpose()?;
    Ok(api::SsaValueFacts {
        status: match source.validity {
            ssa::FactValidity::Current => api::SsaFactStatus::Current,
            ssa::FactValidity::Uncollected => api::SsaFactStatus::Uncollected,
            ssa::FactValidity::Stale => api::SsaFactStatus::Stale,
        },
        scope: match source.scope {
            ssa::FactScope::Definition => api::SsaFactScope::Definition,
            ssa::FactScope::State { state } => api::SsaFactScope::State(state),
        },
        source: match source.source {
            ssa::FactSource::Entry => api::SsaFactSource::Entry,
            ssa::FactSource::Instruction => api::SsaFactSource::Execution,
            ssa::FactSource::Transition => api::SsaFactSource::Transition,
            ssa::FactSource::Literal => api::SsaFactSource::Literal,
            ssa::FactSource::ConstantFold => api::SsaFactSource::Propagation,
            ssa::FactSource::AlgebraicIdentity => api::SsaFactSource::Algebraic,
        },
        constant: source.constant.map(value::word),
        value,
    })
}
