//! Human-readable output is a view of typed syntax, never the semantic owner.
use crate::{
    Artifact, Block, Cancellation, EntityId, Ir, Operation, StructuredKind, StructuredNode,
    ValueKind,
};
use std::fmt::Write;

/// Render the last validated stage, preserving explicit partial/cancel states.
pub fn render_text(artifact: &Artifact) -> String {
    render_text_with_cancel(artifact, &Cancellation::default())
}
/// Cancellable text export; already rendered regions survive cancellation.
pub fn render_text_with_cancel(artifact: &Artifact, cancel: &Cancellation) -> String {
    let mut out = format!(
        "// fork={} strategy={:?} recovery={:?}\n",
        artifact.input.fork, artifact.config.strategy, artifact.completion
    );
    let Some(stage) = artifact.current() else {
        out.push_str("// no validated stage\n");
        return out;
    };
    if let Some(s) = &stage.ir.structured {
        syntax(&mut out, &s.root, &stage.ir, 0, cancel);
    } else {
        for block in &stage.ir.blocks {
            if cancel.is_cancelled() {
                out.push_str("// export cancelled\n");
                break;
            }
            let _ = writeln!(out, "block b{}:", block.id.0);
            body(&mut out, block, &stage.ir, 1);
            let _ = writeln!(out, "  {:?}", block.terminator);
        }
    }
    out
}
fn indent(out: &mut String, depth: usize) {
    out.push_str(&"    ".repeat(depth));
}
fn expression(ir: &Ir, id: EntityId) -> String {
    match ir.value(id).map(|v| &v.kind) {
        Some(ValueKind::Constant(word)) => format!("0x{word:x}"),
        _ => format!("v{}", id.0),
    }
}
fn body(out: &mut String, block: &Block, ir: &Ir, depth: usize) {
    for op in &block.operations {
        if matches!(op.opcode, 0x50 | 0x56 | 0x57 | 0x5b | 0x80..=0x9f) {
            continue;
        }
        indent(out, depth);
        if !op.outputs.is_empty() {
            for (i, id) in op.outputs.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "v{}", id.0);
            }
            out.push_str(" = ");
        }
        if let Some(word) = op.replacement.or_else(|| {
            if (0x5f..=0x7f).contains(&op.opcode) {
                op.outputs
                    .first()
                    .and_then(|id| ir.value(*id))
                    .and_then(|v| v.fact.singleton())
            } else {
                None
            }
        }) {
            let _ = write!(out, "0x{word:x}");
        } else if evm_bytecode::word::arity(op.opcode) == Some(2) && op.inputs.len() == 2 {
            let symbol = match op.opcode {
                0x01 => Some("+"),
                0x02 => Some("*"),
                0x03 => Some("-"),
                0x04 => Some("/"),
                0x06 => Some("%"),
                0x10 => Some("<"),
                0x11 => Some(">"),
                0x14 => Some("=="),
                0x16 => Some("&"),
                0x17 => Some("|"),
                0x18 => Some("^"),
                _ => None,
            };
            if let Some(symbol) = symbol {
                let _ = write!(
                    out,
                    "{} {symbol} {}",
                    expression(ir, op.inputs[0]),
                    expression(ir, op.inputs[1])
                );
            } else {
                primitive_text(out, op, ir);
            }
        } else {
            primitive_text(out, op, ir);
        }
        let _ = writeln!(out, "; // op{}", op.id.0);
    }
}
fn primitive_text(out: &mut String, op: &Operation, ir: &Ir) {
    let _ = write!(
        out,
        "{}({})",
        op.name.to_lowercase(),
        op.inputs
            .iter()
            .map(|id| expression(ir, *id))
            .collect::<Vec<_>>()
            .join(", ")
    );
}
fn transfer(out: &mut String, id: EntityId, ir: &Ir, depth: usize) {
    if let Some(edge) = ir.edge(id) {
        if let Some(target) = ir.block(edge.to) {
            if !target.parameters.is_empty() {
                indent(out, depth);
                let _ = writeln!(
                    out,
                    "({}) = ({});",
                    target
                        .parameters
                        .iter()
                        .map(|id| format!("v{}", id.0))
                        .collect::<Vec<_>>()
                        .join(", "),
                    edge.arguments
                        .iter()
                        .map(|id| expression(ir, *id))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }
}
fn syntax(out: &mut String, n: &StructuredNode, ir: &Ir, depth: usize, cancel: &Cancellation) {
    if cancel.is_cancelled() {
        indent(out, depth);
        out.push_str("// export cancelled\n");
        return;
    }
    match &n.kind {
        StructuredKind::Sequence(v) => {
            for n in v {
                syntax(out, n, ir, depth, cancel);
                if cancel.is_cancelled() {
                    break;
                }
            }
        }
        StructuredKind::Block { block } => {
            if let Some(b) = ir.block(*block) {
                body(out, b, ir, depth);
            }
        }
        StructuredKind::Transfer { edge } => transfer(out, *edge, ir, depth),
        StructuredKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            indent(out, depth);
            let _ = writeln!(out, "if ({} != 0) {{", expression(ir, *condition));
            syntax(out, then_branch, ir, depth + 1, cancel);
            indent(out, depth);
            out.push_str("} else {\n");
            syntax(out, else_branch, ir, depth + 1, cancel);
            indent(out, depth);
            out.push_str("}\n");
        }
        StructuredKind::While {
            header,
            condition,
            condition_nonzero,
            body,
            exit_edge,
        } => {
            indent(out, depth);
            out.push_str("while (true) {\n");
            syntax(out, header, ir, depth + 1, cancel);
            if let Some(condition) = condition {
                indent(out, depth + 1);
                let _ = writeln!(
                    out,
                    "if ({} {} 0) {{",
                    expression(ir, *condition),
                    if *condition_nonzero { "==" } else { "!=" }
                );
                if let Some(edge) = exit_edge {
                    transfer(out, *edge, ir, depth + 2);
                }
                indent(out, depth + 2);
                out.push_str("break;\n");
                indent(out, depth + 1);
                out.push_str("}\n");
            }
            syntax(out, body, ir, depth + 1, cancel);
            indent(out, depth);
            out.push_str("}\n");
        }
        StructuredKind::Break => {
            indent(out, depth);
            out.push_str("break;\n");
        }
        StructuredKind::Continue => {
            indent(out, depth);
            out.push_str("continue;\n");
        }
        StructuredKind::Exit { block } => {
            indent(out, depth);
            if let Some(b) = ir.block(*block) {
                let _ = writeln!(out, "exit {:?};", b.terminator);
            }
        }
        StructuredKind::PartialExit { block } => {
            indent(out, depth);
            let _ = writeln!(out, "partial_indirect_exit(b{});", block.0);
        }
        StructuredKind::Indirect {
            block,
            arms,
            unresolved,
        } => {
            indent(out, depth);
            let _ = writeln!(out, "match jump_target(b{}) {{", block.0);
            for a in arms {
                indent(out, depth + 1);
                let _ = writeln!(out, "0x{:x} => {{", a.target_pc);
                syntax(out, &a.body, ir, depth + 2, cancel);
                indent(out, depth + 1);
                out.push_str("}\n");
            }
            indent(out, depth + 1);
            let _ = writeln!(
                out,
                "_ => {}",
                if *unresolved {
                    "partial_indirect_exit;"
                } else {
                    "invalid_jump;"
                }
            );
            indent(out, depth);
            out.push_str("}\n");
        }
        StructuredKind::Dispatcher { entry, cases } => {
            indent(out, depth);
            let _ = writeln!(out, "state = b{};", entry.0);
            indent(out, depth);
            out.push_str("while (true) {\n");
            indent(out, depth + 1);
            out.push_str("match state {\n");
            for c in cases {
                indent(out, depth + 2);
                let _ = writeln!(out, "b{} => {{", c.block.0);
                if let Some(b) = ir.block(c.block) {
                    body(out, b, ir, depth + 3);
                    indent(out, depth + 3);
                    let _ = writeln!(out, "dispatch {:?};", b.terminator);
                }
                for t in &c.transitions {
                    indent(out, depth + 3);
                    let _ = writeln!(
                        out,
                        "// edge{}: parallel_phi; state = b{}; assignment{}",
                        t.edge.0, t.target.0, t.id.0
                    );
                }
                indent(out, depth + 2);
                out.push_str("}\n");
            }
            indent(out, depth + 1);
            out.push_str("}\n");
            indent(out, depth);
            out.push_str("}\n");
        }
    }
}
