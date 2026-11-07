//! An independent display interpreter: execute the explicit failure-first plan
//! with original constructor primitives, and build/check ABI bytes separately.
//! No producer search or production semantic certificate checker is an oracle.
use evm_decompile::{
    Artifact, EntityId, ExecutionError, ExecutionLimits, ExecutionOutcome, ExitKind, Input,
    LiftedProgram, LiftedRenderIndex, LiftedValue, MemoryHost, Primitive, SemanticHost, Stage,
    StageKind, StructuredKind, StructuredNode, Terminator, U256, execute_bytecode, execute_ir,
    execute_structured,
};
use std::collections::BTreeMap;

pub(super) fn abi(message: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0_u8; 68 + message.len().div_ceil(32) * 32];
    bytes[..4].copy_from_slice(&[8, 0xc3, 0x79, 0xa0]);
    bytes[35] = 32;
    bytes[36..68].copy_from_slice(&U256::from(message.len()).to_be_bytes::<32>());
    bytes[68..68 + message.len()].copy_from_slice(message);
    bytes
}

pub(super) fn reverted(outcome: &ExecutionOutcome, host: &MemoryHost) -> Option<Vec<u8>> {
    let ExecutionOutcome::Exit {
        kind: ExitKind::Revert,
        values,
    } = outcome
    else {
        return None;
    };
    let size = usize::try_from(values[1]).unwrap();
    if size == 0 {
        return Some(Vec::new());
    }
    let offset = usize::try_from(values[0]).unwrap();
    Some(host.memory[offset..offset + size].to_vec())
}

#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Next,
    Break,
    Continue,
    End(ExecutionOutcome),
}

type Observation = (Option<usize>, u8, Vec<U256>, Vec<U256>);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Invocation {
    opcode: u8,
    inputs: Vec<U256>,
    output_count: usize,
    memory_limit: usize,
    result: Result<Vec<U256>, ExecutionError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordingHost {
    state: MemoryHost,
    invocations: Vec<Invocation>,
}

impl RecordingHost {
    fn new(state: MemoryHost) -> Self {
        Self {
            state,
            invocations: Vec::new(),
        }
    }
}

impl SemanticHost for RecordingHost {
    fn execute(&mut self, primitive: Primitive<'_>) -> Result<Vec<U256>, ExecutionError> {
        let mut invocation = Invocation {
            opcode: primitive.opcode,
            inputs: primitive.inputs.to_vec(),
            output_count: primitive.output_count,
            memory_limit: primitive.memory_limit,
            result: Ok(Vec::new()),
        };
        let result = self.state.execute(primitive);
        // Primitive does not carry a PC. The complete request/result prefix,
        // including its final Err, is retained even when execution has no trace.
        invocation.result = result.clone();
        self.invocations.push(invocation);
        result
    }
}

struct Machine<'a> {
    stage: &'a Stage,
    program: &'a LiftedProgram,
    index: LiftedRenderIndex<'a>,
    host: RecordingHost,
    values: BTreeMap<EntityId, U256>,
    current: EntityId,
    observations: Vec<Observation>,
    writes: Vec<(EntityId, U256, Vec<u8>)>,
    memory_limit: usize,
    capsules: usize,
    steps: usize,
}

impl Machine<'_> {
    fn word(&self, id: EntityId) -> U256 {
        let LiftedValue::Word(value) =
            crate::oracle::evaluate(self.program, id, self.current, &self.values)
        else {
            panic!("normalized word input has a predicate sort")
        };
        value
    }

    fn predicate(&self, id: EntityId) -> bool {
        let LiftedValue::Predicate(value) =
            crate::oracle::evaluate(self.program, id, self.current, &self.values)
        else {
            panic!("normalized condition has a word sort")
        };
        value
    }

    fn block(&mut self, id: EntityId) -> Result<(), ExecutionError> {
        assert_eq!(
            self.current, id,
            "source block executed without its transfer"
        );
        self.steps += 1;
        assert!(self.steps <= 1_000, "bounded normalized fixture execution");
        let high = self.program.block(id).unwrap();
        let raw = self.stage.ir.block(id).unwrap();
        let construction = self.index.construction(id);
        if let Some(plan) = construction {
            assert!(plan.start < plan.end && plan.end <= high.statements.len());
            assert_eq!(
                plan.end,
                high.statements.len(),
                "capsule must own its exact terminal suffix"
            );
            let terminal = self.index.revert(id).unwrap();
            assert_eq!(plan.revert, terminal.id);
            assert_eq!(terminal.operation, high.statements[plan.end - 1].operation);
        }
        for (position, statement) in high.statements.iter().enumerate() {
            if construction.is_some_and(|plan| plan.start == position) {
                self.capsules += 1;
            }
            // The counter records source certificate coverage. The explicit
            // rendering plan executes every original statement in this interval,
            // preserving allocation, side effects, ordering and first fault.
            let operation = raw
                .operations
                .iter()
                .find(|op| op.id == statement.operation)
                .unwrap();
            let inputs: Vec<_> = statement.inputs.iter().map(|id| self.word(*id)).collect();
            let outputs = if evm_bytecode::word::arity(operation.opcode).is_some() {
                vec![evm_bytecode::word::evaluate(operation.opcode, &inputs).unwrap()]
            } else {
                self.host.execute(Primitive {
                    opcode: operation.opcode,
                    inputs: &inputs,
                    output_count: operation.outputs.len(),
                    memory_limit: self.memory_limit,
                })?
            };
            assert_eq!(outputs.len(), operation.outputs.len());
            if matches!(operation.opcode, 0x52 | 0x53) {
                let bytes = if operation.opcode == 0x52 {
                    inputs[1].to_be_bytes::<32>().to_vec()
                } else {
                    vec![inputs[1].to_be_bytes::<32>()[31]]
                };
                self.writes.push((operation.id, inputs[0], bytes));
            }
            if !operation.effects.is_pure() {
                self.observations.push((
                    operation.source.map(|s| s.pc),
                    operation.opcode,
                    inputs,
                    outputs.clone(),
                ));
            }
            for output in &statement.outputs {
                let position = operation
                    .outputs
                    .iter()
                    .position(|id| id == output)
                    .unwrap();
                self.values.insert(*output, outputs[position]);
            }
        }
        Ok(())
    }

    fn transfer(&mut self, id: EntityId) {
        let edge = self.stage.ir.edge(id).unwrap();
        assert_eq!(edge.from, self.current);
        let high = self.program.transfer(id).unwrap();
        let arguments: Vec<_> = high.arguments.iter().map(|id| self.word(*id)).collect();
        let destination = self.program.block(edge.to).unwrap();
        assert_eq!(arguments.len(), destination.parameters.len());
        for (id, value) in destination.parameters.iter().zip(arguments) {
            self.values.insert(*id, value);
        }
        self.current = edge.to;
    }

    fn exit(&self, id: EntityId) -> ExecutionOutcome {
        match &self.stage.ir.block(id).unwrap().terminator {
            Terminator::Exit { kind, .. } => {
                let values: Vec<_> = self
                    .program
                    .block(id)
                    .unwrap()
                    .exit_values
                    .iter()
                    .map(|id| self.word(*id))
                    .collect();
                if let Some(proof) = self.index.revert(id) {
                    assert_eq!(*kind, ExitKind::Revert);
                    let operation = self.stage.ir.block(id).unwrap().operations.last().unwrap();
                    assert_eq!(
                        (proof.operation, proof.offset, proof.size),
                        (operation.id, operation.inputs[0], operation.inputs[1])
                    );
                    assert_eq!(operation.memory_input, Some(proof.memory));
                    let payload = self.index.payload(proof.payload).unwrap();
                    assert_eq!(payload.bytes, abi(payload.message.as_bytes()));
                    let offset = usize::try_from(values[0]).unwrap();
                    let size = usize::try_from(values[1]).unwrap();
                    let end = offset.checked_add(size).unwrap();
                    let mut certified = vec![None; size];
                    let mut previous = None;
                    for write in &proof.writes {
                        let (position, (_, address, bytes)) = self
                            .writes
                            .iter()
                            .enumerate()
                            .rfind(|(_, (id, _, _))| id == write)
                            .expect("proof writer never executed");
                        assert!(
                            previous.is_none_or(|prior| prior < position),
                            "wrong certificate write order"
                        );
                        previous = Some(position);
                        let start = usize::try_from(*address).unwrap();
                        let write_end = start.checked_add(bytes.len()).unwrap();
                        let intersection = offset.max(start)..end.min(write_end);
                        assert!(
                            !intersection.is_empty(),
                            "certificate lists an unrelated write"
                        );
                        for address in intersection {
                            certified[address - offset] = Some(bytes[address - start]);
                        }
                    }
                    assert_eq!(
                        certified.into_iter().collect::<Option<Vec<_>>>(),
                        Some(payload.bytes.clone()),
                        "independent last-write-wins witness replay did not establish every payload byte"
                    );
                    let outcome = ExecutionOutcome::Exit {
                        kind: *kind,
                        values: values.clone(),
                    };
                    assert_eq!(
                        reverted(&outcome, &self.host.state).unwrap(),
                        payload.bytes,
                        "terminal proof disagrees with independently executed actual memory"
                    );
                }
                ExecutionOutcome::Exit {
                    kind: *kind,
                    values,
                }
            }
            Terminator::Trap { reason } => ExecutionOutcome::Exceptional {
                reason: reason.clone(),
            },
            Terminator::Pending => ExecutionOutcome::PartialJump { target: None },
            other => panic!("fixture has a nonterminal exit {other:?}"),
        }
    }

    // Independently execute the exact source occurrences selected by the
    // rendering plan. Payload certificates never replace their constructors.
    fn sequence(
        &mut self,
        sequence: EntityId,
        children: &[StructuredNode],
        range: std::ops::Range<usize>,
    ) -> Result<Flow, ExecutionError> {
        let boundary = self.stage.ir.structured.as_ref().unwrap().root.id;
        let mut position = range.start;
        while position < range.end {
            if position > range.start
                && let Some(end) = self
                    .index
                    .failure_first_sequence_end(sequence, position, boundary)
            {
                assert!(end > position && end <= range.end);
                position = end;
                continue;
            }
            let flow = self.syntax(&children[position])?;
            if flow != Flow::Next {
                return Ok(flow);
            }
            position += 1;
        }
        Ok(Flow::Next)
    }

    fn syntax(&mut self, node: &StructuredNode) -> Result<Flow, ExecutionError> {
        match &node.kind {
            StructuredKind::Sequence(children) => {
                self.sequence(node.id, children, 0..children.len())
            }
            StructuredKind::Block { block } => {
                self.block(*block)?;
                Ok(Flow::Next)
            }
            StructuredKind::Transfer { edge } => {
                self.transfer(*edge);
                Ok(Flow::Next)
            }
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                let control = self.program.control(node.id).unwrap();
                let condition = self.predicate(control.condition);
                let boundary = self.stage.ir.structured.as_ref().unwrap().root.id;
                if let Some(plan) = self.index.failure_first(node.id, boundary).cloned() {
                    if condition == plan.failure_nonzero() {
                        let flow = self.syntax(self.index.node(plan.failure_node()).unwrap())?;
                        if flow != Flow::Next {
                            return Ok(flow);
                        }
                        if let Some((sequence, range)) =
                            self.index.failure_first_suffix_range(&plan)
                        {
                            let StructuredKind::Sequence(children) =
                                &self.index.node(sequence).unwrap().kind
                            else {
                                unreachable!()
                            };
                            let flow = self.sequence(sequence, children, range)?;
                            if flow != Flow::Next {
                                return Ok(flow);
                            }
                        }
                        panic!("admitted explicit failure fell into its success continuation");
                    }
                    self.syntax(self.index.node(plan.success_node()).unwrap())
                } else {
                    self.syntax(if condition { then_branch } else { else_branch })
                }
            }
            StructuredKind::While {
                header,
                condition,
                condition_nonzero,
                body,
                exit_edge,
            } => {
                for _ in 0..1_000 {
                    let flow = self.syntax(header)?;
                    if flow != Flow::Next {
                        return Ok(flow);
                    }
                    if condition.is_some()
                        && self.predicate(self.program.control(node.id).unwrap().condition)
                            != *condition_nonzero
                    {
                        if let Some(edge) = exit_edge {
                            self.transfer(*edge)
                        }
                        return Ok(Flow::Next);
                    }
                    match self.syntax(body)? {
                        Flow::Next | Flow::Continue => {}
                        Flow::Break => return Ok(Flow::Next),
                        flow => return Ok(flow),
                    }
                }
                panic!("normalized fixture loop ceiling")
            }
            StructuredKind::Break => Ok(Flow::Break),
            StructuredKind::Continue => Ok(Flow::Continue),
            StructuredKind::Dispatcher { entry, cases } => {
                assert_eq!(self.current, *entry);
                loop {
                    let case = cases
                        .iter()
                        .find(|case| case.block == self.current)
                        .expect("normalized dispatcher selected an unowned case");
                    self.block(case.block)?;
                    let edge = match &self.stage.ir.block(case.block).unwrap().terminator {
                        Terminator::Exit { .. } | Terminator::Trap { .. } => {
                            return Ok(Flow::End(self.exit(case.block)));
                        }
                        Terminator::Fallthrough { edge } => *edge,
                        Terminator::Jump {
                            edges,
                            unresolved: false,
                            ..
                        } if edges.len() == 1 => edges[0],
                        Terminator::Branch {
                            then_edges,
                            else_edge: Some(other),
                            unresolved: false,
                            ..
                        } if then_edges.len() == 1 => {
                            let condition =
                                self.program.block(case.block).unwrap().condition.unwrap();
                            if self.predicate(condition) {
                                then_edges[0]
                            } else {
                                *other
                            }
                        }
                        other => panic!(
                            "fixture needs explicit dispatcher transfer semantics for {other:?}"
                        ),
                    };
                    let transition = case
                        .transitions
                        .iter()
                        .find(|t| t.edge == edge)
                        .expect("dispatcher omitted the chosen source transfer");
                    assert_eq!(transition.target, self.stage.ir.edge(edge).unwrap().to);
                    self.transfer(edge);
                    if !cases.iter().any(|case| case.block == self.current) {
                        return Ok(Flow::Next);
                    }
                }
            }
            StructuredKind::Exit { block } | StructuredKind::PartialExit { block } => {
                Ok(Flow::End(self.exit(*block)))
            }
            other => panic!("fixture needs explicit normalized semantics for {other:?}"),
        }
    }
}

pub(super) fn certify(
    input: &Input,
    artifact: &Artifact,
    host: MemoryHost,
    memory_limit: usize,
) -> (Result<ExecutionOutcome, ExecutionError>, MemoryHost, usize) {
    let limits = ExecutionLimits {
        max_memory_bytes: memory_limit,
        ..ExecutionLimits::default()
    };
    let mut raw_host = RecordingHost::new(host.clone());
    let raw = execute_bytecode(input, &mut raw_host, &limits);
    for stage in artifact
        .stages
        .iter()
        .filter(|stage| stage.kind != StageKind::Decoded)
    {
        let mut graph_host = RecordingHost::new(host.clone());
        let graph = execute_ir(stage, &mut graph_host, &limits);
        assert_eq!(
            raw.as_ref().map(|e| &e.outcome),
            graph.as_ref().map(|e| &e.outcome)
        );
        assert_eq!(
            raw_host, graph_host,
            "IR changed primitive requests/results, fault prefix or host state"
        );
        if stage.ir.structured.is_some() {
            let mut source_host = RecordingHost::new(host.clone());
            let source = execute_structured(stage, &mut source_host, &limits);
            assert_eq!(
                raw.as_ref().map(|e| &e.outcome),
                source.as_ref().map(|e| &e.outcome)
            );
            assert_eq!(raw_host, source_host);
        }
    }
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let index = program.render_index(&stage.ir);
    index.validate_guard_sources().unwrap();
    let mut machine = Machine {
        stage,
        program,
        index,
        host: RecordingHost::new(host),
        values: BTreeMap::new(),
        current: stage.ir.entry.unwrap(),
        observations: Vec::new(),
        writes: Vec::new(),
        memory_limit,
        capsules: 0,
        steps: 0,
    };
    let normalized = machine
        .syntax(&stage.ir.structured.as_ref().unwrap().root)
        .map(|flow| {
            let Flow::End(outcome) = flow else {
                panic!("normalized view lost terminal behavior")
            };
            outcome
        });
    assert_eq!(
        normalized.as_ref(),
        raw.as_ref().map(|e| &e.outcome),
        "normalized display changed exit or first fault"
    );
    assert_eq!(
        machine.host, raw_host,
        "explicit failure-first source changed primitive requests/results, fault prefix or host state"
    );
    if let Ok(raw) = &raw {
        let observations: Vec<_> = raw
            .trace
            .iter()
            .filter(|op| !op.effects.is_pure())
            .map(|op| (op.pc, op.opcode, op.inputs.clone(), op.outputs.clone()))
            .collect();
        assert_eq!(
            machine.observations, observations,
            "normalized view lost or reordered an observation"
        );
    }
    (normalized, machine.host.state, machine.capsules)
}
