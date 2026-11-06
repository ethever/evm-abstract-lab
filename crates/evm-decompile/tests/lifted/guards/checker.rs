//! Independent certificate and normalized-source executor. Recognition is never
//! delegated back to the production projector or its certificate checker.
use super::*;
use crate::oracle::evaluate;
use evm_decompile::{
    LiftedGuardPayload, LiftedProgram, LiftedRenderIndex, LiftedValue, Primitive, SemanticHost,
};
use std::collections::{BTreeMap, BTreeSet};

#[path = "checker/tests.rs"]
mod tests;

fn nodes<'a>(node: &'a StructuredNode, out: &mut BTreeMap<EntityId, &'a StructuredNode>) {
    assert!(out.insert(node.id, node).is_none());
    match &node.kind {
        StructuredKind::Sequence(children) => children.iter().for_each(|node| nodes(node, out)),
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            nodes(then_branch, out);
            nodes(else_branch, out);
        }
        StructuredKind::While { header, body, .. } => {
            nodes(header, out);
            nodes(body, out);
        }
        StructuredKind::Indirect { arms, .. } => arms.iter().for_each(|arm| nodes(&arm.body, out)),
        _ => {}
    }
}

fn successors(terminator: &Terminator) -> Option<Vec<EntityId>> {
    match terminator {
        Terminator::Fallthrough { edge } => Some(vec![*edge]),
        Terminator::Jump {
            edges,
            unresolved: false,
            invalid_targets,
            ..
        } if invalid_targets.is_empty() && !edges.is_empty() => Some(edges.clone()),
        Terminator::Branch {
            then_edges,
            else_edge: Some(other),
            unresolved: false,
            invalid_targets,
            ..
        } if invalid_targets.is_empty() && !then_edges.is_empty() => {
            let mut all = then_edges.clone();
            all.push(*other);
            Some(all)
        }
        _ => None,
    }
}

fn transparent(stage: &Stage, block: EntityId, terminal: bool) -> bool {
    let block = stage.ir.block(block).unwrap();
    if stage.ir.lifted.as_ref().unwrap().block(block.id).is_none() {
        return false;
    }
    block.operations.iter().enumerate().all(|(position, op)| {
        // The contract admits total word arithmetic and stack/control
        // bookkeeping, not every residual opcode labelled effect-free.
        (op.effects.is_pure()
            && (evm_bytecode::word::arity(op.opcode).is_some()
                || matches!(op.opcode, 0x50 | 0x56 | 0x57 | 0x5b | 0x5f..=0x9f)))
            || (terminal && position + 1 == block.operations.len() && op.opcode == 0xfd)
    })
}

fn empty_seed(stage: &Stage, block: EntityId) -> bool {
    let block = stage.ir.block(block).unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let Some(high) = program.block(block.id) else {
        return false;
    };
    // Seed zero is a premise from the separately certified typed projection.
    // Raw numeric facts can remain Unknown after typed identities prove zero.
    matches!(&block.terminator, Terminator::Exit { kind: ExitKind::Revert, values }
        if values.len() == 2 && block.operations.last().is_some_and(|op|
            op.opcode == 0xfd && op.inputs == *values))
        && high.exit_values.len() == 2
        && program.expression(high.exit_values[1]).is_some_and(|expression|
            matches!(expression.kind, evm_decompile::LiftedExpressionKind::Word(value) if value == U256::ZERO))
        && transparent(stage, block.id, true)
}

// Forward saturation is deliberately separate from the producer's reverse
// worklist. Equality below checks completeness relative to the published typed
// seed, opcode and closed-successor domain, not all semantically empty EVM paths.
// Finite ranks are checked locally as well as against this fixed point.
fn failure_set(stage: &Stage) -> BTreeSet<EntityId> {
    let mut found: BTreeSet<_> = stage
        .ir
        .blocks
        .iter()
        .filter(|block| empty_seed(stage, block.id))
        .map(|block| block.id)
        .collect();
    loop {
        let added: Vec<_> = stage
            .ir
            .blocks
            .iter()
            .filter(|block| {
                !found.contains(&block.id)
                    && transparent(stage, block.id, false)
                    && successors(&block.terminator).is_some_and(|edges| {
                        edges
                            .iter()
                            .all(|id| found.contains(&stage.ir.edge(*id).unwrap().to))
                    })
            })
            .map(|block| block.id)
            .collect();
        if added.is_empty() {
            return found;
        }
        found.extend(added);
    }
}

// Conservative normal-source fallthrough; true permits keeping a suffix and
// does not assert that a concrete unknown escape actually reaches that suffix.
fn falls_through(stage: &Stage, node: &StructuredNode) -> bool {
    match &node.kind {
        StructuredKind::Sequence(children) => {
            children.iter().all(|node| falls_through(stage, node))
        }
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => falls_through(stage, then_branch) || falls_through(stage, else_branch),
        StructuredKind::Exit { block } => !stage.ir.block(*block).is_some_and(|block| {
            matches!(
                block.terminator,
                Terminator::Exit { .. } | Terminator::Trap { .. }
            )
        }),
        StructuredKind::Break | StructuredKind::Continue => false,
        StructuredKind::Indirect {
            arms, unresolved, ..
        } => {
            *unresolved || arms.is_empty() || arms.iter().any(|arm| falls_through(stage, &arm.body))
        }
        StructuredKind::PartialExit { .. } => true,
        _ => true,
    }
}

fn first_transfer(node: &StructuredNode) -> Option<EntityId> {
    match &node.kind {
        StructuredKind::Transfer { edge } => Some(*edge),
        StructuredKind::Sequence(children) => children.iter().find_map(first_transfer),
        _ => None,
    }
}

pub(super) fn certificate(stage: &Stage) {
    let program = stage.ir.lifted.as_ref().unwrap();
    let root = &stage.ir.structured.as_ref().unwrap().root;
    let mut source = BTreeMap::new();
    nodes(root, &mut source);
    let table: BTreeMap<_, _> = program
        .failures
        .iter()
        .map(|fact| (fact.block, fact.rank))
        .collect();
    assert_eq!(
        table.len(),
        program.failures.len(),
        "duplicate shared failure fact"
    );
    assert_eq!(
        table.keys().copied().collect::<BTreeSet<_>>(),
        failure_set(stage),
        "certificate must cover the least failure fixed point in the published proof domain"
    );
    for (&id, &rank) in &table {
        let block = stage.ir.block(id).unwrap();
        if rank == 0 {
            assert!(
                empty_seed(stage, id),
                "rank-zero fact has observations/payload/fault: {id:?}"
            );
        } else {
            assert!(transparent(stage, id, false));
            let edges = successors(&block.terminator)
                .expect("ranked failure has known nonempty successors");
            let children: Vec<_> = edges
                .iter()
                .map(|id| table[&stage.ir.edge(*id).unwrap().to])
                .collect();
            assert!(
                children.iter().all(|child| *child < rank),
                "cyclic or nondecreasing proof rank"
            );
            assert_eq!(rank, children.into_iter().max().unwrap() + 1);
        }
    }
    let index = program.render_index(&stage.ir);
    let mut omissions = BTreeMap::new();
    for control in &program.controls {
        let Some(guard) = &control.guard else {
            continue;
        };
        assert_eq!(index.guard(control.node), Some(guard));
        let predicate = program.render_expression(control.condition);
        let expected = if guard.pass_nonzero {
            format!("require({predicate});")
        } else {
            format!("require(!({predicate}));")
        };
        assert_eq!(
            index.render_guard(control, |_| true).unwrap(),
            Some(expected),
            "shared formatter must use this exact condition and polarity"
        );
        assert_eq!(guard.payload, LiftedGuardPayload::EmptyRevert);
        let StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } = &source[&control.node].kind
        else {
            panic!("guard must refer to its actual raw If");
        };
        let (pass, fail) = if guard.pass_nonzero {
            (&**then_branch, &**else_branch)
        } else {
            (&**else_branch, &**then_branch)
        };
        assert_eq!((guard.pass_node, guard.failure_node), (pass.id, fail.id));
        let block = stage.ir.block(guard.block).unwrap();
        let Terminator::Branch {
            then_edges,
            else_edge: Some(other),
            unresolved: false,
            invalid_targets,
            ..
        } = &block.terminator
        else {
            panic!("guard needs closed, nonexceptional raw branch");
        };
        assert!(invalid_targets.is_empty());
        assert_eq!(then_edges.len(), 1);
        assert_eq!(
            (guard.pass_edge, guard.failure_edge),
            if guard.pass_nonzero {
                (then_edges[0], *other)
            } else {
                (*other, then_edges[0])
            }
        );
        assert_eq!(
            program.expression(control.condition).unwrap().block,
            block.id
        );
        assert_eq!(
            program.block(block.id).unwrap().condition,
            Some(control.condition)
        );
        assert_eq!(first_transfer(pass), Some(guard.pass_edge));
        assert_eq!(first_transfer(fail), Some(guard.failure_edge));
        assert!(table.contains_key(&stage.ir.edge(guard.failure_edge).unwrap().to));
        assert_eq!(index.node(pass.id), Some(pass));
        assert_eq!(index.node(fail.id), Some(fail));
        if let Some(tail) = &guard.continuation {
            let StructuredKind::Sequence(children) = &source[&tail.sequence].kind else {
                panic!("continuation scope must be its actual source sequence");
            };
            assert_eq!(children[tail.start - 1].id, control.node);
            assert_eq!(tail.end, children.len());
            assert!(tail.start < tail.end);
            assert!(
                falls_through(stage, fail),
                "closed failure arm cannot authorize a following suffix"
            );
            assert_eq!(
                tail.pass_falls_through,
                falls_through(stage, pass),
                "wrongly deleting a pass-reachable join"
            );
            let mut tail_nodes = BTreeMap::new();
            for node in &children[tail.start..tail.end] {
                nodes(node, &mut tail_nodes);
            }
            for node in tail_nodes.values() {
                if let StructuredKind::Block { block } | StructuredKind::Exit { block } = node.kind
                {
                    assert!(
                        table.contains_key(&block),
                        "omitted suffix contains nonfailure block"
                    );
                }
            }
            if !tail.pass_falls_through {
                assert!(
                    omissions
                        .insert((tail.sequence, tail.start), tail.end)
                        .is_none()
                );
            }
        } else if falls_through(stage, fail) {
            // An inner last-child If may fail into an enclosing join. Its
            // global CFG proof authorizes require, but no omission is implied.
            assert!(table.contains_key(&stage.ir.edge(guard.failure_edge).unwrap().to));
        }
    }
    for node in source.values() {
        if let StructuredKind::Sequence(children) = &node.kind {
            for position in 0..=children.len() {
                assert_eq!(
                    index.sequence_end(node.id, position),
                    omissions.get(&(node.id, position)).copied(),
                    "shared formatting plan has a wrong skip range"
                );
            }
        }
    }
}

enum Flow {
    Normal,
    Break,
    Continue,
    End(ExecutionOutcome),
}

struct Machine<'a> {
    stage: &'a Stage,
    program: &'a LiftedProgram,
    index: LiftedRenderIndex<'a>,
    source: BTreeMap<EntityId, &'a StructuredNode>,
    raw: &'a evm_decompile::Execution,
    host: MemoryHost,
    values: BTreeMap<EntityId, U256>,
    current: EntityId,
    position: usize,
    decisions: usize,
    rejected: bool,
    branch: Option<bool>,
}

impl Machine<'_> {
    fn eval(&self, id: EntityId) -> evm_decompile::LiftedValue {
        evaluate(self.program, id, self.current, &self.values)
    }
    fn word(&self, id: EntityId) -> U256 {
        let LiftedValue::Word(value) = self.eval(id) else {
            panic!("word required")
        };
        value
    }
    fn test(&mut self, node: EntityId) -> bool {
        let control = self.program.control(node).unwrap();
        self.decisions += 1;
        let LiftedValue::Predicate(value) = self.eval(control.condition) else {
            panic!("predicate required")
        };
        assert_eq!(
            Some(value),
            self.branch,
            "normalized predicate disagrees with actual original JUMPI"
        );
        value
    }
    fn block(&mut self, id: EntityId) -> Flow {
        assert_eq!(
            self.current, id,
            "normalized plan executed a block before its transfer"
        );
        let block = self.stage.ir.block(id).unwrap();
        let high = self.program.block(id).unwrap();
        self.branch = None;
        for (position, op) in block.operations.iter().enumerate() {
            if position + 1 == block.operations.len()
                && matches!(block.terminator, Terminator::Trap { .. })
            {
                break;
            }
            let primitive = &self.raw.trace[self.position];
            self.position += 1;
            assert_eq!(
                (primitive.pc, primitive.opcode),
                (op.source.map(|s| s.pc), op.opcode),
                "normalized view executed different source code"
            );
            if op.opcode == 0x57 {
                self.branch = Some(primitive.inputs[1] != U256::ZERO);
            }
            if let Some(statement) = high.statements.iter().find(|s| s.operation == op.id) {
                let args: Vec<_> = statement.inputs.iter().map(|id| self.word(*id)).collect();
                assert_eq!(
                    args, primitive.inputs,
                    "normalized statement evaluated a stale observation/phi"
                );
                let outputs = if op.effects.is_pure() {
                    primitive.outputs.clone()
                } else {
                    self.host
                        .execute(Primitive {
                            opcode: op.opcode,
                            inputs: &args,
                            output_count: op.outputs.len(),
                            memory_limit: 1_048_576,
                        })
                        .unwrap()
                };
                assert_eq!(outputs, primitive.outputs);
                for id in &statement.outputs {
                    let index = op.outputs.iter().position(|output| output == id).unwrap();
                    self.values.insert(*id, outputs[index]);
                }
            } else {
                assert!(
                    op.effects.is_pure(),
                    "normalized plan erased a retained observation"
                );
            }
        }
        Flow::Normal
    }
    fn transfer(&mut self, id: EntityId) {
        let edge = self.stage.ir.edge(id).unwrap();
        assert_eq!(edge.from, self.current);
        let transfer = self.program.transfer(id).unwrap();
        let destination = self.program.block(edge.to).unwrap();
        // Parallel RHS evaluation must precede every destination assignment.
        let args: Vec<_> = transfer.arguments.iter().map(|id| self.word(*id)).collect();
        assert_eq!(args.len(), destination.parameters.len());
        for (id, value) in destination.parameters.iter().zip(args) {
            self.values.insert(*id, value);
        }
        self.current = edge.to;
    }
    fn syntax(&mut self, node: &StructuredNode) -> Flow {
        assert!(self.position <= self.raw.trace.len());
        match &node.kind {
            StructuredKind::Sequence(children) => {
                let mut position = 0;
                while position < children.len() {
                    // Consume the exact shared render plan, then independently
                    // execute what that normalized view makes visible.
                    if let Some(end) = self.index.sequence_end(node.id, position) {
                        position = end;
                        continue;
                    }
                    match self.syntax(&children[position]) {
                        Flow::Normal => {}
                        other => return other,
                    }
                    position += 1;
                }
                Flow::Normal
            }
            StructuredKind::Block { block } => self.block(*block),
            StructuredKind::Transfer { edge } => {
                self.transfer(*edge);
                Flow::Normal
            }
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                let test = self.test(node.id);
                if let Some(guard) = &self.program.control(node.id).unwrap().guard {
                    if test != guard.pass_nonzero {
                        self.rejected = true;
                        return Flow::End(ExecutionOutcome::Exit {
                            kind: ExitKind::Revert,
                            values: vec![U256::ZERO, U256::ZERO],
                        });
                    }
                    self.syntax(self.source[&guard.pass_node])
                } else {
                    self.syntax(if test { then_branch } else { else_branch })
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
                    match self.syntax(header) {
                        Flow::Normal => {}
                        other => return other,
                    }
                    if condition.is_some() && self.test(node.id) != *condition_nonzero {
                        if let Some(edge) = exit_edge {
                            self.transfer(*edge);
                        }
                        return Flow::Normal;
                    }
                    match self.syntax(body) {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => return Flow::Normal,
                        other => return other,
                    }
                }
                panic!("normalized fixture loop ceiling");
            }
            StructuredKind::Break => Flow::Break,
            StructuredKind::Continue => Flow::Continue,
            StructuredKind::Exit { block } | StructuredKind::PartialExit { block } => {
                let outcome = match &self.stage.ir.block(*block).unwrap().terminator {
                    Terminator::Exit { kind, .. } => ExecutionOutcome::Exit {
                        kind: *kind,
                        values: self
                            .program
                            .block(*block)
                            .unwrap()
                            .exit_values
                            .iter()
                            .map(|id| self.word(*id))
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
                            .program
                            .block(*block)
                            .unwrap()
                            .target
                            .map(|id| self.word(id)),
                    },
                    _ => panic!("unexpected nonterminal source exit"),
                };
                Flow::End(outcome)
            }
            _ => panic!("fixture needs explicit normalized semantics for {node:?}"),
        }
    }
}

pub(super) fn partial_success(input: &Input, artifact: &Artifact, host: MemoryHost) {
    let limits = evm_decompile::ExecutionLimits::default();
    let mut raw_host = host.clone();
    let raw = evm_decompile::execute_bytecode(input, &mut raw_host, &limits).unwrap();
    assert!(
        matches!(raw.outcome, ExecutionOutcome::Exceptional { .. }),
        "this particular supplied invalid target faults in the direct machine"
    );
    let (stage, program) = lifted(artifact);
    certificate(stage);
    let mut graph_host = host.clone();
    let graph = evm_decompile::execute_ir(stage, &mut graph_host, &limits).unwrap();
    let mut source_host = host.clone();
    let source_execution =
        evm_decompile::execute_structured(stage, &mut source_host, &limits).unwrap();
    assert!(
        matches!(graph.outcome, ExecutionOutcome::PartialJump { target: Some(value) } if value == U256::MAX)
    );
    assert_eq!(source_execution.outcome, graph.outcome);
    assert_eq!(raw.observations(), graph.observations());
    assert_eq!(raw.observations(), source_execution.observations());
    let root = &stage.ir.structured.as_ref().unwrap().root;
    let mut source = BTreeMap::new();
    nodes(root, &mut source);
    let mut machine = Machine {
        stage,
        program,
        index: program.render_index(&stage.ir),
        source,
        raw: &raw,
        host,
        values: BTreeMap::new(),
        current: stage.ir.entry.unwrap(),
        position: 0,
        decisions: 0,
        rejected: false,
        branch: None,
    };
    let Flow::End(outcome) = machine.syntax(root) else {
        panic!("normalized source lost its partial escape")
    };
    assert_eq!(outcome, graph.outcome);
    assert!(!machine.rejected);
    assert_eq!(machine.position, raw.trace.len());
    assert_eq!(machine.decisions, 1);
    assert_eq!(machine.host, raw_host);
    assert_eq!(graph_host, raw_host);
    assert_eq!(source_host, raw_host);
}

pub(super) fn certify(
    input: &Input,
    artifact: &Artifact,
    host: MemoryHost,
) -> (evm_decompile::Execution, MemoryHost) {
    let (raw, observed) = crate::oracle::certify(input, artifact, host.clone());
    let (stage, program) = lifted(artifact);
    certificate(stage);
    let root = &stage.ir.structured.as_ref().unwrap().root;
    let mut source = BTreeMap::new();
    nodes(root, &mut source);
    let mut machine = Machine {
        stage,
        program,
        index: program.render_index(&stage.ir),
        source,
        raw: &raw,
        host,
        values: BTreeMap::new(),
        current: stage.ir.entry.unwrap(),
        position: 0,
        decisions: 0,
        rejected: false,
        branch: None,
    };
    let Flow::End(outcome) = machine.syntax(root) else {
        panic!("normalized source lost explicit exit")
    };
    assert_eq!(
        machine.decisions,
        raw.trace[..machine.position]
            .iter()
            .filter(|op| op.opcode == 0x57)
            .count(),
        "conditions must be evaluated once in original branch order"
    );
    if machine.rejected {
        assert!(
            matches!(raw.outcome, ExecutionOutcome::Exit { kind: ExitKind::Revert, ref values } if values[1] == U256::ZERO)
        );
        assert_eq!(
            outcome,
            ExecutionOutcome::Exit {
                kind: ExitKind::Revert,
                values: vec![U256::ZERO, U256::ZERO]
            }
        );
        assert!(raw.trace[machine.position..].iter().all(|op| op.effects.is_pure() || (op.opcode == 0xfd && op.inputs[1] == U256::ZERO)), "guard failed before a retained observation on its failure path");
    } else {
        assert_eq!(outcome, raw.outcome);
        assert_eq!(machine.position, raw.trace.len());
    }
    assert_eq!(
        machine.host, observed,
        "normalized source changed memory/storage/calls or observation captures"
    );
    (raw, observed)
}
