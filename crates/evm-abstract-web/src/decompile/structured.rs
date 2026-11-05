//! Readable, selectable presentation of the server's actual structured AST.
//! Formatting does not recover control flow or create transformation history.

use super::index::StageIndex;

use evm_abstract_protocol::decompile_model::{
    EntityId, ExitKind, Ir, LiftedRenderIndex, Operation, Stage, StructuredKind, StructuredNode,
    Terminator, ValueKind,
};

mod dispatch;

#[derive(Clone, Copy, Default)]
struct Scope<'a> {
    following: Option<EntityId>,
    outer_loop: Option<LoopScope>,
    lifted: Option<&'a LiftedRenderIndex<'a>>,
    show_state_dependencies: bool,
}
#[derive(Clone, Copy)]
struct LoopScope {
    node: EntityId,
    entry: Option<EntityId>,
}

pub(super) struct Line {
    pub entity: EntityId,
    pub indent: usize,
    pub text: String,
}

pub(super) fn visit(node: &StructuredNode, visitor: &mut impl FnMut(&StructuredNode)) {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        visitor(node);
        match &node.kind {
            StructuredKind::Sequence(nodes) => {
                pending.extend(nodes.iter().rev());
            }
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                pending.push(else_branch);
                pending.push(then_branch);
            }
            StructuredKind::While { header, body, .. } => {
                pending.push(body);
                pending.push(header);
            }
            StructuredKind::Indirect { arms, .. } => {
                pending.extend(arms.iter().rev().map(|arm| &arm.body));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
pub(super) fn lines(stage: &Stage) -> Vec<Line> {
    lines_indexed(stage, &StageIndex::new(&stage.ir))
}

pub(super) fn lines_indexed(stage: &Stage, index: &StageIndex) -> Vec<Line> {
    render_lines(stage, index, false, true)
}

pub(super) fn lifted_lines_indexed(
    stage: &Stage,
    index: &StageIndex,
    show_state_dependencies: bool,
) -> Vec<Line> {
    render_lines(stage, index, true, show_state_dependencies)
}

fn render_lines(
    stage: &Stage,
    index: &StageIndex,
    lifted: bool,
    show_state_dependencies: bool,
) -> Vec<Line> {
    // The shared semantic index is built once for this complete code batch.
    let renderer = lifted
        .then(|| {
            stage
                .ir
                .lifted
                .as_ref()
                .map(|program| program.render_index(&stage.ir))
        })
        .flatten();
    let mut output = Vec::new();
    if let Some(program) = &stage.ir.structured {
        render(
            &program.root,
            &stage.ir,
            index,
            Scope {
                lifted: renderer.as_ref(),
                show_state_dependencies,
                ..Scope::default()
            },
            0,
            &mut output,
        );
    }
    output
}

fn emit(output: &mut Vec<Line>, entity: EntityId, indent: usize, text: impl Into<String>) {
    output.push(Line {
        entity,
        indent,
        text: text.into(),
    });
}

fn render(
    node: &StructuredNode,
    ir: &Ir,
    index: &StageIndex,
    scope: Scope<'_>,
    indent: usize,
    output: &mut Vec<Line>,
) {
    match &node.kind {
        StructuredKind::Sequence(nodes) => {
            emit(
                output,
                node.id,
                indent,
                format!("// sequence ast{} · {} children", node.id.0, nodes.len()),
            );
            for (position, child) in nodes.iter().enumerate() {
                let following = nodes
                    .get(position + 1)
                    .map(|next| index.first_block(ir, next))
                    .unwrap_or(scope.following);
                render(
                    child,
                    ir,
                    index,
                    Scope { following, ..scope },
                    indent,
                    output,
                );
            }
        }
        StructuredKind::Block { block } => body(node.id, *block, ir, index, scope, indent, output),
        StructuredKind::Transfer { edge } => {
            transfer(node.id, *edge, ir, index, scope, indent, output)
        }
        StructuredKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let (owner, predicate) = predicate(node.id, *condition, ir, index, scope.lifted);
            emit(output, owner, indent, format!("if ({predicate}) {{"));
            render(then_branch, ir, index, scope, indent + 1, output);
            emit(output, owner, indent, "} else {");
            render(else_branch, ir, index, scope, indent + 1, output);
            emit(output, owner, indent, "}");
        }
        StructuredKind::While {
            header,
            condition,
            condition_nonzero,
            body,
            exit_edge,
        } => {
            let owner = if scope.lifted.is_some() {
                index
                    .source_control(ir, node.id)
                    .map_or(node.id, |control| control.id)
            } else {
                node.id
            };
            emit(
                output,
                owner,
                indent,
                format!("'loop{}: while (true) {{", node.id.0),
            );
            render(header, ir, index, scope, indent + 1, output);
            if let Some(condition) = condition {
                let condition = if scope.lifted.is_some() {
                    let (_, predicate) = predicate(node.id, *condition, ir, index, scope.lifted);
                    if *condition_nonzero {
                        format!("!({predicate})")
                    } else {
                        predicate
                    }
                } else {
                    format!(
                        "{} {} 0",
                        value(*condition),
                        if *condition_nonzero { "==" } else { "!=" }
                    )
                };
                emit(output, owner, indent + 1, format!("if ({condition}) {{"));
                if let Some(edge) = exit_edge {
                    transfer(owner, *edge, ir, index, scope, indent + 2, output);
                }
                emit(output, owner, indent + 2, "break;");
                emit(output, owner, indent + 1, "}");
            }
            let outer_loop = Some(LoopScope {
                node: node.id,
                entry: index
                    .first_block(ir, header)
                    .or_else(|| index.first_block(ir, body)),
            });
            render(
                body,
                ir,
                index,
                Scope {
                    following: None,
                    outer_loop,
                    ..scope
                },
                indent + 1,
                output,
            );
            emit(output, owner, indent, "}");
        }
        StructuredKind::Break => emit(output, node.id, indent, "break;"),
        StructuredKind::Continue => emit(output, node.id, indent, "continue;"),
        StructuredKind::Exit { block } | StructuredKind::PartialExit { block } => {
            if let Some(block) = index.block(ir, *block) {
                let text = match &block.terminator {
                    // The surrounding If has already selected the arm. Only
                    // its false arm falls off the terminal JUMPI; the true
                    // arm must retain its target and partial/invalid boundary.
                    Terminator::Branch {
                        else_edge: None, ..
                    } if index.branch_arm(ir, node.id) == Some(false) => {
                        "evm.stop(); // JUMPI false continuation: end of code".into()
                    }
                    Terminator::Branch {
                        target,
                        then_edges,
                        else_edge: None,
                        unresolved,
                        invalid_targets,
                        ..
                    } if index.branch_arm(ir, node.id) == Some(true) => jump_boundary(
                        if scope.lifted.is_some() {
                            projected_target(block.id, ir, index, scope.lifted)
                        } else {
                            value(*target)
                        },
                        then_edges,
                        *unresolved,
                        !invalid_targets.is_empty(),
                    ),
                    term => projected_terminator(block.id, term, ir, index, scope.lifted),
                };
                let owner = if scope.lifted.is_some() {
                    index
                        .source_block(ir, block.id)
                        .map_or(node.id, |block| block.id)
                } else {
                    node.id
                };
                emit(output, owner, indent, text);
            } else {
                emit(
                    output,
                    node.id,
                    indent,
                    format!("unavailable_exit(b{});", block.0),
                );
            }
        }
        StructuredKind::Indirect {
            block,
            arms,
            unresolved,
        } => {
            emit(
                output,
                node.id,
                indent,
                if scope.lifted.is_some() {
                    format!(
                        "indirect_jump {} {{",
                        projected_target(*block, ir, index, scope.lifted)
                    )
                } else {
                    format!("indirect_jump b{} {{", block.0)
                },
            );
            for arm in arms {
                emit(
                    output,
                    node.id,
                    indent + 1,
                    format!("target 0x{:x}: {{", arm.target_pc),
                );
                render(&arm.body, ir, index, scope, indent + 2, output);
                emit(output, node.id, indent + 1, "}");
            }
            if *unresolved {
                emit(
                    output,
                    node.id,
                    indent + 1,
                    "unresolved_target: partial_exit;",
                );
            } else {
                emit(
                    output,
                    node.id,
                    indent + 1,
                    "otherwise: exception(\"invalid jump destination\");",
                );
            }
            emit(output, node.id, indent, "}");
        }
        StructuredKind::Dispatcher { entry, cases } => {
            dispatch::show(
                node.id,
                *entry,
                cases,
                dispatch::Context { ir, index, scope },
                indent,
                output,
            );
        }
    }
}

fn body(
    node: EntityId,
    block: EntityId,
    ir: &Ir,
    index: &StageIndex,
    scope: Scope<'_>,
    indent: usize,
    output: &mut Vec<Line>,
) {
    let Some(block) = index.block(ir, block) else {
        return;
    };
    let semantic = scope.lifted.and_then(|_| index.source_block(ir, block.id));
    let owner = semantic.map_or(node, |block| block.id);
    emit(
        output,
        owner,
        indent,
        format!(
            "// b{}{}",
            block.id.0,
            block.source.map_or_else(String::new, |source| format!(
                " · source 0x{:04x}",
                source.pc
            ))
        ),
    );
    if let Some(renderer) = scope.lifted {
        let Some(semantic) = semantic else {
            emit(
                output,
                owner,
                indent,
                format!("unavailable_high_block(b{});", block.id.0),
            );
            return;
        };
        for statement in &semantic.statements {
            if let Some(op) = index.operation(ir, statement.operation) {
                if let Some(reason) = fault_reason(op, ir, index) {
                    emit(
                        output,
                        statement.id,
                        indent,
                        format!("// {}", fault_marker(op, reason)),
                    );
                    continue;
                }
                if op.effects.terminate {
                    continue;
                }
            }
            emit(
                output,
                statement.id,
                indent,
                super::lifted::statement_with_dependencies(
                    renderer,
                    statement,
                    scope.show_state_dependencies,
                ),
            );
        }
        return;
    }
    for op in &block.operations {
        if let Some(reason) = fault_reason(op, ir, index) {
            emit(
                output,
                op.id,
                indent,
                format!("// {}", fault_marker(op, reason)),
            );
            continue;
        }
        // The typed AST owns control transfers and termination. Their original
        // instructions remain in the IR view and provenance index.
        if op.effects.terminate || matches!(op.opcode, 0x50 | 0x56 | 0x57 | 0x5b | 0x80..=0x9f) {
            continue;
        }
        emit(output, op.id, indent, operation(op, ir, index));
    }
}

fn transfer(
    node: EntityId,
    edge: EntityId,
    ir: &Ir,
    index: &StageIndex,
    scope: Scope<'_>,
    indent: usize,
    output: &mut Vec<Line>,
) {
    let Some(edge) = index.edge(ir, edge) else {
        return;
    };
    let Some(target) = index.block(ir, edge.to) else {
        return;
    };
    let semantic = scope
        .lifted
        .and_then(|_| index.source_transfer(ir, edge.id));
    let owner = semantic.map_or(node, |transfer| transfer.id);
    if let Some(renderer) = scope.lifted {
        if let Some(transfer) = semantic {
            if let Some(text) = super::lifted::transfer(renderer, transfer) {
                emit(output, owner, indent, text);
            }
        } else {
            emit(
                output,
                owner,
                indent,
                format!("unavailable_live_transfer(e{});", edge.id.0),
            );
        }
    } else if !target.parameters.is_empty() {
        emit(
            output,
            node,
            indent,
            format!(
                "({}) = ({}); // parallel assignment on e{}",
                values(&target.parameters),
                values(&edge.arguments),
                edge.id.0
            ),
        );
    }
    if scope.lifted.is_some() && !scope.show_state_dependencies {
        return;
    }
    emit(
        output,
        owner,
        indent,
        format!(
            "m{} = m{}; // memory token on e{}",
            target.memory_parameter.0, edge.memory.0, edge.id.0
        ),
    );
    emit(
        output,
        owner,
        indent,
        format!(
            "q{} = q{}; // effect token on e{}",
            target.effect_parameter.0, edge.effect.0, edge.id.0
        ),
    );
}

fn predicate(
    node: EntityId,
    raw: EntityId,
    ir: &Ir,
    index: &StageIndex,
    lifted: Option<&LiftedRenderIndex<'_>>,
) -> (EntityId, String) {
    if lifted.is_none() {
        return (node, format!("{} != 0", value(raw)));
    }
    match (lifted, index.source_control(ir, node)) {
        (Some(renderer), Some(control)) => (
            control.id,
            super::lifted::expression(renderer, control.condition),
        ),
        _ => (node, format!("unavailable_predicate(ast{})", node.0)),
    }
}

fn projected_target(
    block: EntityId,
    ir: &Ir,
    index: &StageIndex,
    renderer: Option<&LiftedRenderIndex<'_>>,
) -> String {
    match (
        renderer,
        index.source_block(ir, block).and_then(|block| block.target),
    ) {
        (Some(renderer), Some(target)) => super::lifted::expression(renderer, target),
        _ => format!("unavailable_target(b{})", block.0),
    }
}

fn projected_terminator(
    block: EntityId,
    term: &Terminator,
    ir: &Ir,
    index: &StageIndex,
    lifted: Option<&LiftedRenderIndex<'_>>,
) -> String {
    let Some(renderer) = lifted else {
        return terminator(term);
    };
    let Some(semantic) = index.source_block(ir, block) else {
        return format!("unavailable_exit(b{});", block.0);
    };
    let edges = |ids: &[EntityId]| {
        ids.iter()
            .map(|id| format!("e{}", id.0))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match term {
        Terminator::Jump {
            edges: known,
            unresolved,
            invalid_targets,
            ..
        } => jump_boundary(
            projected_target(block, ir, index, lifted),
            known,
            *unresolved,
            !invalid_targets.is_empty(),
        ),
        Terminator::Branch {
            then_edges,
            else_edge,
            unresolved,
            invalid_targets,
            ..
        } => {
            let condition = semantic
                .condition
                .map(|id| super::lifted::expression(renderer, id))
                .unwrap_or_else(|| format!("unavailable_predicate(b{})", block.0));
            format!(
                "branch {condition}: jump {} via [{}]; else {};{}{}",
                projected_target(block, ir, index, lifted),
                edges(then_edges),
                else_edge.map_or_else(
                    || "evm.stop() (end of code)".into(),
                    |edge| format!("e{}", edge.0)
                ),
                if *unresolved {
                    " unresolved_target (partial);"
                } else {
                    ""
                },
                if invalid_targets.is_empty() {
                    ""
                } else {
                    " exceptional_invalid_target;"
                },
            )
        }
        Terminator::Exit { kind, .. } => {
            let name = match kind {
                ExitKind::Stop => "stop",
                ExitKind::Return => "return",
                ExitKind::Revert => "revert",
                ExitKind::SelfDestruct => "selfdestruct",
                ExitKind::EndOfCode => return "evm.stop(); // end of code".into(),
            };
            let arguments = semantic
                .exit_values
                .iter()
                .map(|id| super::lifted::expression(renderer, *id))
                .collect::<Vec<_>>()
                .join(", ");
            format!("evm.{name}({arguments});")
        }
        Terminator::Pending => "partial_exit(\"recovery pending\");".into(),
        Terminator::Fallthrough { edge } => format!("follow e{};", edge.0),
        Terminator::Trap { reason } => format!("exception({reason:?});"),
    }
}

fn jump_boundary(target: String, edges: &[EntityId], unresolved: bool, invalid: bool) -> String {
    let edges = edges
        .iter()
        .map(|edge| format!("e{}", edge.0))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "jump {target} via [{edges}]{}{};",
        if unresolved {
            " + unresolved_target (partial)"
        } else {
            ""
        },
        if invalid {
            " + exceptional_invalid_target"
        } else {
            ""
        },
    )
}

pub(super) fn value(id: EntityId) -> String {
    format!("v{}", id.0)
}

pub(super) fn decoded_operation(op: &Operation, bytecode: &[u8]) -> String {
    let Some(span) = op.source else {
        return format!(
            "Original {} · source location unavailable; unrecovered",
            op.name
        );
    };
    let bytes = bytecode
        .get(span.pc..span.end_pc)
        .unwrap_or_default()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "Source 0x{:04x}: {} {bytes} · unrecovered operands/results",
        span.pc, op.name
    )
}
pub(super) fn values(ids: &[EntityId]) -> String {
    ids.iter()
        .map(|id| value(*id))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A fault belongs to the indexed occurrence's owning block, never its PC.
pub(super) fn fault_reason<'a>(op: &Operation, ir: &'a Ir, index: &StageIndex) -> Option<&'a str> {
    let block = ir.blocks.get(index.containing_block(op.id)?)?;
    if block.operations.last().is_none_or(|last| last.id != op.id) {
        return None;
    }
    match &block.terminator {
        Terminator::Trap { reason } => Some(reason),
        _ => None,
    }
}

fn fault_marker(op: &Operation, reason: &str) -> String {
    let location = op.source.map_or_else(
        || "generated instruction".into(),
        |span| format!("source 0x{:04x}", span.pc),
    );
    format!(
        "fault o{} {} at {location}: {reason} (not completed)",
        op.id.0, op.name
    )
}

pub(super) fn operation(op: &Operation, ir: &Ir, index: &StageIndex) -> String {
    if let Some(reason) = fault_reason(op, ir, index) {
        return format!(
            "{}; raw opcode 0x{:02x}; inputs ({}) outputs ({})",
            fault_marker(op, reason),
            op.opcode,
            values(&op.inputs),
            values(&op.outputs)
        );
    }
    let expression = if let Some(replacement) = op.replacement {
        format!("{replacement:#x}")
    } else if op.opcode == 0x5f || (0x60..=0x7f).contains(&op.opcode) {
        op.outputs
            .first()
            .and_then(|id| index.value(ir, *id))
            .and_then(|value| match value.kind {
                ValueKind::Constant(word) => Some(format!("{word:#x}")),
                _ => None,
            })
            .unwrap_or_else(|| format!("evm.{}()", op.name.to_ascii_lowercase()))
    } else {
        format!(
            "evm.{}({})",
            op.name.to_ascii_lowercase(),
            values(&op.inputs)
        )
    };
    let assignment = if op.outputs.is_empty() {
        expression
    } else {
        format!("{} = {expression}", values(&op.outputs))
    };
    let memory = match (op.memory_input, op.memory_output) {
        (Some(input), Some(output)) => format!(" // m{} → m{}", input.0, output.0),
        (Some(input), None) => format!(" // reads m{}", input.0),
        _ => String::new(),
    };
    let effect = match (op.effect_input, op.effect_output) {
        (Some(input), Some(output)) => format!(" // effect q{} → q{}", input.0, output.0),
        (Some(input), None) => format!(" // effect input q{}", input.0),
        _ => String::new(),
    };
    format!("{assignment};{memory}{effect}")
}

pub(super) fn terminator(term: &Terminator) -> String {
    match term {
        Terminator::Pending => "partial_exit(\"recovery pending\");".into(),
        Terminator::Fallthrough { edge } => format!("follow e{};", edge.0),
        Terminator::Jump {
            target,
            edges,
            unresolved,
            invalid_targets,
        } => format!(
            "jump {} via [{}]{}{};",
            value(*target),
            edges
                .iter()
                .map(|edge| format!("e{}", edge.0))
                .collect::<Vec<_>>()
                .join(", "),
            if *unresolved {
                " + unresolved_target (partial)"
            } else {
                ""
            },
            if invalid_targets.is_empty() {
                ""
            } else {
                " + exceptional_invalid_target"
            }
        ),
        Terminator::Branch {
            condition,
            target,
            then_edges,
            else_edge,
            unresolved,
            invalid_targets,
        } => format!(
            "branch {} != 0: jump {} via [{}]; else {};{}{}",
            value(*condition),
            value(*target),
            then_edges
                .iter()
                .map(|edge| format!("e{}", edge.0))
                .collect::<Vec<_>>()
                .join(", "),
            else_edge.map_or_else(
                || "evm.stop() (end of code)".into(),
                |edge| format!("e{}", edge.0)
            ),
            if *unresolved {
                " unresolved_target (partial);"
            } else {
                ""
            },
            if invalid_targets.is_empty() {
                ""
            } else {
                " exceptional_invalid_target;"
            }
        ),
        Terminator::Exit { kind, values: args } => {
            let name = match kind {
                ExitKind::Stop => "stop",
                ExitKind::Return => "return",
                ExitKind::Revert => "revert",
                ExitKind::SelfDestruct => "selfdestruct",
                ExitKind::EndOfCode => return "evm.stop(); // end of code".into(),
            };
            format!("evm.{name}({});", values(args))
        }
        Terminator::Trap { reason } => format!("exception({reason:?});"),
    }
}
