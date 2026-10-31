//! Readable, selectable presentation of the server's actual structured AST.
//! Formatting does not recover control flow or create transformation history.

use evm_abstract_protocol::decompile_model::{
    EntityId, Ir, Operation, Stage, StructuredKind, StructuredNode, Terminator, ValueKind,
};

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

pub(super) fn lines(stage: &Stage) -> Vec<Line> {
    let mut output = Vec::new();
    if let Some(program) = &stage.ir.structured {
        render(&program.root, &stage.ir, 0, &mut output);
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

fn render(node: &StructuredNode, ir: &Ir, indent: usize, output: &mut Vec<Line>) {
    match &node.kind {
        StructuredKind::Sequence(nodes) => {
            for child in nodes {
                render(child, ir, indent, output);
            }
        }
        StructuredKind::Block { block } => body(node.id, *block, ir, indent, output),
        StructuredKind::Transfer { edge } => transfer(node.id, *edge, ir, indent, output),
        StructuredKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            emit(
                output,
                node.id,
                indent,
                format!("if ({} != 0) {{", value(*condition)),
            );
            render(then_branch, ir, indent + 1, output);
            emit(output, node.id, indent, "} else {");
            render(else_branch, ir, indent + 1, output);
            emit(output, node.id, indent, "}");
        }
        StructuredKind::While {
            header,
            condition,
            condition_nonzero,
            body,
            exit_edge,
        } => {
            emit(output, node.id, indent, "while (true) {");
            render(header, ir, indent + 1, output);
            if let Some(condition) = condition {
                emit(
                    output,
                    node.id,
                    indent + 1,
                    format!(
                        "if ({} {} 0) {{",
                        value(*condition),
                        if *condition_nonzero { "==" } else { "!=" }
                    ),
                );
                if let Some(edge) = exit_edge {
                    transfer(node.id, *edge, ir, indent + 2, output);
                }
                emit(output, node.id, indent + 2, "break;");
                emit(output, node.id, indent + 1, "}");
            }
            render(body, ir, indent + 1, output);
            emit(output, node.id, indent, "}");
        }
        StructuredKind::Break => emit(output, node.id, indent, "break;"),
        StructuredKind::Continue => emit(output, node.id, indent, "continue;"),
        StructuredKind::Exit { block } | StructuredKind::PartialExit { block } => {
            if let Some(block) = ir.block(*block) {
                emit(output, node.id, indent, terminator(&block.terminator));
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
                format!("indirect_jump b{} {{", block.0),
            );
            for arm in arms {
                emit(
                    output,
                    node.id,
                    indent + 1,
                    format!("target 0x{:x}: {{", arm.target_pc),
                );
                render(&arm.body, ir, indent + 2, output);
                emit(output, node.id, indent + 1, "}");
            }
            if *unresolved {
                emit(
                    output,
                    node.id,
                    indent + 1,
                    "unresolved_target: partial_exit;",
                );
            }
            emit(output, node.id, indent, "}");
        }
        StructuredKind::Dispatcher { entry, cases } => {
            emit(
                output,
                node.id,
                indent,
                format!("dispatch_state = b{};", entry.0),
            );
            emit(
                output,
                node.id,
                indent,
                "while (dispatch_state is active) {",
            );
            emit(output, node.id, indent + 1, "switch (dispatch_state) {");
            for case in cases {
                emit(
                    output,
                    node.id,
                    indent + 2,
                    format!("case b{}: {{", case.block.0),
                );
                body(node.id, case.block, ir, indent + 3, output);
                if let Some(block) = ir.block(case.block) {
                    emit(output, block.id, indent + 3, terminator(&block.terminator));
                }
                for transition in &case.transitions {
                    emit(
                        output,
                        transition.id,
                        indent + 3,
                        format!("on e{}: {{", transition.edge.0),
                    );
                    transfer(transition.id, transition.edge, ir, indent + 4, output);
                    emit(
                        output,
                        transition.id,
                        indent + 4,
                        format!("dispatch_state = b{};", transition.target.0),
                    );
                    emit(output, transition.id, indent + 3, "}");
                }
                emit(output, node.id, indent + 2, "}");
            }
            emit(output, node.id, indent + 1, "}");
            emit(output, node.id, indent, "}");
        }
    }
}

fn body(node: EntityId, block: EntityId, ir: &Ir, indent: usize, output: &mut Vec<Line>) {
    let Some(block) = ir.block(block) else {
        return;
    };
    emit(
        output,
        node,
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
    for op in &block.operations {
        emit(output, op.id, indent, operation(op, ir));
    }
}

fn transfer(node: EntityId, edge: EntityId, ir: &Ir, indent: usize, output: &mut Vec<Line>) {
    let Some(edge) = ir.edge(edge) else {
        return;
    };
    let Some(target) = ir.block(edge.to) else {
        return;
    };
    if !target.parameters.is_empty() {
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
    emit(
        output,
        node,
        indent,
        format!(
            "m{} = m{}; // memory token on e{}",
            target.memory_parameter.0, edge.memory.0, edge.id.0
        ),
    );
    emit(
        output,
        node,
        indent,
        format!(
            "q{} = q{}; // effect token on e{}",
            target.effect_parameter.0, edge.effect.0, edge.id.0
        ),
    );
}

pub(super) fn value(id: EntityId) -> String {
    format!("v{}", id.0)
}
pub(super) fn values(ids: &[EntityId]) -> String {
    ids.iter()
        .map(|id| value(*id))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn operation(op: &Operation, ir: &Ir) -> String {
    let expression = if let Some(replacement) = op.replacement {
        format!("{replacement:#x}")
    } else if op.opcode == 0x5f || (0x60..=0x7f).contains(&op.opcode) {
        op.outputs
            .first()
            .and_then(|id| ir.value(*id))
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
            else_edge.map_or_else(|| "end".into(), |edge| format!("e{}", edge.0)),
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
        Terminator::Exit { kind, values: args } => format!(
            "evm.{}({});",
            format!("{kind:?}").to_ascii_lowercase(),
            values(args)
        ),
        Terminator::Trap { reason } => format!("exception({reason:?});"),
    }
}
