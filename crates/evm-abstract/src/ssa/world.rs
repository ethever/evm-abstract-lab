//! SSA over the whole suspended-frame machine. Stack parameters are indexed by
//! frame and edge; a single effect bundle carries memory, calldata, returndata,
//! transaction storage, balances, logs and rollback checkpoints. This is a
//! deliberately coarse effect SSA, not an alias-partitioned MemorySSA.

use super::{Instruction, SsaError, ValueId};
use crate::analysis::{MachineEdgeKind, Status, WorldAnalysis};
use revm_bytecode::opcode;
use serde::Serialize;

mod verify;

/// A name for the complete abstract machine's effects and saved checkpoints.
pub type EffectId = usize;

/// One stack parameter in one active or suspended frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FramePhi {
    /// Oldest caller first.
    pub frame: usize,
    /// Bottom-to-top stack position.
    pub slot: usize,
    /// Unique value definition at this state entry.
    pub result: ValueId,
    /// (transition index, incoming value); parallel edges remain distinct.
    pub inputs: Vec<(usize, ValueId)>,
}

/// An incoming machine-effect bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EffectInput {
    /// Index into the SSA transition table.
    pub transition: usize,
    /// Effect definition on that transition.
    pub effect: EffectId,
}

/// Machine state at a join, including rollback snapshots in suspended frames.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EffectPhi {
    /// Entry effect name. The root is supplied by the declared world and entry.
    pub result: EffectId,
    /// Exactly one input for each incoming transition.
    pub inputs: Vec<EffectInput>,
}

/// A basic block in a particular complete calling context.
#[derive(Clone, Debug, Serialize)]
pub struct WorldBlock {
    /// The machine-state ID, not a program-local block number.
    pub state: usize,
    /// Parameters for all stacks, including suspended callers.
    pub phis: Vec<FramePhi>,
    /// Active-frame bytecode definitions and uses.
    pub instructions: Vec<Instruction>,
    /// Stacks after the block, before call/return dispatch.
    pub exit_frames: Vec<Vec<ValueId>>,
    /// Entry memory, state and environment dependencies.
    pub effect: EffectPhi,
    /// (pc, input effect, output effect), in execution order.
    pub effects: Vec<(usize, EffectId, EffectId)>,
    /// Final effect bundle used by outgoing transitions.
    pub exit_effect: EffectId,
}

/// Call/return dispatch is an explicit operation, with its own result and
/// effect definition. A revert restores the checkpoint within its input bundle;
/// it does not forward the callee's mutated store as a successful commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Transition {
    /// Index of the corresponding WorldAnalysis edge.
    pub edge: usize,
    /// Frame transition, including commit versus rollback.
    pub kind: MachineEdgeKind,
    /// Call parameters or return/revert ranges, from the final instruction.
    pub operands: Vec<ValueId>,
    /// Stack arguments for every frame in the destination.
    pub stacks: Vec<Vec<ValueId>>,
    /// All memory/state/environment/checkpoint dependencies.
    pub effect_input: EffectId,
    /// Bundle after call entry, continuation commit or rollback.
    pub effect_result: EffectId,
    /// Deferred CALL boolean or CREATE address, defined when a caller resumes.
    pub result: Option<ValueId>,
}

/// Verified frame-aware stack and machine-effect SSA.
#[derive(Clone, Debug, Serialize)]
pub struct WorldSsa {
    blocks: Vec<WorldBlock>,
    transitions: Vec<Transition>,
    value_count: usize,
    effect_count: usize,
}

impl WorldSsa {
    /// Blocks in stable machine-state order.
    pub fn blocks(&self) -> &[WorldBlock] {
        &self.blocks
    }
    /// One transition for every discovered edge.
    pub fn transitions(&self) -> &[Transition] {
        &self.transitions
    }
    /// Number of uniquely defined stack values.
    pub fn value_count(&self) -> usize {
        self.value_count
    }
    /// Number of uniquely defined effect bundles.
    pub fn effect_count(&self) -> usize {
        self.effect_count
    }
    /// Check bytecode uses, frame identity, edge parameters, checkpoint effects
    /// and exactly-once definitions against the original analysis.
    pub fn verify(&self, analysis: &WorldAnalysis) -> Result<(), SsaError> {
        verify::verify(self, analysis)
    }
}

fn invariant(message: impl Into<String>) -> SsaError {
    SsaError::Invariant(message.into())
}

fn is_call(op: u8) -> bool {
    matches!(
        op,
        opcode::CALL
            | opcode::CALLCODE
            | opcode::DELEGATECALL
            | opcode::STATICCALL
            | opcode::CREATE
            | opcode::CREATE2
    )
}

/// Build SSA for the native multi-account graph. Missing facts, unsupported
/// semantics and resource frontiers all prevent a claim of complete SSA.
pub fn build_world(analysis: &WorldAnalysis) -> Result<WorldSsa, SsaError> {
    if analysis.status() != Status::Converged {
        return Err(SsaError::IncompleteAnalysis);
    }
    let mut next = 0;
    let mut effect_next = 0;
    let mut blocks = Vec::new();
    for state in analysis.states() {
        let mut phis = Vec::new();
        let mut stacks = Vec::new();
        for (frame, item) in state.entry.frames.iter().enumerate() {
            let mut stack = Vec::new();
            for slot in 0..item.stack.len() {
                phis.push(FramePhi {
                    frame,
                    slot,
                    result: next,
                    inputs: Vec::new(),
                });
                stack.push(next);
                next += 1;
            }
            stacks.push(stack);
        }
        let effect = EffectPhi {
            result: effect_next,
            inputs: Vec::new(),
        };
        effect_next += 1;
        let mut exit_effect = effect.result;
        let mut effects = Vec::new();
        let mut instructions = Vec::new();
        let original = state
            .program()
            .and_then(|program| program.blocks().get(state.active().block));
        let stack = stacks
            .last_mut()
            .ok_or_else(|| invariant("state has no active frame"))?;
        for source in original
            .into_iter()
            .flat_map(|block| block.instructions.iter())
            .take(state.executed_pcs.len())
        {
            let op = source.opcode;
            let (inputs, outputs) = source.stack_io();
            let fault =
                !source.is_valid() || stack.len() < inputs || stack.len() - inputs + outputs > 1024;
            let mut item = Instruction {
                pc: source.pc,
                opcode: op,
                immediate: source.immediate,
                operands: Vec::new(),
                results: Vec::new(),
                fault,
            };
            if !fault {
                if (opcode::DUP1..=opcode::DUP16).contains(&op) {
                    let value = stack[stack.len() - usize::from(op - opcode::DUP1 + 1)];
                    item.operands.push(value);
                    stack.push(value);
                } else if (opcode::SWAP1..=opcode::SWAP16).contains(&op) {
                    let top = stack.len() - 1;
                    let other = top - usize::from(op - opcode::SWAP1 + 1);
                    item.operands.extend([stack[top], stack[other]]);
                    stack.swap(top, other);
                } else {
                    item.operands = stack.drain(stack.len() - inputs..).rev().collect();
                    // The result belongs to a continuation transition, not the
                    // suspended caller or the callee's empty entry stack.
                    for _ in 0..if is_call(op) { 0 } else { outputs } {
                        item.results.push(next);
                        stack.push(next);
                        next += 1;
                    }
                }
            }
            effects.push((source.pc, exit_effect, effect_next));
            exit_effect = effect_next;
            effect_next += 1;
            instructions.push(item);
            if fault {
                break;
            }
        }
        blocks.push(WorldBlock {
            state: state.id,
            phis,
            instructions,
            exit_frames: stacks,
            effect,
            effects,
            exit_effect,
        });
    }
    let mut transitions = Vec::new();
    for (index, edge) in analysis.edges().iter().enumerate() {
        let source = &blocks[edge.from];
        let destination = &analysis.states()[edge.to];
        let mut stacks = source.exit_frames.clone();
        let mut result = None;
        match edge.kind {
            MachineEdgeKind::Intraprocedural(_) => {}
            MachineEdgeKind::Call => stacks.push(Vec::new()),
            MachineEdgeKind::Return | MachineEdgeKind::Revert | MachineEdgeKind::Failure => {
                if stacks.len() > destination.entry.frames.len() {
                    stacks.truncate(destination.entry.frames.len());
                }
                result = Some(next);
                stacks
                    .last_mut()
                    .ok_or_else(|| invariant("return has no caller"))?
                    .push(next);
                next += 1;
            }
        }
        let operands = if matches!(edge.kind, MachineEdgeKind::Intraprocedural(_)) {
            Vec::new()
        } else {
            source
                .instructions
                .last()
                .map(|i| i.operands.clone())
                .unwrap_or_default()
        };
        let transition = Transition {
            edge: index,
            kind: edge.kind,
            operands,
            stacks,
            effect_input: source.exit_effect,
            effect_result: effect_next,
            result,
        };
        effect_next += 1;
        transitions.push(transition);
    }
    for (index, edge) in analysis.edges().iter().enumerate() {
        let transition = &transitions[index];
        let block = &mut blocks[edge.to];
        for phi in &mut block.phis {
            let value = transition
                .stacks
                .get(phi.frame)
                .and_then(|s| s.get(phi.slot))
                .copied()
                .ok_or_else(|| invariant("transition stack does not match destination"))?;
            phi.inputs.push((index, value));
        }
        block.effect.inputs.push(EffectInput {
            transition: index,
            effect: transition.effect_result,
        });
    }
    let ir = WorldSsa {
        blocks,
        transitions,
        value_count: next,
        effect_count: effect_next,
    };
    ir.verify(analysis)?;
    Ok(ir)
}
