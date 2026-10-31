//! Semantic regression boundary: raw stack execution, SSA graph execution and
//! AST execution share only primitive semantics, not their control machinery.
use crate::{
    Effects, EntityId, ExitKind, Input, Ir, Stage, StructuredKind, StructuredNode, Terminator,
    U256, ValueKind, effects::effects,
};
use evm_bytecode::Program;
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};

/// Bounded concrete semantic regression execution (not gas accounting).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionLimits {
    /// Maximum attempted EVM instructions.
    pub max_steps: usize,
    /// Host memory ceiling.
    pub max_memory_bytes: usize,
}
impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_steps: 100_000,
            max_memory_bytes: 1_048_576,
        }
    }
}
/// Typed execution/refusal failures distinct from EVM exceptional halts.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionError {
    /// An IR/AST reference does not exist.
    #[error("missing semantic entity {0:?}")]
    MissingEntity(EntityId),
    /// An SSA operand was never assigned on the selected path.
    #[error("unassigned SSA operand {0:?}")]
    UnassignedValue(EntityId),
    /// The representation violates a control invariant.
    #[error("invalid structured execution: {0}")]
    InvalidStructure(String),
    /// The supplied host does not model a required primitive.
    #[error("host does not implement opcode 0x{0:02x}")]
    UnsupportedPrimitive(u8),
    /// Host output shape disagrees with the instruction definition.
    #[error("opcode 0x{opcode:02x} returned {actual} results, expected {expected}")]
    OutputArity {
        opcode: u8,
        expected: usize,
        actual: usize,
    },
    /// Concrete memory expansion refused before allocation.
    #[error("concrete host memory ceiling exceeded")]
    MemoryLimit,
    /// EVM return-data copy is out of bounds.
    #[error("return-data copy exceeds current return-data buffer")]
    ReturnDataBounds,
    /// Decode refused unsupported runtime representation.
    #[error("concrete bytecode decode failed: {0}")]
    Decode(String),
}
/// A concrete primitive request. World-dependent behavior remains host-owned.
pub struct Primitive<'a> {
    /// EVM opcode.
    pub opcode: u8,
    /// Stack operands in pop order.
    pub inputs: &'a [U256],
    /// Required word outputs.
    pub output_count: usize,
    /// Maximum permitted concrete memory allocation.
    pub memory_limit: usize,
}
/// Narrow static boundary for memory, storage, calls and environment effects.
pub trait SemanticHost {
    /// Execute one primitive, preserving its explicit observable ordering.
    fn execute(&mut self, primitive: Primitive<'_>) -> Result<Vec<U256>, ExecutionError>;
}
/// Trace step retains operation identity while semantic comparison ignores IDs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TraceStep {
    /// Stable IR operation, absent in the independent raw stack executor.
    pub operation: Option<EntityId>,
    /// Original bytecode PC, retained through actual copies.
    pub pc: Option<usize>,
    /// Executed EVM opcode.
    pub opcode: u8,
    /// Concrete pop-order operands.
    pub inputs: Vec<U256>,
    /// Concrete word results.
    pub outputs: Vec<U256>,
    /// Explicit observable effects.
    pub effects: Effects,
}
/// ID-independent semantic observation used to compare cloned representations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SemanticObservation {
    /// Original bytecode PC.
    pub pc: Option<usize>,
    /// Opcode.
    pub opcode: u8,
    /// Concrete pop-order operands.
    pub inputs: Vec<U256>,
    /// Concrete outputs.
    pub outputs: Vec<U256>,
    /// Explicit effect envelope.
    pub effects: Effects,
}
/// Concrete terminal or deliberately partial result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOutcome {
    /// Terminal EVM primitive with its original arguments.
    Exit { kind: ExitKind, values: Vec<U256> },
    /// EVM exceptional halt (not a recovery failure).
    Exceptional { reason: String },
    /// Recovery has no represented continuation for this concrete target.
    PartialJump { target: Option<U256> },
    /// Concrete instruction step ceiling was reached.
    StepLimit,
}
/// Actual ordered execution and final SSA assignments.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Execution {
    /// Terminal/partial behavior.
    pub outcome: ExecutionOutcome,
    /// Executed operations in order, including calls and memory accesses.
    pub trace: Vec<TraceStep>,
    /// Concrete attempted instruction count.
    pub steps: usize,
    /// Final SSA environment (raw executor has no SSA assignments).
    pub values: BTreeMap<EntityId, U256>,
}
impl Execution {
    /// Exact opcode/operand/result/effect order, independent of clone IDs.
    pub fn observations(&self) -> Vec<SemanticObservation> {
        self.trace
            .iter()
            .map(|t| SemanticObservation {
                pc: t.pc,
                opcode: t.opcode,
                inputs: t.inputs.clone(),
                outputs: t.outputs.clone(),
                effects: t.effects.clone(),
            })
            .collect()
    }
}

/// Execute original runtime bytes using a direct concrete stack and PC machine.
pub fn execute_bytecode(
    input: &Input,
    host: &mut impl SemanticHost,
    limits: &ExecutionLimits,
) -> Result<Execution, ExecutionError> {
    let program = Program::decode_with_fork(&input.runtime_bytecode, input.fork)
        .map_err(|e| ExecutionError::Decode(e.to_string()))?;
    let instructions: BTreeMap<_, _> = program
        .blocks()
        .iter()
        .flat_map(|b| &b.instructions)
        .map(|i| (i.pc, i))
        .collect();
    let mut pc = 0;
    let mut stack = Vec::<U256>::new();
    let mut trace = Vec::new();
    let mut steps = 0;
    let outcome = loop {
        let Some(i) = instructions.get(&pc) else {
            break ExecutionOutcome::Exit {
                kind: ExitKind::EndOfCode,
                values: Vec::new(),
            };
        };
        if steps >= limits.max_steps {
            break ExecutionOutcome::StepLimit;
        }
        steps += 1;
        if !i.is_valid() {
            break ExecutionOutcome::Exceptional {
                reason: i
                    .invalid_reason()
                    .expect("invalid opcode reason")
                    .to_string(),
            };
        }
        let code = i.opcode;
        let (inputs, outputs) = if (0x80..=0x8f).contains(&code) {
            let depth = usize::from(code - 0x7f);
            if stack.len() < depth {
                break fault("EVM stack underflow");
            }
            if stack.len() == 1024 {
                break fault("EVM stack overflow");
            }
            let value = stack[stack.len() - depth];
            stack.push(value);
            (vec![value], Vec::new())
        } else if (0x90..=0x9f).contains(&code) {
            let depth = usize::from(code - 0x8f);
            if stack.len() <= depth {
                break fault("EVM stack underflow");
            }
            let top = stack.len() - 1;
            let other = top - depth;
            let inputs = vec![stack[top], stack[other]];
            stack.swap(top, other);
            (inputs, Vec::new())
        } else {
            let (needed, count) = i.stack_io();
            if stack.len() < needed {
                break fault("EVM stack underflow");
            }
            if stack.len() - needed + count > 1024 {
                break fault("EVM stack overflow");
            }
            let inputs: Vec<_> = (0..needed)
                .map(|_| stack.pop().expect("checked stack"))
                .collect();
            let outputs = primitive(
                code,
                &inputs,
                count,
                i.immediate,
                i.pc,
                program.byte_len(),
                None,
                host,
                limits,
            )?;
            stack.extend(outputs.iter().copied());
            (inputs, outputs)
        };
        trace.push(TraceStep {
            operation: None,
            pc: Some(pc),
            opcode: code,
            inputs: inputs.clone(),
            outputs,
            effects: effects(code),
        });
        if matches!(code, 0x00 | 0xf3 | 0xfd | 0xff) {
            break ExecutionOutcome::Exit {
                kind: match code {
                    0x00 => ExitKind::Stop,
                    0xf3 => ExitKind::Return,
                    0xfd => ExitKind::Revert,
                    _ => ExitKind::SelfDestruct,
                },
                values: inputs,
            };
        }
        if code == 0x56 || (code == 0x57 && inputs[1] != U256::ZERO) {
            let Some(target) = usize::try_from(inputs[0])
                .ok()
                .filter(|p| program.jumpdest_blocks().contains_key(p))
            else {
                break fault("invalid jump destination");
            };
            pc = target;
        } else {
            pc = i.pc.saturating_add(i.size);
        }
    };
    Ok(Execution {
        outcome,
        trace,
        steps,
        values: BTreeMap::new(),
    })
}
fn fault(reason: &str) -> ExecutionOutcome {
    ExecutionOutcome::Exceptional {
        reason: reason.to_owned(),
    }
}

/// Execute the SSA graph with simultaneous phi assignment at every edge.
pub fn execute_ir(
    stage: &Stage,
    host: &mut impl SemanticHost,
    limits: &ExecutionLimits,
) -> Result<Execution, ExecutionError> {
    let mut runtime = Runtime::new(&stage.ir, host, limits);
    let outcome = if let Some(mut current) = stage.ir.entry {
        loop {
            if let Some(out) = runtime.block(current)? {
                break out;
            }
            match runtime.choose(current)? {
                Choice::Edge(edge) => {
                    runtime.transfer(edge)?;
                    current = stage
                        .ir
                        .edge(edge)
                        .ok_or(ExecutionError::MissingEntity(edge))?
                        .to;
                }
                Choice::End(out) => break out,
            }
        }
    } else {
        ExecutionOutcome::Exit {
            kind: ExitKind::EndOfCode,
            values: Vec::new(),
        }
    };
    Ok(runtime.finish(outcome))
}
/// Execute actual structured syntax, independently of graph traversal scheduling.
pub fn execute_structured(
    stage: &Stage,
    host: &mut impl SemanticHost,
    limits: &ExecutionLimits,
) -> Result<Execution, ExecutionError> {
    let root = &stage
        .ir
        .structured
        .as_ref()
        .ok_or_else(|| ExecutionError::InvalidStructure("stage has no structured AST".to_owned()))?
        .root;
    let mut runtime = Runtime::new(&stage.ir, host, limits);
    let flow = runtime.syntax(root)?;
    let outcome = match flow {
        Flow::End(out) => out,
        Flow::Normal if stage.ir.entry.is_none() => ExecutionOutcome::Exit {
            kind: ExitKind::EndOfCode,
            values: Vec::new(),
        },
        _ => {
            return Err(ExecutionError::InvalidStructure(
                "AST completed without an explicit terminal result".to_owned(),
            ));
        }
    };
    Ok(runtime.finish(outcome))
}
enum Choice {
    Edge(EntityId),
    End(ExecutionOutcome),
}
enum Flow {
    Normal,
    Break,
    Continue,
    Escaped(EntityId),
    End(ExecutionOutcome),
}
struct Runtime<'a, H> {
    ir: &'a Ir,
    host: &'a mut H,
    limits: &'a ExecutionLimits,
    values: BTreeMap<EntityId, U256>,
    trace: Vec<TraceStep>,
    steps: usize,
    current: Option<EntityId>,
}
impl<'a, H: SemanticHost> Runtime<'a, H> {
    fn new(ir: &'a Ir, host: &'a mut H, limits: &'a ExecutionLimits) -> Self {
        Self {
            ir,
            host,
            limits,
            values: BTreeMap::new(),
            trace: Vec::new(),
            steps: 0,
            current: ir.entry,
        }
    }
    fn finish(self, outcome: ExecutionOutcome) -> Execution {
        Execution {
            outcome,
            trace: self.trace,
            steps: self.steps,
            values: self.values,
        }
    }
    fn value(&self, id: EntityId) -> Result<U256, ExecutionError> {
        self.values
            .get(&id)
            .copied()
            .ok_or(ExecutionError::UnassignedValue(id))
    }
    fn block(&mut self, id: EntityId) -> Result<Option<ExecutionOutcome>, ExecutionError> {
        if self.current != Some(id) {
            return Err(ExecutionError::InvalidStructure(format!(
                "block {id:?} executed at {:?}",
                self.current
            )));
        }
        let block = self.ir.block(id).ok_or(ExecutionError::MissingEntity(id))?;
        if matches!(block.terminator, Terminator::Pending) {
            return Ok(Some(ExecutionOutcome::PartialJump { target: None }));
        }
        for (position, op) in block.operations.iter().enumerate() {
            if self.steps >= self.limits.max_steps {
                return Ok(Some(ExecutionOutcome::StepLimit));
            }
            self.steps += 1;
            // Invalid/underflow/overflow instructions fault before effects. The
            // IR retains their identity, but does not invent missing operands.
            if position + 1 == block.operations.len()
                && matches!(block.terminator, Terminator::Trap { .. })
            {
                break;
            }
            let inputs: Vec<_> = op
                .inputs
                .iter()
                .map(|v| self.value(*v))
                .collect::<Result<_, _>>()?;
            let immediate = op
                .outputs
                .first()
                .and_then(|id| self.ir.value(*id))
                .and_then(|v| match v.kind {
                    ValueKind::Constant(c) => Some(c),
                    _ => None,
                });
            let outputs = if (0x80..=0x9f).contains(&op.opcode) {
                Vec::new()
            } else {
                primitive(
                    op.opcode,
                    &inputs,
                    op.outputs.len(),
                    immediate,
                    op.source.map(|s| s.pc).unwrap_or(0),
                    self.byte_len(),
                    op.replacement,
                    self.host,
                    self.limits,
                )?
            };
            for (id, value) in op.outputs.iter().zip(&outputs) {
                self.values.insert(*id, *value);
            }
            self.trace.push(TraceStep {
                operation: Some(op.id),
                pc: op.source.map(|s| s.pc),
                opcode: op.opcode,
                inputs,
                outputs,
                effects: op.effects.clone(),
            });
        }
        Ok(None)
    }
    fn byte_len(&self) -> usize {
        self.ir.bytecode_len
    }
    fn transfer(&mut self, id: EntityId) -> Result<(), ExecutionError> {
        let edge = self.ir.edge(id).ok_or(ExecutionError::MissingEntity(id))?;
        if self.current != Some(edge.from) {
            return Err(ExecutionError::InvalidStructure(
                "edge source differs from current block".to_owned(),
            ));
        }
        let target = self
            .ir
            .block(edge.to)
            .ok_or(ExecutionError::MissingEntity(edge.to))?;
        if target.parameters.len() != edge.arguments.len() {
            return Err(ExecutionError::InvalidStructure(
                "phi assignment arity mismatch".to_owned(),
            ));
        }
        // Evaluate every RHS before updating any parameter (loop swaps matter).
        let args: Vec<_> = edge
            .arguments
            .iter()
            .map(|v| self.value(*v))
            .collect::<Result<_, _>>()?;
        for (id, value) in target.parameters.iter().zip(args) {
            self.values.insert(*id, value);
        }
        self.current = Some(edge.to);
        Ok(())
    }
    fn choose(&self, id: EntityId) -> Result<Choice, ExecutionError> {
        let block = self.ir.block(id).ok_or(ExecutionError::MissingEntity(id))?;
        let choice = match &block.terminator {
            Terminator::Pending => Choice::End(ExecutionOutcome::PartialJump { target: None }),
            Terminator::Trap { reason } => Choice::End(fault(reason)),
            Terminator::Exit { kind, values } => Choice::End(ExecutionOutcome::Exit {
                kind: *kind,
                values: values
                    .iter()
                    .map(|v| self.value(*v))
                    .collect::<Result<_, _>>()?,
            }),
            Terminator::Fallthrough { edge } => Choice::Edge(*edge),
            Terminator::Jump {
                target,
                edges,
                unresolved,
                ..
            } => self.target(*target, edges, *unresolved)?,
            Terminator::Branch {
                condition,
                target,
                then_edges,
                else_edge,
                unresolved,
                ..
            } => {
                if self.value(*condition)? == U256::ZERO {
                    else_edge
                        .map(Choice::Edge)
                        .unwrap_or(Choice::End(ExecutionOutcome::Exit {
                            kind: ExitKind::EndOfCode,
                            values: Vec::new(),
                        }))
                } else {
                    self.target(*target, then_edges, *unresolved)?
                }
            }
        };
        Ok(choice)
    }
    fn target(
        &self,
        target: EntityId,
        edges: &[EntityId],
        unresolved: bool,
    ) -> Result<Choice, ExecutionError> {
        let value = self.value(target)?;
        for id in edges {
            let edge = self
                .ir
                .edge(*id)
                .ok_or(ExecutionError::MissingEntity(*id))?;
            if self
                .ir
                .block(edge.to)
                .and_then(|b| b.source)
                .is_some_and(|s| U256::from(s.pc) == value)
            {
                return Ok(Choice::Edge(*id));
            }
        }
        Ok(Choice::End(if unresolved {
            ExecutionOutcome::PartialJump {
                target: Some(value),
            }
        } else {
            fault("invalid jump destination")
        }))
    }
    fn syntax(&mut self, node: &StructuredNode) -> Result<Flow, ExecutionError> {
        match &node.kind {
            StructuredKind::Sequence(nodes) => {
                let mut escaped = None;
                for node in nodes {
                    if let Some(target) = escaped.take() {
                        if first_block(node, self.ir) != Some(target) {
                            return Ok(Flow::Escaped(target));
                        }
                    }
                    match self.syntax(node)? {
                        Flow::Normal => {}
                        Flow::Escaped(target) => escaped = Some(target),
                        other => return Ok(other),
                    }
                }
                Ok(escaped.map(Flow::Escaped).unwrap_or(Flow::Normal))
            }
            StructuredKind::Block { block } => {
                Ok(self.block(*block)?.map(Flow::End).unwrap_or(Flow::Normal))
            }
            StructuredKind::Transfer { edge } => {
                self.transfer(*edge)?;
                Ok(Flow::Normal)
            }
            StructuredKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                if self.value(*condition)? != U256::ZERO {
                    self.syntax(then_branch)
                } else {
                    self.syntax(else_branch)
                }
            }
            StructuredKind::While {
                header,
                condition,
                condition_nonzero,
                body,
                exit_edge,
            } => {
                let entry = first_block(header, self.ir).or_else(|| first_block(body, self.ir));
                loop {
                    match self.syntax(header)? {
                        Flow::Normal => {}
                        other => return Ok(other),
                    }
                    if let Some(id) = condition {
                        if (self.value(*id)? != U256::ZERO) != *condition_nonzero {
                            if let Some(edge) = exit_edge {
                                self.transfer(*edge)?;
                            }
                            return Ok(Flow::Normal);
                        }
                    }
                    match self.syntax(body)? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => return Ok(Flow::Normal),
                        Flow::Escaped(target) if Some(target) == entry => {}
                        Flow::Escaped(_) => return Ok(Flow::Normal),
                        other => return Ok(other),
                    }
                }
            }
            StructuredKind::Break => Ok(Flow::Break),
            StructuredKind::Continue => Ok(Flow::Continue),
            StructuredKind::Exit { block } | StructuredKind::PartialExit { block } => {
                match self.choose(*block)? {
                    Choice::End(out) => Ok(Flow::End(out)),
                    Choice::Edge(_) => Err(ExecutionError::InvalidStructure(
                        "terminal AST node has a known continuation".to_owned(),
                    )),
                }
            }
            StructuredKind::Indirect { block, arms, .. } => match self.choose(*block)? {
                Choice::End(out) => Ok(Flow::End(out)),
                Choice::Edge(edge) => {
                    let target = self
                        .ir
                        .edge(edge)
                        .and_then(|e| self.ir.block(e.to))
                        .and_then(|b| b.source)
                        .ok_or(ExecutionError::MissingEntity(edge))?
                        .pc;
                    let arm = arms.iter().find(|a| a.target_pc == target).ok_or_else(|| {
                        ExecutionError::InvalidStructure(
                            "known indirect target arm missing".to_owned(),
                        )
                    })?;
                    self.syntax(&arm.body)
                }
            },
            StructuredKind::Dispatcher { entry, cases } => {
                let mut state = *entry;
                loop {
                    let case = cases.iter().find(|c| c.block == state).ok_or_else(|| {
                        ExecutionError::InvalidStructure("dispatcher state missing".to_owned())
                    })?;
                    if let Some(out) = self.block(case.block)? {
                        return Ok(Flow::End(out));
                    }
                    match self.choose(case.block)? {
                        Choice::End(out) => return Ok(Flow::End(out)),
                        Choice::Edge(edge) => {
                            let transition = case
                                .transitions
                                .iter()
                                .find(|t| t.edge == edge)
                                .ok_or_else(|| {
                                    ExecutionError::InvalidStructure(
                                        "dispatcher transition missing".to_owned(),
                                    )
                                })?;
                            self.transfer(transition.edge)?;
                            state = transition.target;
                            if !cases.iter().any(|c| c.block == state) {
                                return Ok(Flow::Escaped(state));
                            }
                        }
                    }
                }
            }
        }
    }
}
fn first_block(node: &StructuredNode, ir: &Ir) -> Option<EntityId> {
    match &node.kind {
        StructuredKind::Block { block }
        | StructuredKind::Exit { block }
        | StructuredKind::PartialExit { block }
        | StructuredKind::Indirect { block, .. } => Some(*block),
        StructuredKind::Sequence(nodes) => nodes.iter().find_map(|n| first_block(n, ir)),
        StructuredKind::While { header, body, .. } => {
            first_block(header, ir).or_else(|| first_block(body, ir))
        }
        StructuredKind::Dispatcher { entry, .. } => Some(*entry),
        StructuredKind::Transfer { edge } => ir.edge(*edge).map(|e| e.from),
        _ => None,
    }
}
#[allow(clippy::too_many_arguments)]
fn primitive(
    op: u8,
    args: &[U256],
    count: usize,
    immediate: Option<U256>,
    pc: usize,
    byte_len: usize,
    replacement: Option<U256>,
    host: &mut impl SemanticHost,
    limits: &ExecutionLimits,
) -> Result<Vec<U256>, ExecutionError> {
    let outputs = if let Some(word) = replacement {
        vec![word]
    } else if (0x5f..=0x7f).contains(&op) {
        vec![immediate.unwrap_or(U256::ZERO)]
    } else if op == 0x58 {
        vec![U256::from(pc)]
    } else if op == 0x38 {
        vec![U256::from(byte_len)]
    } else if evm_bytecode::word::arity(op).is_some() {
        vec![
            evm_bytecode::word::evaluate(op, args)
                .map_err(|e| ExecutionError::InvalidStructure(e.to_string()))?,
        ]
    } else if matches!(op, 0x50 | 0x56 | 0x57 | 0x5b) {
        Vec::new()
    } else {
        host.execute(Primitive {
            opcode: op,
            inputs: args,
            output_count: count,
            memory_limit: limits.max_memory_bytes,
        })?
    };
    if outputs.len() != count {
        return Err(ExecutionError::OutputArity {
            opcode: op,
            expected: count,
            actual: outputs.len(),
        });
    }
    Ok(outputs)
}

/// One deterministic external operation outcome for semantic fixtures.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExternalResponse {
    /// CALL success word or CREATE address.
    pub result: U256,
    /// Buffer replacing RETURNDATA (also copied to CALL's output range).
    pub return_data: Vec<u8>,
    /// Host-visible writes performed by the external execution.
    pub storage_writes: BTreeMap<U256, U256>,
}
/// Inspectable concrete host for fixtures. Application hosts may implement their
/// own world primitives; recovery itself never constructs this concrete state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryHost {
    /// Runtime bytes used by CODECOPY.
    pub code: Vec<u8>,
    /// Immutable call input.
    pub calldata: Vec<u8>,
    /// Concrete zero-initialized expanded local memory.
    pub memory: Vec<u8>,
    /// Persistent storage snapshot.
    pub storage: BTreeMap<U256, U256>,
    /// Transaction-local transient storage.
    pub transient: BTreeMap<U256, U256>,
    /// Current return-data buffer.
    pub return_data: Vec<u8>,
    /// Explicit environment observations by opcode.
    pub context: BTreeMap<u8, U256>,
    /// FIFO external call/create responses.
    pub responses: VecDeque<ExternalResponse>,
    /// Emitted log opcode/arguments/memory bytes.
    pub logs: Vec<(u8, Vec<U256>, Vec<u8>)>,
}
impl MemoryHost {
    /// Initial zero memory plus supplied runtime and calldata.
    pub fn new(input: &Input, calldata: Vec<u8>) -> Self {
        Self {
            code: input.runtime_bytecode.clone(),
            calldata,
            ..Self::default()
        }
    }
    fn range(
        &mut self,
        offset: U256,
        length: U256,
        limit: usize,
    ) -> Result<(usize, usize), ExecutionError> {
        let length = usize::try_from(length).map_err(|_| ExecutionError::MemoryLimit)?;
        if length == 0 {
            return Ok((0, 0));
        }
        let offset = usize::try_from(offset).map_err(|_| ExecutionError::MemoryLimit)?;
        let end = offset
            .checked_add(length)
            .ok_or(ExecutionError::MemoryLimit)?;
        let rounded = end.checked_add(31).ok_or(ExecutionError::MemoryLimit)? / 32 * 32;
        if rounded > limit {
            return Err(ExecutionError::MemoryLimit);
        }
        if self.memory.len() < rounded {
            self.memory.resize(rounded, 0);
        }
        Ok((offset, length))
    }
}
impl SemanticHost for MemoryHost {
    fn execute(&mut self, p: Primitive<'_>) -> Result<Vec<U256>, ExecutionError> {
        let a = p.inputs;
        let mut out = Vec::new();
        match p.opcode {
            0x00 | 0xff => {}
            0x20 => {
                let (off, len) = self.range(a[0], a[1], p.memory_limit)?;
                out.push(U256::from_be_bytes(
                    alloy_primitives::keccak256(&self.memory[off..off + len]).0,
                ));
            }
            0x35 => {
                let offset = usize::try_from(a[0]).ok();
                let mut bytes = [0; 32];
                for (i, b) in bytes.iter_mut().enumerate() {
                    *b = offset
                        .and_then(|o| o.checked_add(i))
                        .and_then(|o| self.calldata.get(o))
                        .copied()
                        .unwrap_or(0);
                }
                out.push(U256::from_be_bytes(bytes));
            }
            0x36 => out.push(U256::from(self.calldata.len())),
            0x3d => out.push(U256::from(self.return_data.len())),
            0x37 | 0x39 | 0x3e => {
                let (off, len) = self.range(a[0], a[2], p.memory_limit)?;
                let source = usize::try_from(a[1]).ok();
                let data = match p.opcode {
                    0x37 => &self.calldata,
                    0x39 => &self.code,
                    _ => &self.return_data,
                };
                if p.opcode == 0x3e
                    && source
                        .and_then(|s| s.checked_add(len))
                        .is_none_or(|end| end > data.len())
                {
                    return Err(ExecutionError::ReturnDataBounds);
                }
                for i in 0..len {
                    self.memory[off + i] = source
                        .and_then(|s| s.checked_add(i))
                        .and_then(|s| data.get(s))
                        .copied()
                        .unwrap_or(0);
                }
            }
            0x51 => {
                let (off, _) = self.range(a[0], U256::from(32), p.memory_limit)?;
                let mut bytes = [0; 32];
                bytes.copy_from_slice(&self.memory[off..off + 32]);
                out.push(U256::from_be_bytes(bytes));
            }
            0x52 => {
                let (off, _) = self.range(a[0], U256::from(32), p.memory_limit)?;
                self.memory[off..off + 32].copy_from_slice(&a[1].to_be_bytes::<32>());
            }
            0x53 => {
                let (off, _) = self.range(a[0], U256::from(1), p.memory_limit)?;
                self.memory[off] = a[1].to_be_bytes::<32>()[31];
            }
            0x54 => out.push(self.storage.get(&a[0]).copied().unwrap_or(U256::ZERO)),
            0x55 => {
                self.storage.insert(a[0], a[1]);
            }
            0x5c => out.push(self.transient.get(&a[0]).copied().unwrap_or(U256::ZERO)),
            0x5d => {
                self.transient.insert(a[0], a[1]);
            }
            0x59 => out.push(U256::from(self.memory.len())),
            0x5e => {
                let (dst, len) = self.range(a[0], a[2], p.memory_limit)?;
                let (src, _) = self.range(a[1], a[2], p.memory_limit)?;
                self.memory.copy_within(src..src + len, dst);
            }
            0xa0..=0xa4 => {
                let (off, len) = self.range(a[0], a[1], p.memory_limit)?;
                self.logs
                    .push((p.opcode, a.to_vec(), self.memory[off..off + len].to_vec()));
            }
            0xf1 | 0xf2 | 0xf4 | 0xfa => {
                let base = if matches!(p.opcode, 0xf1 | 0xf2) {
                    3
                } else {
                    2
                };
                self.range(a[base], a[base + 1], p.memory_limit)?;
                let (off, len) = self.range(a[base + 2], a[base + 3], p.memory_limit)?;
                let response = self
                    .responses
                    .pop_front()
                    .ok_or(ExecutionError::UnsupportedPrimitive(p.opcode))?;
                self.return_data = response.return_data;
                let copied = len.min(self.return_data.len());
                self.memory[off..off + copied].copy_from_slice(&self.return_data[..copied]);
                if p.opcode != 0xfa {
                    self.storage.extend(response.storage_writes);
                }
                out.push(response.result);
            }
            0xf0 | 0xf5 => {
                self.range(a[1], a[2], p.memory_limit)?;
                let response = self
                    .responses
                    .pop_front()
                    .ok_or(ExecutionError::UnsupportedPrimitive(p.opcode))?;
                self.return_data = response.return_data;
                self.storage.extend(response.storage_writes);
                out.push(response.result);
            }
            0xf3 | 0xfd => {
                self.range(a[0], a[1], p.memory_limit)?;
            }
            0x30..=0x34 | 0x3a | 0x3b | 0x3f..=0x4a | 0x5a => {
                out.push(self.context.get(&p.opcode).copied().unwrap_or(U256::ZERO))
            }
            _ => return Err(ExecutionError::UnsupportedPrimitive(p.opcode)),
        }
        Ok(out)
    }
}
