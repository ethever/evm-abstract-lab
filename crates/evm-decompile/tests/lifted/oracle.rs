//! Execute the projection against a primitive trace from the direct PC/stack
//! machine. Only retained high statements and projected phis define high values;
//! elided raw pure outputs cannot accidentally supply a missing dynamic capture.
use evm_decompile::{
    Artifact, EntityId, Execution, ExecutionOutcome, ExitKind, Input, LiftedProgram, MemoryHost,
    Stage, StructuredKind, StructuredNode, Terminator, U256, WordRewrite,
};
use std::collections::BTreeMap;

use super::support::differential;

#[path = "oracle/expressions.rs"]
mod expressions;

pub(super) use expressions::{evaluate, signed_less};
use expressions::{predicate, word};

fn controls(node: &StructuredNode, out: &mut BTreeMap<EntityId, EntityId>) {
    match &node.kind {
        StructuredKind::Sequence(nodes) => {
            for node in nodes {
                controls(node, out);
            }
        }
        StructuredKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            out.insert(node.id, *condition);
            controls(then_branch, out);
            controls(else_branch, out);
        }
        StructuredKind::While {
            header,
            condition,
            body,
            ..
        } => {
            if let Some(condition) = condition {
                out.insert(node.id, *condition);
            }
            controls(header, out);
            controls(body, out);
        }
        StructuredKind::Indirect { arms, .. } => {
            for arm in arms {
                controls(&arm.body, out);
            }
        }
        _ => {}
    }
}

pub(super) fn certify(
    input: &Input,
    artifact: &Artifact,
    host: MemoryHost,
) -> (Execution, MemoryHost) {
    let (raw, observed) = differential(input, artifact, host);
    let stage = artifact.current().unwrap();
    let program = stage
        .ir
        .lifted
        .as_ref()
        .expect("typed lifted semantic model");
    let outcome = run(stage, program, &raw);
    assert_eq!(
        outcome, raw.outcome,
        "high control and terminal operands disagree with direct EVM execution"
    );
    (raw, observed)
}

fn select_target(
    stage: &Stage,
    edges: &[EntityId],
    value: U256,
    unresolved: bool,
) -> Result<EntityId, ExecutionOutcome> {
    for id in edges {
        let edge = stage.ir.edge(*id).unwrap();
        if stage
            .ir
            .block(edge.to)
            .unwrap()
            .source
            .is_some_and(|source| U256::from(source.pc) == value)
        {
            return Ok(*id);
        }
    }
    Err(if unresolved {
        ExecutionOutcome::PartialJump {
            target: Some(value),
        }
    } else {
        ExecutionOutcome::Exceptional {
            reason: "invalid jump destination".to_owned(),
        }
    })
}

fn run(stage: &Stage, program: &LiftedProgram, raw: &Execution) -> ExecutionOutcome {
    let Some(mut current) = stage.ir.entry else {
        assert!(raw.trace.is_empty());
        return ExecutionOutcome::Exit {
            kind: ExitKind::EndOfCode,
            values: Vec::new(),
        };
    };
    let renderer = program.render_base_index_with_budget(&stage.ir, |_| true);
    let mut raw_values = BTreeMap::new();
    let mut high_values = BTreeMap::new();
    let mut position = 0;
    let mut source_controls = BTreeMap::new();
    if let Some(source) = &stage.ir.structured {
        controls(&source.root, &mut source_controls);
    }
    for _ in 0..100_000 {
        let block = stage.ir.block(current).unwrap();
        let high = program
            .block(current)
            .expect("projection for executed source block");
        if matches!(block.terminator, Terminator::Pending) {
            return ExecutionOutcome::PartialJump { target: None };
        }
        let mut retained = high.statements.iter().peekable();
        for (index, operation) in block.operations.iter().enumerate() {
            let fault = index + 1 == block.operations.len()
                && matches!(block.terminator, Terminator::Trap { .. });
            let statement = if retained
                .peek()
                .is_some_and(|statement| statement.operation == operation.id)
            {
                retained.next()
            } else {
                None
            };
            if fault {
                continue;
            }
            let primitive = raw
                .trace
                .get(position)
                .expect("projection executed an extra primitive/block");
            assert_eq!(
                primitive.pc,
                operation.source.map(|span| span.pc),
                "high control chose the wrong dynamic source block"
            );
            assert_eq!(primitive.opcode, operation.opcode);
            position += 1;
            if let Some(statement) = statement {
                let inputs: Vec<_> = statement
                    .inputs
                    .iter()
                    .map(|id| word(&evaluate(program, *id, current, &high_values)))
                    .collect();
                assert_eq!(
                    inputs, primitive.inputs,
                    "lifted primitive {:?} at {:?} re-evaluated a stale SSA operand",
                    operation.id, operation.source
                );
                // Execute the actual typed RHS view selected by the renderer,
                // using only already-bound high values. Raw/elided pure output
                // values cannot silently supply a missing displayed dependency.
                if let Some(recipe) = renderer.statement_word_rhs(statement, |_| true).unwrap() {
                    let displayed = match recipe {
                        WordRewrite::Existing(id) => {
                            word(&evaluate(program, id, current, &high_values))
                        }
                        WordRewrite::Literal(value) => value,
                        WordRewrite::Masked { value, mask } => {
                            word(&evaluate(program, value, current, &high_values)) & mask
                        }
                        WordRewrite::Unchanged => panic!("unchanged roots use the primitive view"),
                    };
                    assert_eq!(statement.outputs.len(), 1);
                    assert_eq!(primitive.outputs.len(), 1);
                    assert_eq!(
                        displayed, primitive.outputs[0],
                        "derived bound-word RHS changed original operation {:?}",
                        operation.id
                    );
                }
                for output in &statement.outputs {
                    let slot = operation
                        .outputs
                        .iter()
                        .position(|id| id == output)
                        .expect("statement output defined by its primitive");
                    high_values.insert(*output, primitive.outputs[slot]);
                }
            } else {
                assert!(
                    operation.effects.is_pure(),
                    "high projection erased observable primitive 0x{:02x}",
                    operation.opcode
                );
            }
            for (id, output) in operation.outputs.iter().zip(&primitive.outputs) {
                raw_values.insert(*id, *output);
            }
        }
        assert!(
            retained.next().is_none(),
            "high statements changed original primitive order"
        );
        for control in &program.controls {
            if program.expression(control.condition).unwrap().block == current {
                let condition = source_controls[&control.node];
                assert_eq!(
                    predicate(&evaluate(program, control.condition, current, &high_values)),
                    raw_values[&condition] != U256::ZERO,
                    "high structured condition disagrees with its primitive witness"
                );
            }
        }
        let eval_word = |id| word(&evaluate(program, id, current, &high_values));
        let choice = match &block.terminator {
            Terminator::Pending => unreachable!(),
            Terminator::Trap { reason } => Err(ExecutionOutcome::Exceptional {
                reason: reason.clone(),
            }),
            Terminator::Exit { kind, values } => {
                let outputs: Vec<_> = high.exit_values.iter().map(|id| eval_word(*id)).collect();
                assert_eq!(
                    outputs,
                    values.iter().map(|id| raw_values[id]).collect::<Vec<_>>()
                );
                Err(ExecutionOutcome::Exit {
                    kind: *kind,
                    values: outputs,
                })
            }
            Terminator::Fallthrough { edge } => Ok(*edge),
            Terminator::Jump {
                target,
                edges,
                unresolved,
                invalid_targets,
            } => {
                if let Some(expression) = high.target {
                    let value = eval_word(expression);
                    assert_eq!(value, raw_values[target]);
                    select_target(stage, edges, value, *unresolved)
                } else {
                    assert!(
                        !unresolved && invalid_targets.is_empty() && edges.len() == 1,
                        "absorbed jump lacks resolved unique topology"
                    );
                    Ok(edges[0])
                }
            }
            Terminator::Branch {
                condition,
                target,
                then_edges,
                else_edge,
                unresolved,
                invalid_targets,
            } => {
                let test = predicate(&evaluate(
                    program,
                    high.condition.expect("typed branch condition"),
                    current,
                    &high_values,
                ));
                assert_eq!(test, raw_values[condition] != U256::ZERO);
                if !test {
                    else_edge.ok_or(ExecutionOutcome::Exit {
                        kind: ExitKind::EndOfCode,
                        values: Vec::new(),
                    })
                } else if let Some(expression) = high.target {
                    let value = eval_word(expression);
                    assert_eq!(value, raw_values[target]);
                    select_target(stage, then_edges, value, *unresolved)
                } else {
                    assert!(!unresolved && invalid_targets.is_empty() && then_edges.len() == 1);
                    Ok(then_edges[0])
                }
            }
        };
        let edge = match choice {
            Ok(id) => stage.ir.edge(id).unwrap(),
            Err(outcome) => {
                assert_eq!(
                    position,
                    raw.trace.len(),
                    "projection stopped before the direct EVM trace"
                );
                return outcome;
            }
        };
        let target = stage.ir.block(edge.to).unwrap();
        let high_target = program.block(edge.to).unwrap();
        let transfer = program
            .transfer(edge.id)
            .expect("projected high edge transfer");
        assert_eq!(transfer.arguments.len(), high_target.parameters.len());
        let raw_arguments: Vec<_> = edge.arguments.iter().map(|id| raw_values[id]).collect();
        // Evaluate every high RHS against the same pre-transfer environment.
        let high_arguments: Vec<_> = transfer.arguments.iter().map(|id| eval_word(*id)).collect();
        for (parameter, actual) in high_target.parameters.iter().zip(&high_arguments) {
            let slot = target
                .parameters
                .iter()
                .position(|id| id == parameter)
                .expect("high parameter from raw tuple");
            assert_eq!(
                *actual, raw_arguments[slot],
                "projected phi captured a different dynamic value"
            );
        }
        for (id, value) in target.parameters.iter().zip(raw_arguments) {
            raw_values.insert(*id, value);
        }
        for (id, value) in high_target.parameters.iter().zip(high_arguments) {
            high_values.insert(*id, value);
        }
        current = edge.to;
    }
    panic!("high projection exceeded independent fixture execution ceiling");
}
