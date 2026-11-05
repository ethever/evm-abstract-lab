//! Execute the projection against a primitive trace from the direct PC/stack
//! machine. Only retained high statements and projected phis define high values;
//! elided raw pure outputs cannot accidentally supply a missing dynamic capture.
use evm_decompile::{
    Artifact, EntityId, Execution, ExecutionOutcome, ExitKind, Input, LiftedExpressionKind as Expr,
    LiftedProgram, LiftedRelation as Relation, LiftedType, LiftedValue, MemoryHost, Stage,
    StructuredKind, StructuredNode, Terminator, U256,
};
use std::collections::{BTreeMap, BTreeSet};

use super::support::differential;

fn word(value: &LiftedValue) -> U256 {
    let LiftedValue::Word(word) = value else {
        panic!("predicate used as a numeric word without an explicit cast: {value:?}");
    };
    *word
}

fn predicate(value: &LiftedValue) -> bool {
    let LiftedValue::Predicate(predicate) = value else {
        panic!("numeric word used as a typed predicate: {value:?}");
    };
    *predicate
}

pub(super) fn signed_less(left: U256, right: U256) -> bool {
    if left.bit(255) == right.bit(255) {
        left < right
    } else {
        left.bit(255)
    }
}

fn relation(relation: Relation, left: U256, right: U256) -> bool {
    match relation {
        Relation::Eq => left == right,
        Relation::Ne => left != right,
        Relation::Ult => left < right,
        Relation::Ule => left <= right,
        Relation::Ugt => left > right,
        Relation::Uge => left >= right,
        Relation::Slt => signed_less(left, right),
        Relation::Sle => !signed_less(right, left),
        Relation::Sgt => signed_less(right, left),
        Relation::Sge => !signed_less(left, right),
    }
}

fn magnitude(value: U256) -> U256 {
    if value.bit(255) {
        (!value).wrapping_add(U256::from(1))
    } else {
        value
    }
}

fn primitive_word(opcode: u8, args: &[U256]) -> U256 {
    let first = args[0];
    let second = args.get(1).copied().unwrap_or_default();
    match opcode {
        0x01 => first.wrapping_add(second),
        0x02 => first.wrapping_mul(second),
        0x03 => first.wrapping_sub(second),
        0x04 => {
            if second == U256::ZERO {
                U256::ZERO
            } else {
                first / second
            }
        }
        0x05 => {
            if second == U256::ZERO {
                return U256::ZERO;
            }
            let result = magnitude(first) / magnitude(second);
            if first.bit(255) != second.bit(255) {
                (!result).wrapping_add(U256::from(1))
            } else {
                result
            }
        }
        0x06 => {
            if second == U256::ZERO {
                U256::ZERO
            } else {
                first % second
            }
        }
        0x07 => {
            if second == U256::ZERO {
                return U256::ZERO;
            }
            let result = magnitude(first) % magnitude(second);
            if first.bit(255) {
                (!result).wrapping_add(U256::from(1))
            } else {
                result
            }
        }
        0x0a => {
            let mut result = U256::from(1);
            let mut base = first;
            let mut exponent = second;
            while exponent != U256::ZERO {
                if exponent.bit(0) {
                    result = result.wrapping_mul(base);
                }
                base = base.wrapping_mul(base);
                exponent >>= 1;
            }
            result
        }
        0x0b => {
            if first >= U256::from(31) {
                return second;
            }
            let bit = usize::try_from(first).unwrap() * 8 + 7;
            let mask = (U256::from(1) << bit) - U256::from(1);
            if second.bit(bit) {
                second | !mask
            } else {
                second & mask
            }
        }
        0x16 => first & second,
        0x17 => first | second,
        0x18 => first ^ second,
        0x19 => !first,
        0x1a => {
            if first >= U256::from(32) {
                U256::ZERO
            } else {
                U256::from(second.to_be_bytes::<32>()[usize::try_from(first).unwrap()])
            }
        }
        0x1b => {
            if first >= U256::from(256) {
                U256::ZERO
            } else {
                second << usize::try_from(first).unwrap()
            }
        }
        0x1c => {
            if first >= U256::from(256) {
                U256::ZERO
            } else {
                second >> usize::try_from(first).unwrap()
            }
        }
        0x1d => {
            let negative = second.bit(255);
            if first >= U256::from(256) {
                return if negative { U256::MAX } else { U256::ZERO };
            }
            let shift = usize::try_from(first).unwrap();
            if shift == 0 {
                return second;
            }
            let shifted = second >> shift;
            if negative {
                shifted | (U256::MAX << (256 - shift))
            } else {
                shifted
            }
        }
        0x1e => U256::from(first.leading_zeros()),
        _ => panic!("fixture needs an independent reference for WordOp 0x{opcode:02x}"),
    }
}

fn dependencies(kind: &Expr) -> Vec<EntityId> {
    match kind {
        Expr::Word(_) | Expr::Predicate(_) | Expr::Value(_) => Vec::new(),
        Expr::WordFromPredicate(id) | Expr::NonZero(id) | Expr::Not(id) => vec![*id],
        Expr::Compare { left, right, .. } => vec![*left, *right],
        Expr::WordOp { arguments, .. } => arguments.clone(),
    }
}

/// Independent flat-DAG evaluator with an explicit current dynamic block.
pub(super) fn evaluate(
    program: &LiftedProgram,
    root: EntityId,
    block: EntityId,
    bindings: &BTreeMap<EntityId, U256>,
) -> LiftedValue {
    let mut needed = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if needed.insert(id) {
            let expression = program.expression(id).expect("expression reference");
            assert_eq!(
                expression.block, block,
                "expression crossed its dynamic block scope"
            );
            pending.extend(dependencies(&expression.kind));
        }
    }
    let mut evaluated = BTreeMap::new();
    for expression in program
        .expressions
        .iter()
        .filter(|expression| needed.contains(&expression.id))
    {
        let operand = |id: &EntityId| evaluated.get(id).expect("topologically earlier operand");
        let result = match &expression.kind {
            Expr::Word(value) => LiftedValue::Word(*value),
            Expr::Predicate(value) => LiftedValue::Predicate(*value),
            Expr::Value(id) => LiftedValue::Word(*bindings.get(id).unwrap_or_else(|| {
                panic!("high projection has no capture/binding for {id:?} in block {block:?}")
            })),
            Expr::WordFromPredicate(id) => {
                LiftedValue::Word(U256::from(u64::from(predicate(operand(id)))))
            }
            Expr::NonZero(id) => LiftedValue::Predicate(word(operand(id)) != U256::ZERO),
            Expr::Not(id) => LiftedValue::Predicate(!predicate(operand(id))),
            Expr::Compare {
                relation: kind,
                left,
                right,
            } => LiftedValue::Predicate(relation(*kind, word(operand(left)), word(operand(right)))),
            Expr::WordOp { opcode, arguments } => LiftedValue::Word(primitive_word(
                *opcode,
                &arguments
                    .iter()
                    .map(|id| word(operand(id)))
                    .collect::<Vec<_>>(),
            )),
        };
        assert!(matches!(
            (&result, expression.ty),
            (LiftedValue::Word(_), LiftedType::Word256)
                | (LiftedValue::Predicate(_), LiftedType::Predicate)
        ));
        evaluated.insert(expression.id, result);
    }
    let result = evaluated.remove(&root).expect("root evaluated");
    assert_eq!(
        program.evaluate_expression(root, |id| bindings.get(&id).copied()),
        Some(result),
        "public evaluator disagrees with independent word/predicate semantics"
    );
    result
}

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
