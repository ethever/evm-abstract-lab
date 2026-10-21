//! Explicit root-block projection of the verified native frame SSA.

use super::{Block, Phi, PhiInput, Ssa, SsaError, SsaInvariantKind as Kind, error::invariant};
use crate::{analysis::EvmAnalysis, domain::WordElement};

pub(super) fn build<N: WordElement>(analysis: &EvmAnalysis<N>) -> Result<Ssa, SsaError> {
    let execution = super::build_world(analysis.execution())?;
    let mut blocks = Vec::with_capacity(analysis.states().len());
    for state in analysis.states() {
        super::checkpoint()?;
        let native_state = analysis
            .execution_state_id(state.id)
            .ok_or_else(|| invariant(Kind::BlockIdentity).state(state.id))?;
        let native = execution
            .blocks()
            .get(native_state)
            .ok_or_else(|| invariant(Kind::BlockIdentity).state(state.id))?;
        if native.exit_frames.len() != 1 || native.phis.iter().any(|phi| phi.frame != 0) {
            return Err(invariant(Kind::FrameParameterIdentity).state(native_state));
        }
        blocks.push(Block {
            state: state.id,
            native_state,
            phis: native
                .phis
                .iter()
                .map(|phi| Phi {
                    result: phi.result,
                    slot: phi.slot,
                    inputs: phi
                        .inputs
                        .iter()
                        .map(|(edge, value)| PhiInput {
                            edge: *edge,
                            predecessor: analysis.execution().edges()[*edge].from,
                            value: *value,
                        })
                        .collect(),
                })
                .collect(),
            instructions: native.instructions.clone(),
            exit_stack: native.exit_frames[0].clone(),
        });
    }
    Ok(Ssa {
        blocks,
        value_count: execution.value_count(),
        execution,
    })
}
