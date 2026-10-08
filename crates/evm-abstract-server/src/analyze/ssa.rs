use evm_abstract::{analysis, ssa};
use evm_abstract_protocol::{
    BlockCoverage, DeferredEdge, DeferredReason, EffectInput, InstructionProgress, Phi, PhiInput,
    SsaBlock, SsaInstruction, SsaReport, SsaTransition,
};

pub(super) fn report(graph: &analysis::WorldAnalysis) -> Result<SsaReport, ssa::SsaError> {
    let complete = graph.status() == analysis::Status::Converged;
    // The complete flag requires the stronger closed-world verifier. The shared
    // presentation below also retains phases and effect dependencies for each row.
    if complete {
        ssa::build_world(graph)?;
    }
    let ir = ssa::build_partial_world(graph)?;
    Ok(SsaReport {
        complete,
        value_count: ir.value_count(),
        effect_count: ir.effect_count(),
        blocks: ir
            .blocks()
            .iter()
            .map(|block| SsaBlock {
                state: block.state,
                coverage: match block.coverage {
                    ssa::PartialBlockCoverage::Unexecuted => BlockCoverage::Unexecuted,
                    ssa::PartialBlockCoverage::Stale => BlockCoverage::Stale,
                    ssa::PartialBlockCoverage::Current => BlockCoverage::Current,
                },
                incoming_complete: block.incoming_complete,
                phis: block
                    .phis
                    .iter()
                    .map(|phi| Phi {
                        result: phi.result,
                        frame: phi.frame,
                        slot: phi.slot,
                        inputs: phi
                            .inputs
                            .iter()
                            .map(|(edge, value)| PhiInput {
                                edge: *edge,
                                predecessor: graph.edges()[*edge].from,
                                value: *value,
                            })
                            .collect(),
                    })
                    .collect(),
                instructions: block
                    .instructions
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let source = &item.instruction;
                        let name = graph.states()[block.state]
                            .program()
                            .and_then(|program| {
                                program
                                    .blocks()
                                    .get(graph.states()[block.state].active().basic_block_index)
                            })
                            .and_then(|source| source.instructions.get(index))
                            .map_or("UNKNOWN", |instruction| instruction.name());
                        SsaInstruction {
                            pc: source.pc,
                            opcode: source.opcode,
                            name: name.into(),
                            immediate: source.immediate.map(|value| format!("0x{value:x}")),
                            operands: source.operands.clone(),
                            results: source.results.clone(),
                            fault: source.fault,
                            progress: match item.progress {
                                analysis::InstructionProgress::Started => {
                                    InstructionProgress::Started
                                }
                                analysis::InstructionProgress::OperandsConsumed => {
                                    InstructionProgress::OperandsConsumed
                                }
                                analysis::InstructionProgress::Completed => {
                                    InstructionProgress::Completed
                                }
                                analysis::InstructionProgress::Dispatched => {
                                    InstructionProgress::Dispatched
                                }
                                analysis::InstructionProgress::Faulted => {
                                    InstructionProgress::Faulted
                                }
                            },
                            effect_input: item.effect_input,
                            effect_result: item.effect_result,
                        }
                    })
                    .collect(),
                exit_frames: block.exit_frames.clone(),
                effect: block.effect.result,
                effect_inputs: block
                    .effect
                    .inputs
                    .iter()
                    .map(|input| EffectInput {
                        edge: input.transition,
                        effect: input.effect,
                    })
                    .collect(),
                exit_effect: block.exit_effect,
                open_incoming: block.open_incoming.clone(),
            })
            .collect(),
        transitions: ir
            .transitions()
            .iter()
            .map(|item| SsaTransition {
                edge: item.edge,
                kind: super::mapping::edge(item.kind),
                operands: item.operands.clone(),
                stacks: item.stacks.clone(),
                effect_input: item.effect_input,
                effect_result: item.effect_result,
                result: item.result,
            })
            .collect(),
        deferred_edges: ir
            .deferred_edges()
            .iter()
            .map(|item| DeferredEdge {
                edge: item.edge,
                reason: match item.reason {
                    ssa::DeferredEdgeReason::SourceUnexecuted => DeferredReason::SourceUnexecuted,
                    ssa::DeferredEdgeReason::SourceStale => DeferredReason::SourceStale,
                    ssa::DeferredEdgeReason::NotInExecutionEvidence => {
                        DeferredReason::NotInExecutionEvidence
                    }
                    ssa::DeferredEdgeReason::SourcePendingInstruction => {
                        DeferredReason::SourcePendingInstruction
                    }
                },
            })
            .collect(),
    })
}
