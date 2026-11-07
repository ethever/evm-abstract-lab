//! Execute the sharing projection itself. A call runs its canonical definition
//! in a fresh scalar frame; only declared inputs/outputs cross that boundary.
//! Memory, storage, returndata and primitive requests keep one shared host.
use super::support::{RecordingHost, source_execution};
use crate::expressions;
use evm_decompile::{
    Artifact, EntityId, ExecutionError, ExecutionLimits, ExecutionOutcome, ExitKind, Input,
    LiftedProgram, LiftedValue, MemoryHost, Primitive, SemanticHost, SharedInvocation, Stage,
    StructuredKind as Kind, StructuredNode, Terminator, U256, ValueKind,
};
use std::collections::BTreeMap;

type Observation = (Option<usize>, u8, Vec<U256>, Vec<U256>);

#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Next,
    Break,
    Continue,
    Escaped(EntityId),
    End(ExecutionOutcome),
}

struct Frame<'a> {
    values: BTreeMap<EntityId, U256>,
    current: Option<EntityId>,
    invocation: Option<&'a SharedInvocation>,
}

impl Frame<'_> {
    fn actual(&self, formal: EntityId) -> EntityId {
        self.invocation.map_or(formal, |call| {
            call.actual(formal).expect("complete invocation source map")
        })
    }

    fn value(&self, id: EntityId) -> Result<U256, ExecutionError> {
        self.values
            .get(&id)
            .copied()
            .ok_or(ExecutionError::UnassignedValue(id))
    }
}

pub(super) struct Report {
    pub outcome: Result<ExecutionOutcome, ExecutionError>,
    pub host: RecordingHost,
    pub calls: Vec<EntityId>,
    observations: Vec<Observation>,
}

struct Machine<'a> {
    stage: &'a Stage,
    high: &'a LiftedProgram,
    nodes: BTreeMap<EntityId, &'a StructuredNode>,
    host: RecordingHost,
    limits: &'a ExecutionLimits,
    calls: Vec<EntityId>,
    steps: usize,
    observations: Vec<Observation>,
}

fn nodes(root: &StructuredNode) -> BTreeMap<EntityId, &StructuredNode> {
    let mut result = BTreeMap::new();
    let mut todo = vec![root];
    while let Some(node) = todo.pop() {
        assert!(result.insert(node.id, node).is_none());
        match &node.kind {
            Kind::Sequence(children) => todo.extend(children),
            Kind::If {
                then_branch,
                else_branch,
                ..
            } => todo.extend([&**then_branch, &**else_branch]),
            Kind::While { header, body, .. } => todo.extend([&**header, &**body]),
            Kind::Indirect { arms, .. } => todo.extend(arms.iter().map(|arm| &arm.body)),
            _ => {}
        }
    }
    result
}

impl<'a> Machine<'a> {
    fn word(&self, expression: EntityId, frame: &Frame<'a>) -> U256 {
        let LiftedValue::Word(value) =
            expressions::evaluate(self.high, expression, frame.current.unwrap(), &frame.values)
        else {
            panic!("predicate used as a procedure word without a cast")
        };
        value
    }

    fn predicate(&self, expression: EntityId, frame: &Frame<'a>) -> bool {
        let LiftedValue::Predicate(value) =
            expressions::evaluate(self.high, expression, frame.current.unwrap(), &frame.values)
        else {
            panic!("word used as a procedure condition without a truth test")
        };
        value
    }

    fn block(&mut self, id: EntityId, frame: &mut Frame<'a>) -> Result<Flow, ExecutionError> {
        assert_eq!(
            frame.current,
            Some(id),
            "definition body lost its control transfer"
        );
        let block = self.stage.ir.block(id).unwrap();
        if matches!(block.terminator, Terminator::Pending) {
            return Ok(Flow::End(ExecutionOutcome::PartialJump { target: None }));
        }
        let high = self.high.block(id).unwrap();
        for statement in &high.statements {
            if self.steps == self.limits.max_steps {
                return Ok(Flow::End(ExecutionOutcome::StepLimit));
            }
            self.steps += 1;
            let operation = block
                .operations
                .iter()
                .find(|operation| operation.id == statement.operation)
                .unwrap();
            if block
                .operations
                .last()
                .is_some_and(|last| last.id == operation.id)
                && matches!(block.terminator, Terminator::Trap { .. })
            {
                break;
            }
            let inputs: Vec<_> = statement
                .inputs
                .iter()
                .map(|id| self.word(*id, frame))
                .collect();
            let code = operation.opcode;
            let outputs = if (0x5f..=0x7f).contains(&code) {
                let constant = operation
                    .outputs
                    .first()
                    .and_then(|id| self.stage.ir.value(*id))
                    .unwrap();
                let ValueKind::Constant(value) = constant.kind else {
                    panic!("push immediate")
                };
                vec![value]
            } else if code == 0x58 {
                // PC observes the actual call site's source operation.
                let actual = frame.actual(operation.id);
                let source = self
                    .stage
                    .ir
                    .blocks
                    .iter()
                    .flat_map(|block| &block.operations)
                    .find(|op| op.id == actual)
                    .unwrap();
                vec![U256::from(source.source.unwrap().pc)]
            } else if code == 0x38 {
                vec![U256::from(self.stage.ir.bytecode_len)]
            } else if evm_bytecode::word::arity(code).is_some() {
                vec![evm_bytecode::word::evaluate(code, &inputs).unwrap()]
            } else if matches!(code, 0x50 | 0x56 | 0x57 | 0x5b | 0x80..=0x9f) {
                Vec::new()
            } else {
                self.host.execute(Primitive {
                    opcode: code,
                    inputs: &inputs,
                    output_count: operation.outputs.len(),
                    memory_limit: self.limits.max_memory_bytes,
                })?
            };
            assert_eq!(outputs.len(), operation.outputs.len());
            if !operation.effects.is_pure() {
                let actual = frame.actual(operation.id);
                let actual_operation = self
                    .stage
                    .ir
                    .blocks
                    .iter()
                    .flat_map(|block| &block.operations)
                    .find(|operation| operation.id == actual)
                    .unwrap();
                self.observations.push((
                    actual_operation.source.map(|source| source.pc),
                    code,
                    inputs,
                    outputs.clone(),
                ));
            }
            for id in &statement.outputs {
                let position = operation
                    .outputs
                    .iter()
                    .position(|output| output == id)
                    .unwrap();
                frame.values.insert(*id, outputs[position]);
            }
        }
        Ok(Flow::Next)
    }

    fn transfer(&self, id: EntityId, frame: &mut Frame<'a>) -> Result<(), ExecutionError> {
        let edge = self.stage.ir.edge(id).unwrap();
        assert_eq!(frame.current, Some(edge.from));
        let target = self.high.block(edge.to).unwrap();
        let arguments: Vec<_> = self
            .high
            .transfer(id)
            .unwrap()
            .arguments
            .iter()
            .map(|id| self.word(*id, frame))
            .collect();
        assert_eq!(arguments.len(), target.parameters.len());
        // All RHS values precede every destination assignment, including swaps.
        for (id, value) in target.parameters.iter().zip(arguments) {
            frame.values.insert(*id, value);
        }
        frame.current = Some(edge.to);
        Ok(())
    }

    fn terminal(&self, block: EntityId, frame: &Frame<'a>) -> Result<Flow, ExecutionError> {
        let outcome = match &self.stage.ir.block(block).unwrap().terminator {
            Terminator::Exit { kind, .. } => ExecutionOutcome::Exit {
                kind: *kind,
                values: self
                    .high
                    .block(block)
                    .unwrap()
                    .exit_values
                    .iter()
                    .map(|id| self.word(*id, frame))
                    .collect(),
            },
            Terminator::Trap { reason } => ExecutionOutcome::Exceptional {
                reason: reason.clone(),
            },
            Terminator::Pending => ExecutionOutcome::PartialJump { target: None },
            Terminator::Jump {
                unresolved: true, ..
            } => ExecutionOutcome::PartialJump {
                target: self
                    .high
                    .block(block)
                    .unwrap()
                    .target
                    .map(|target| self.word(target, frame)),
            },
            _ => panic!("nonterminal exit in independent procedure machine"),
        };
        Ok(Flow::End(outcome))
    }

    fn choose(
        &self,
        block: EntityId,
        frame: &Frame<'a>,
    ) -> Result<Result<EntityId, ExecutionOutcome>, ExecutionError> {
        let block = self.stage.ir.block(block).unwrap();
        let high = self.high.block(block.id).unwrap();
        let (edges, unresolved) = match &block.terminator {
            Terminator::Fallthrough { edge } => return Ok(Ok(*edge)),
            Terminator::Jump {
                edges, unresolved, ..
            } => (edges, *unresolved),
            Terminator::Branch {
                then_edges,
                else_edge,
                unresolved,
                ..
            } => {
                if !self.predicate(high.condition.unwrap(), frame) {
                    return Ok(else_edge.ok_or(ExecutionOutcome::Exit {
                        kind: ExitKind::EndOfCode,
                        values: Vec::new(),
                    }));
                }
                (then_edges, *unresolved)
            }
            _ => {
                let Flow::End(outcome) = self.terminal(block.id, frame)? else {
                    unreachable!()
                };
                return Ok(Err(outcome));
            }
        };
        let target = self.word(high.target.unwrap(), frame);
        for edge in edges {
            let edge = self.stage.ir.edge(*edge).unwrap();
            if self
                .stage
                .ir
                .block(edge.to)
                .unwrap()
                .source
                .is_some_and(|source| U256::from(source.pc) == target)
            {
                return Ok(Ok(edge.id));
            }
        }
        Ok(Err(if unresolved {
            ExecutionOutcome::PartialJump {
                target: Some(target),
            }
        } else {
            ExecutionOutcome::Exceptional {
                reason: "invalid jump destination".to_owned(),
            }
        }))
    }

    fn invoke(
        &mut self,
        call: &'a SharedInvocation,
        caller: &mut Frame<'a>,
    ) -> Result<Flow, ExecutionError> {
        assert!(
            caller.invocation.is_none(),
            "definition references are acyclic leaf code"
        );
        let procedure = self
            .high
            .procedures
            .iter()
            .find(|procedure| procedure.id == call.procedure)
            .unwrap();
        let body = self.nodes[&procedure.body];
        let formal_current = caller.current.map(|actual| {
            call.bindings
                .iter()
                .find(|binding| binding.actual == actual)
                .map_or(actual, |binding| binding.formal)
        });
        let mut local = Frame {
            values: BTreeMap::new(),
            current: formal_current,
            invocation: Some(call),
        };
        for formal in &procedure.interface.inputs {
            let actual = call.actual(*formal).expect("formal argument map");
            local.values.insert(*formal, caller.value(actual)?);
        }
        self.calls.push(call.id);
        let flow = self.syntax(body, &mut local)?;
        if matches!(flow, Flow::Next | Flow::Escaped(_)) {
            for formal in &procedure.interface.outputs {
                let actual = call.actual(*formal).expect("formal output map");
                // A normal call must return every promised word, never a stale
                // result left over from an earlier call or a different exit.
                caller.values.insert(actual, local.value(*formal)?);
            }
            caller.current = local
                .current
                .map(|id| call.actual(id).expect("return continuation block map"));
        }
        Ok(match flow {
            Flow::Escaped(id) => Flow::Escaped(call.actual(id).unwrap()),
            other => other,
        })
    }

    fn syntax(
        &mut self,
        node: &'a StructuredNode,
        frame: &mut Frame<'a>,
    ) -> Result<Flow, ExecutionError> {
        if frame.invocation.is_none()
            && let Some(call) = self
                .high
                .invocations
                .iter()
                .find(|call| call.source == node.id)
        {
            return self.invoke(call, frame);
        }
        match &node.kind {
            Kind::Sequence(children) => {
                let mut escaped = None;
                for child in children {
                    if let Some(target) = escaped.take()
                        && child.entry_block(&self.stage.ir) != Some(target)
                    {
                        return Ok(Flow::Escaped(target));
                    }
                    match self.syntax(child, frame)? {
                        Flow::Next => {}
                        Flow::Escaped(target) => escaped = Some(target),
                        other => return Ok(other),
                    }
                }
                Ok(escaped.map_or(Flow::Next, Flow::Escaped))
            }
            Kind::Block { block } => self.block(*block, frame),
            Kind::Transfer { edge } => {
                self.transfer(*edge, frame)?;
                Ok(Flow::Next)
            }
            Kind::If {
                then_branch,
                else_branch,
                ..
            } => self.syntax(
                if self.predicate(self.high.control(node.id).unwrap().condition, frame) {
                    then_branch
                } else {
                    else_branch
                },
                frame,
            ),
            Kind::While {
                header,
                condition,
                condition_nonzero,
                body,
                exit_edge,
            } => {
                let entry = header
                    .entry_block(&self.stage.ir)
                    .or_else(|| body.entry_block(&self.stage.ir));
                for _ in 0..=self.limits.max_steps {
                    match self.syntax(header, frame)? {
                        Flow::Next => {}
                        other => return Ok(other),
                    }
                    if condition.is_some()
                        && self.predicate(self.high.control(node.id).unwrap().condition, frame)
                            != *condition_nonzero
                    {
                        if let Some(edge) = exit_edge {
                            self.transfer(*edge, frame)?;
                        }
                        return Ok(Flow::Next);
                    }
                    match self.syntax(body, frame)? {
                        Flow::Next | Flow::Continue => {}
                        Flow::Break => return Ok(Flow::Next),
                        Flow::Escaped(target) if Some(target) == entry => {}
                        Flow::Escaped(_) => return Ok(Flow::Next),
                        other => return Ok(other),
                    }
                }
                Ok(Flow::End(ExecutionOutcome::StepLimit))
            }
            Kind::Break => Ok(Flow::Break),
            Kind::Continue => Ok(Flow::Continue),
            Kind::Exit { block } | Kind::PartialExit { block } => self.terminal(*block, frame),
            Kind::Indirect { block, arms, .. } => match self.choose(*block, frame)? {
                Err(outcome) => Ok(Flow::End(outcome)),
                Ok(edge) => {
                    let destination = self.stage.ir.edge(edge).unwrap().to;
                    let pc = self.stage.ir.block(destination).unwrap().source.unwrap().pc;
                    self.syntax(
                        &arms.iter().find(|arm| arm.target_pc == pc).unwrap().body,
                        frame,
                    )
                }
            },
            Kind::Dispatcher { entry, cases } => {
                let mut state = *entry;
                for _ in 0..=self.limits.max_steps {
                    let case = cases.iter().find(|case| case.block == state).unwrap();
                    if let flow @ Flow::End(_) = self.block(case.block, frame)? {
                        return Ok(flow);
                    }
                    match self.choose(case.block, frame)? {
                        Err(outcome) => return Ok(Flow::End(outcome)),
                        Ok(edge) => {
                            let transition = case
                                .transitions
                                .iter()
                                .find(|transition| transition.edge == edge)
                                .unwrap();
                            self.transfer(edge, frame)?;
                            state = transition.target;
                            if !cases.iter().any(|case| case.block == state) {
                                return Ok(Flow::Escaped(state));
                            }
                        }
                    }
                }
                Ok(Flow::End(ExecutionOutcome::StepLimit))
            }
        }
    }
}

pub(super) fn run(stage: &Stage, host: MemoryHost, limits: &ExecutionLimits) -> Report {
    let high = stage.ir.lifted.as_ref().unwrap();
    high.validate_shared_sources(&stage.ir, |_| true)
        .expect("direct supplied source-certificate admission");
    let root = &stage.ir.structured.as_ref().unwrap().root;
    let mut machine = Machine {
        stage,
        high,
        nodes: nodes(root),
        host: RecordingHost::new(host),
        limits,
        calls: Vec::new(),
        steps: 0,
        observations: Vec::new(),
    };
    let mut frame = Frame {
        values: BTreeMap::new(),
        current: stage.ir.entry,
        invocation: None,
    };
    let outcome = machine.syntax(root, &mut frame).map(|flow| {
        let Flow::End(outcome) = flow else {
            panic!("procedure projection lost a terminal outcome: {flow:?}")
        };
        outcome
    });
    Report {
        outcome,
        host: machine.host,
        calls: machine.calls,
        observations: machine.observations,
    }
}

pub(super) fn certify(
    input: &Input,
    artifact: &Artifact,
    host: MemoryHost,
    limits: &ExecutionLimits,
) -> Report {
    let (expected, expected_host, raw) = source_execution(input, artifact, host.clone(), limits);
    let report = run(artifact.current().unwrap(), host, limits);
    assert_eq!(
        report.outcome, expected,
        "actual invocation/definition execution outcome"
    );
    if let Some(raw) = raw {
        let observations: Vec<_> = raw
            .trace
            .into_iter()
            .filter(|operation| !operation.effects.is_pure())
            .map(|operation| {
                (
                    operation.pc,
                    operation.opcode,
                    operation.inputs,
                    operation.outputs,
                )
            })
            .collect();
        assert_eq!(
            report.observations, observations,
            "normalized procedure successful source PCs and primitive order/count"
        );
    }
    assert_eq!(
        report.host, expected_host,
        "actual invocation/definition complete state and primitive attempts"
    );
    report
}
