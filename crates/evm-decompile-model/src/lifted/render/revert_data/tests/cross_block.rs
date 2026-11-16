//! Ancestor bindings need real declarations and a closed path from the entry.
use super::*;
use crate::{Edge, EdgeKind, ExitKind, FactScope, LiftedTransfer};

struct Graph {
    ir: Ir,
    program: LiftedProgram,
    next: u64,
}
impl Graph {
    fn new() -> Self {
        Self {
            ir: Ir {
                fact_scope: FactScope::CompleteLocalCfg,
                ..Ir::default()
            },
            program: LiftedProgram::default(),
            next: 1,
        }
    }
    fn id(&mut self) -> EntityId {
        let id = EntityId(self.next);
        self.next += 1;
        id
    }
    fn block(&mut self) -> usize {
        let id = self.id();
        let memory_parameter = self.id();
        let effect_parameter = self.id();
        let high_id = self.id();
        self.ir.entry.get_or_insert(id);
        self.ir.blocks.push(Block {
            id,
            source: None,
            parameters: vec![],
            memory_parameter,
            effect_parameter,
            operations: vec![],
            terminator: Terminator::Exit {
                kind: ExitKind::Stop,
                values: vec![],
            },
        });
        self.program.blocks.push(LiftedBlock {
            id: high_id,
            block: id,
            parameters: vec![],
            statements: vec![],
            condition: None,
            target: None,
            exit_values: vec![],
        });
        self.ir.blocks.len() - 1
    }
    fn parameter(&mut self, block: usize) -> EntityId {
        let id = self.id();
        self.ir.values.push(Value {
            id,
            kind: ValueKind::Parameter {
                block: self.ir.blocks[block].id,
                index: self.ir.blocks[block].parameters.len(),
            },
            fact: ValueFact::Unknown,
        });
        self.ir.blocks[block].parameters.push(id);
        id
    }
    fn emit(&mut self, block: usize, opcode: u8, inputs: Vec<EntityId>) -> Option<EntityId> {
        let id = self.id();
        let result = (!matches!(opcode, 0x52 | 0x57)).then(|| self.id());
        if let Some(value) = result {
            self.ir.values.push(Value {
                id: value,
                kind: if opcode == 0x5f {
                    ValueKind::Constant(crate::U256::ZERO)
                } else {
                    ValueKind::Operation {
                        operation: id,
                        index: 0,
                    }
                },
                fact: ValueFact::Unknown,
            });
        }
        let mut expressions = vec![];
        for input in &inputs {
            let expression = self.id();
            self.program.expressions.push(LiftedExpression {
                id: expression,
                block: self.ir.blocks[block].id,
                ty: LiftedType::Word256,
                kind: X::Value(*input),
            });
            expressions.push(expression);
        }
        self.ir.blocks[block].operations.push(Operation {
            id,
            source: None,
            opcode,
            name: format!("op{opcode:02x}"),
            inputs,
            outputs: result.into_iter().collect(),
            effects: Effects::default(),
            memory_input: None,
            memory_output: None,
            replacement: None,
            effect_input: None,
            effect_output: None,
        });
        let statement = self.id();
        self.program.blocks[block].statements.push(LiftedStatement {
            id: statement,
            operation: id,
            inputs: expressions,
            outputs: result.into_iter().collect(),
        });
        result
    }
    fn load(&mut self, block: usize) -> EntityId {
        let offset = self.emit(block, 0x5f, vec![]).unwrap();
        self.emit(block, 0x35, vec![offset]).unwrap()
    }
    fn edge(
        &mut self,
        from: usize,
        to: usize,
        kind: EdgeKind,
        arguments: Vec<EntityId>,
    ) -> EntityId {
        let id = self.id();
        self.ir.edges.push(Edge {
            id,
            from: self.ir.blocks[from].id,
            to: self.ir.blocks[to].id,
            kind,
            arguments,
            memory: self.ir.blocks[from].memory_parameter,
            effect: self.ir.blocks[from].effect_parameter,
        });
        let high_id = self.id();
        self.program.transfers.push(LiftedTransfer {
            id: high_id,
            edge: id,
            arguments: vec![],
        });
        id
    }
    fn connect(&mut self, from: usize, to: usize, arguments: Vec<EntityId>) {
        let edge = self.edge(from, to, EdgeKind::Fallthrough, arguments);
        self.ir.blocks[from].terminator = Terminator::Fallthrough { edge };
    }
    fn branch(&mut self, from: usize, yes: usize, no: usize) -> (EntityId, EntityId) {
        let condition = self.load(from);
        let target = self.emit(from, 0x5f, vec![]).unwrap();
        self.emit(from, 0x57, vec![target, condition]);
        let yes = self.edge(from, yes, EdgeKind::True, vec![]);
        let no = self.edge(from, no, EdgeKind::False, vec![]);
        self.ir.blocks[from].terminator = Terminator::Branch {
            condition,
            target,
            then_edges: vec![yes],
            else_edge: Some(no),
            unresolved: false,
            invalid_targets: vec![],
        };
        (yes, no)
    }
}

struct Fixture {
    graph: Graph,
    entry: usize,
    decision: usize,
    failure: usize,
    success: usize,
    value: EntityId,
    raw_argument: EntityId,
    expression: EntityId,
    control: LiftedControl,
}
impl Fixture {
    fn new() -> Self {
        let mut graph = Graph::new();
        let entry = graph.block();
        let decision = graph.block();
        let failure = graph.block();
        let success = graph.block();
        let value = graph.load(entry);
        let decision_argument = graph.parameter(decision);
        let raw_argument = graph.parameter(failure);
        graph.connect(entry, decision, vec![value]);
        let (failure_edge, pass_edge) = graph.branch(decision, failure, success);
        graph
            .ir
            .edges
            .iter_mut()
            .find(|e| e.id == failure_edge)
            .unwrap()
            .arguments = vec![decision_argument];
        let offset = graph.emit(failure, 0x5f, vec![]).unwrap();
        graph.emit(failure, 0x52, vec![offset, raw_argument]);
        let expression = graph.program.blocks[failure]
            .statements
            .last()
            .unwrap()
            .inputs[1];
        graph
            .program
            .expressions
            .iter_mut()
            .find(|e| e.id == expression)
            .unwrap()
            .kind = X::Value(value);
        let control = LiftedControl {
            id: graph.id(),
            node: graph.id(),
            condition: graph.id(),
            guard: Some(LiftedGuard {
                block: graph.ir.blocks[decision].id,
                pass_nonzero: false,
                pass_edge,
                failure_edge,
                pass_node: graph.id(),
                failure_node: graph.id(),
                payload: LiftedGuardPayload::EmptyRevert,
                continuation: None,
            }),
        };
        Self {
            graph,
            entry,
            decision,
            failure,
            success,
            value,
            raw_argument,
            expression,
            control,
        }
    }
    fn binds(&self) -> bool {
        let index = self
            .graph
            .program
            .render_base_index_with_budget(&self.graph.ir, |_| true);
        index
            .guard_expression(self.expression, &self.control, &mut |_| true)
            .unwrap()
            .is_some()
    }
    fn move_binding(&mut self, to: usize) {
        let source = &mut self.graph.ir.blocks[self.entry];
        let position = source
            .operations
            .iter()
            .position(|o| o.outputs.contains(&self.value))
            .unwrap();
        let operation = source.operations.remove(position);
        let high = &mut self.graph.program.blocks[self.entry];
        let position = high
            .statements
            .iter()
            .position(|s| s.operation == operation.id)
            .unwrap();
        let statement = high.statements.remove(position);
        for expression in &statement.inputs {
            self.graph
                .program
                .expressions
                .iter_mut()
                .find(|e| e.id == *expression)
                .unwrap()
                .block = self.graph.ir.blocks[to].id;
        }
        self.graph.ir.blocks[to].operations.push(operation);
        self.graph.program.blocks[to].statements.push(statement);
    }
}

#[test]
fn retained_ancestor_binding_survives_pruned_phi_tuples() {
    let fixture = Fixture::new();
    let index = fixture
        .graph
        .program
        .render_base_index_with_budget(&fixture.graph.ir, |_| true);
    assert!(
        fixture.graph.program.blocks[fixture.failure]
            .parameters
            .is_empty()
    );
    assert!(
        index
            .word_sources
            .copy_identity(fixture.raw_argument, fixture.value, &mut |_| true)
            .unwrap()
    );
    assert!(fixture.binds());
}

#[test]
fn retained_ancestor_parameters_require_the_real_source_slot() {
    let mut fixture = Fixture::new();
    let parameter = fixture.graph.parameter(fixture.entry);
    fixture.graph.program.blocks[fixture.entry]
        .parameters
        .push(parameter);
    fixture.graph.ir.edges[0].arguments = vec![parameter];
    fixture
        .graph
        .program
        .expressions
        .iter_mut()
        .find(|e| e.id == fixture.expression)
        .unwrap()
        .kind = X::Value(parameter);
    assert!(fixture.binds());
    let value = fixture
        .graph
        .ir
        .values
        .iter_mut()
        .find(|v| v.id == parameter)
        .unwrap();
    let ValueKind::Parameter { index, .. } = &mut value.kind else {
        unreachable!()
    };
    *index += 1;
    assert!(!fixture.binds());
}

#[test]
fn sibling_future_unreachable_and_undeclared_bindings_are_unavailable() {
    let mut sibling = Fixture::new();
    let owner = sibling.graph.block();
    // Replace the entry's single edge with a branch that can bypass the owner.
    sibling.graph.ir.edges.remove(0);
    sibling.graph.program.transfers.remove(0);
    sibling.graph.branch(sibling.entry, owner, sibling.decision);
    sibling
        .graph
        .connect(owner, sibling.decision, vec![sibling.value]);
    sibling.move_binding(owner);
    assert!(!sibling.binds());

    let mut future = Fixture::new();
    future.move_binding(future.failure);
    assert!(!future.binds());

    let mut after_decision = Fixture::new();
    after_decision.move_binding(after_decision.decision);
    assert!(!after_decision.binds());

    let mut unreachable = Fixture::new();
    let owner = unreachable.graph.block();
    unreachable.move_binding(owner);
    assert!(!unreachable.binds());

    let mut undeclared = Fixture::new();
    for statement in &mut undeclared.graph.program.blocks[undeclared.entry].statements {
        statement.outputs.retain(|v| *v != undeclared.value);
    }
    assert!(!undeclared.binds());
}

#[test]
fn expression_scope_and_value_metadata_cannot_forge_an_ancestor_declaration() {
    let mut fixture = Fixture::new();
    fixture.move_binding(fixture.failure);
    let expression = fixture
        .graph
        .program
        .expressions
        .iter_mut()
        .find(|e| e.id == fixture.expression)
        .unwrap();
    expression.block = fixture.graph.ir.blocks[fixture.decision].id;
    assert!(!fixture.binds());

    let mut fixture = Fixture::new();
    let value = fixture
        .graph
        .ir
        .values
        .iter_mut()
        .find(|v| v.id == fixture.value)
        .unwrap();
    value.kind = ValueKind::Operation {
        operation: EntityId(u64::MAX),
        index: 0,
    };
    assert!(!fixture.binds());
}

#[test]
fn closed_loops_keep_anchored_bindings_but_cycles_do_not_invent_copy_identity() {
    let mut fixture = Fixture::new();
    let parameter = fixture.graph.ir.blocks[fixture.decision].parameters[0];
    fixture
        .graph
        .connect(fixture.success, fixture.decision, vec![parameter]);
    assert!(
        fixture.binds(),
        "the entry definition dominates every loop iteration"
    );
    let index = fixture
        .graph
        .program
        .render_base_index_with_budget(&fixture.graph.ir, |_| true);
    assert!(
        !index
            .word_sources
            .copy_identity(fixture.raw_argument, fixture.value, &mut |_| true)
            .unwrap()
    );

    // Removing the entry path leaves an unanchored decision/failure cycle.
    fixture.graph.ir.edges.remove(0);
    fixture.graph.program.transfers.remove(0);
    fixture.graph.ir.blocks[fixture.entry].terminator = Terminator::Exit {
        kind: ExitKind::Stop,
        values: vec![],
    };
    assert!(!fixture.binds());
}

#[test]
fn open_or_malformed_cfg_never_certifies_an_ancestor_binding() {
    for mutation in 0..12 {
        let mut fixture = Fixture::new();
        match mutation {
            0 => fixture.graph.ir.fact_scope = FactScope::RecoveredPaths,
            1 => fixture.graph.ir.entry = None,
            2 => fixture.graph.ir.blocks[fixture.success].terminator = Terminator::Pending,
            3 => {
                let Terminator::Branch { unresolved, .. } =
                    &mut fixture.graph.ir.blocks[fixture.decision].terminator
                else {
                    unreachable!()
                };
                *unresolved = true;
            }
            4 => {
                let Terminator::Branch {
                    invalid_targets, ..
                } = &mut fixture.graph.ir.blocks[fixture.decision].terminator
                else {
                    unreachable!()
                };
                invalid_targets.push(crate::U256::ZERO);
            }
            5 => {
                fixture.graph.ir.edges.remove(0);
            }
            6 => fixture
                .graph
                .ir
                .edges
                .push(fixture.graph.ir.edges[0].clone()),
            7 => {
                fixture
                    .graph
                    .edge(fixture.success, fixture.failure, EdgeKind::Jump, vec![]);
            }
            8 => fixture.graph.ir.edges[0].from = fixture.graph.ir.blocks[fixture.success].id,
            9 => fixture.graph.ir.edges[0].to = EntityId(u64::MAX),
            10 => fixture
                .graph
                .ir
                .blocks
                .push(fixture.graph.ir.blocks[0].clone()),
            11 => {
                let Terminator::Branch { else_edge, .. } =
                    &mut fixture.graph.ir.blocks[fixture.decision].terminator
                else {
                    unreachable!()
                };
                *else_edge = None;
            }
            _ => unreachable!(),
        }
        assert!(
            !fixture.binds(),
            "mutation {mutation} forged an ancestor binding"
        );
    }
}

#[test]
fn dominance_work_refusal_quarantines_the_render_index() {
    let fixture = Fixture::new();
    let mut paid = 0;
    let accepted = fixture
        .graph
        .program
        .render_base_index_with_budget(&fixture.graph.ir, |n| {
            paid += n;
            true
        });
    assert!(
        accepted
            .word_sources
            .bound_at(fixture.value, fixture.graph.ir.blocks[fixture.decision].id)
    );
    assert!(paid > 0);
    let refused = fixture
        .graph
        .program
        .render_base_index_with_budget(&fixture.graph.ir, |_| false);
    assert!(
        !refused
            .word_sources
            .bound_at(fixture.value, fixture.graph.ir.blocks[fixture.decision].id)
    );
    let mut steps = 0;
    assert!(matches!(
        crate::lifted::render::word::WordSources::new(
            &fixture.graph.program,
            Some(&fixture.graph.ir),
            &mut |_| {
                steps += 1;
                steps < 4
            }
        ),
        Err(E::Limit)
    ));
    assert_eq!(
        steps, 4,
        "refusal occurred inside the paid CFG construction"
    );
}
