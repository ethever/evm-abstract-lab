//! Interfaces follow selected AST execution roles and the free Value leaves of
//! normalized expressions, rather than eliminated physical stack temporaries.
use super::{checked, fixtures};
use evm_decompile::{
    EntityId, LiftedExpressionKind as Expression, LiftedProgram, SharedSourceIndex,
    StructuredKind as Kind, StructuredNode,
};
use std::collections::BTreeSet;

pub(super) fn nodes(root: &StructuredNode) -> Vec<&StructuredNode> {
    let mut todo = vec![root];
    let mut result = Vec::new();
    while let Some(node) = todo.pop() {
        result.push(node);
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

fn leaves(high: &LiftedProgram, roots: impl IntoIterator<Item = EntityId>) -> BTreeSet<EntityId> {
    let mut todo: Vec<_> = roots.into_iter().collect();
    let mut seen = BTreeSet::new();
    let mut result = BTreeSet::new();
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        match &high.expression(id).unwrap().kind {
            Expression::Word(_) | Expression::Predicate(_) => {}
            Expression::Value(id) => {
                result.insert(*id);
            }
            Expression::WordFromPredicate(id) | Expression::NonZero(id) | Expression::Not(id) => {
                todo.push(*id)
            }
            Expression::Compare { left, right, .. } => todo.extend([*left, *right]),
            Expression::WordOp { arguments, .. } => todo.extend(arguments),
        }
    }
    result
}

#[test]
fn condition_templates_and_nonterminal_bodies_have_executable_boundaries() {
    let mut external_conditions = 0;
    let mut terminal_captures = 0;
    let mut body_outputs = 0;
    for fixture in [fixtures::sequential(), fixtures::repeated_tail(0xf3)] {
        let artifact = checked(&fixture.input);
        let stage = artifact.current().unwrap();
        let high = stage.ir.lifted.as_ref().unwrap();
        let index = SharedSourceIndex::new(&stage.ir, &mut |_| true).unwrap();
        for node in nodes(&stage.ir.structured.as_ref().unwrap().root) {
            match &node.kind {
                Kind::If { .. } => {
                    let condition = high.control(node.id).unwrap().condition;
                    let inputs = if let Ok(region) = index.region(node.id, &mut |_| true) {
                        region.interface.inputs.into_iter().collect()
                    } else {
                        match index.arithmetic_region(condition, &mut |_| true) {
                            Ok(expression) => leaves(high, expression.interface.inputs),
                            Err(_) if leaves(high, [condition]).is_empty() => BTreeSet::new(),
                            Err(error) => panic!("dynamic condition template rejected: {error}"),
                        }
                    };
                    for value in leaves(high, [condition]) {
                        assert!(
                            inputs.contains(&value),
                            "condition template omitted free condition value {value:?}"
                        );
                        external_conditions += 1;
                    }
                }
                Kind::Exit { block } => {
                    assert!(index.region(node.id, &mut |_| true).is_err());
                    for value in leaves(
                        high,
                        high.block(*block).unwrap().exit_values.iter().copied(),
                    ) {
                        assert!(stage.ir.value(value).is_some());
                        terminal_captures += 1;
                    }
                }
                Kind::Sequence(children) => {
                    for pair in children.windows(2) {
                        let Kind::Block { block } = pair[0].kind else {
                            continue;
                        };
                        let required = match &pair[1].kind {
                            Kind::If { .. } => {
                                leaves(high, [high.control(pair[1].id).unwrap().condition])
                            }
                            Kind::Exit { block: exit } if *exit == block => {
                                leaves(high, high.block(block).unwrap().exit_values.iter().copied())
                            }
                            _ => BTreeSet::new(),
                        };
                        let produced: BTreeSet<_> = high
                            .block(block)
                            .unwrap()
                            .statements
                            .iter()
                            .flat_map(|statement| statement.outputs.iter().copied())
                            .collect();
                        let required: Vec<_> = required.intersection(&produced).copied().collect();
                        if required.is_empty() {
                            continue;
                        }
                        if matches!(
                            stage.ir.block(block).unwrap().terminator,
                            evm_decompile::Terminator::Exit { .. }
                                | evm_decompile::Terminator::Trap { .. }
                        ) {
                            assert!(index.region(pair[0].id, &mut |_| true).is_err());
                            continue;
                        }
                        let region = index.region(pair[0].id, &mut |_| true).unwrap();
                        for value in required {
                            assert!(
                                region.interface.outputs.contains(&value),
                                "body-only definition hid following control/terminal value {value:?}"
                            );
                            body_outputs += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    assert!(
        external_conditions > 0,
        "fixture must exercise condition captures"
    );
    assert!(
        terminal_captures > 0,
        "fixture must exercise dynamic terminal captures"
    );
    assert!(
        body_outputs > 0,
        "fixture must exercise body-only live outputs"
    );
}
