//! Human-readable output is a view of typed syntax, never the semantic owner.
use crate::{
    Artifact, Block, Cancellation, Edge, EntityId, ExitKind, Ir, Operation, StageKind,
    StructuredKind, StructuredNode, Terminator, Value, ValueKind,
};
use evm_resource::{ResourceError, WorkBudget};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// Completion of one text export, independently of bytecode recovery coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextExportStatus {
    /// Every represented region was printed.
    Complete,
    /// The owner cancelled before export completed.
    Cancelled,
    /// The export work allowance refused further output.
    Limited,
}
/// Typed text export result; consumers need not interpret interruption comments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextExport {
    /// Complete or retained partial pseudocode.
    pub text: String,
    /// Export completion, separate from Artifact::completion.
    pub status: TextExportStatus,
    /// Work successfully reserved by this independent export.
    pub work: usize,
}

/// Export with cancellation and the artifact's configured independent allowance.
pub fn export_text(artifact: &Artifact, cancel: &Cancellation) -> TextExport {
    export_text_with_observer(artifact, cancel, artifact.config.limits.max_work, |_| {})
}
/// Render the last validated stage, preserving explicit partial/cancel states.
pub fn render_text(artifact: &Artifact) -> String {
    render_text_with_cancel(artifact, &Cancellation::default())
}
/// Cancellable text export; already rendered regions survive cancellation.
pub fn render_text_with_cancel(artifact: &Artifact, cancel: &Cancellation) -> String {
    render_text_with_observer(artifact, cancel, artifact.config.limits.max_work, |_| {})
}
/// Render with an explicit independent work allowance and monotone paid-work
/// observations. An observer may request cancellation during primitive export.
pub fn render_text_with_observer(
    artifact: &Artifact,
    cancel: &Cancellation,
    max_work: usize,
    observer: impl FnMut(usize),
) -> String {
    export_text_with_observer(artifact, cancel, max_work, observer).text
}
/// Export typed status/work with an explicit allowance and static work observer.
pub fn export_text_with_observer(
    artifact: &Artifact,
    cancel: &Cancellation,
    max_work: usize,
    observer: impl FnMut(usize),
) -> TextExport {
    analysis_control::with_cancellation(cancel, || export_scoped(artifact, max_work, observer))
}

fn export_scoped(artifact: &Artifact, max_work: usize, observer: impl FnMut(usize)) -> TextExport {
    let mut out = format!(
        "// fork={} strategy={:?} recovery={:?}\n",
        artifact.input.fork, artifact.config.strategy, artifact.completion
    );
    let mut index = RenderIndex {
        values: BTreeMap::new(),
        blocks: BTreeMap::new(),
        edges: BTreeMap::new(),
        budget: WorkBudget::new(max_work),
        observer,
        stopped: false,
        status: TextExportStatus::Complete,
        loops: Vec::new(),
        next_block: None,
        source_ir: artifact.current().map(|stage| &stage.ir),
    };
    let stage = artifact.current();
    if stage.is_none() {
        out.push_str("// no validated stage; local recovery unavailable\n");
    }
    diagnostics(&mut out, artifact, &mut index);
    let Some(stage) = stage else {
        return index.finish(out);
    };
    if index.stopped {
        return index.finish(out);
    }
    if !index.pay(crate::transform::ir_size(&stage.ir).saturating_mul(4)) {
        return index.finish(out);
    }
    if stage.kind == StageKind::Decoded {
        source_disassembly(
            &mut out,
            &stage.ir.blocks,
            &artifact.input.runtime_bytecode,
            &mut index,
        );
        return index.finish(out);
    }
    index.values = stage.ir.values.iter().map(|v| (v.id, v)).collect();
    index.blocks = stage.ir.blocks.iter().map(|b| (b.id, b)).collect();
    index.edges = stage.ir.edges.iter().map(|e| (e.id, e)).collect();
    if let Some(s) = &stage.ir.structured {
        syntax(&mut out, &s.root, &mut index, 0);
    } else {
        for block in &stage.ir.blocks {
            if !index.pay(1) {
                break;
            }
            let _ = writeln!(out, "block b{}:", block.id.0);
            body(&mut out, block, &mut index, 1);
            if index.stopped {
                break;
            }
            control(&mut out, block, &mut index, 1, None);
        }
    }
    index.finish(out)
}

fn diagnostics(
    out: &mut String,
    artifact: &Artifact,
    index: &mut RenderIndex<'_, impl FnMut(usize)>,
) {
    for diagnostic in &artifact.diagnostics {
        if !index.pay(diagnostic.message.len().saturating_mul(2).saturating_add(1)) {
            return;
        }
        let _ = write!(out, "// {:?} {:?}", diagnostic.severity, diagnostic.code);
        if let Some(source) = diagnostic.source {
            let _ = write!(out, " at 0x{:04x}", source.pc);
        }
        out.push_str(":\n");
        for line in diagnostic.message.lines() {
            let _ = writeln!(out, "//   {line}");
        }
    }
}

fn source_disassembly(
    out: &mut String,
    blocks: &[Block],
    bytes: &[u8],
    index: &mut RenderIndex<'_, impl FnMut(usize)>,
) {
    // Decoded definitions are stable source placeholders. Their missing inputs
    // and Bottom facts must never be presented as recovered SSA computations.
    out.push_str("// decoded source only; stack/value recovery unavailable\ndisassembly:\n");
    for block in blocks {
        for op in &block.operations {
            let raw = op
                .source
                .and_then(|span| bytes.get(span.pc..span.end_pc))
                .unwrap_or_default();
            if !index.pay(
                raw.len()
                    .saturating_mul(4)
                    .saturating_add(op.name.len())
                    .saturating_add(1),
            ) {
                return;
            }
            if let Some(source) = op.source {
                let _ = write!(out, "0x{:04x}: ", source.pc);
            } else {
                out.push_str("<unlocated>: ");
            }
            out.push_str(&op.name);
            if (0x60..=0x7f).contains(&op.opcode) && raw.len() > 1 {
                out.push_str(" 0x");
                for byte in &raw[1..] {
                    let _ = write!(out, "{byte:02x}");
                }
            }
            out.push_str(" // bytes ");
            for byte in raw {
                let _ = write!(out, "{byte:02x}");
            }
            let _ = writeln!(out, "; unrecovered; op{}", op.id.0);
        }
    }
}
// One export owns its own work ledger. Indexed lookups keep a large recovered
// graph from turning each rendered operand into another whole-value scan.
struct RenderIndex<'a, P: FnMut(usize)> {
    values: BTreeMap<EntityId, &'a Value>,
    blocks: BTreeMap<EntityId, &'a Block>,
    edges: BTreeMap<EntityId, &'a Edge>,
    budget: WorkBudget,
    observer: P,
    stopped: bool,
    status: TextExportStatus,
    loops: Vec<LoopControl>,
    next_block: Option<EntityId>,
    source_ir: Option<&'a Ir>,
}
#[derive(Clone, Copy)]
struct LoopControl {
    node: EntityId,
    entry: Option<EntityId>,
}
struct DispatchControl {
    node: EntityId,
    cases: BTreeSet<EntityId>,
    continuation: Option<EntityId>,
    outer: Option<LoopControl>,
}
impl<'a, P: FnMut(usize)> RenderIndex<'a, P> {
    fn pay(&mut self, work: usize) -> bool {
        if self.stopped {
            return false;
        }
        if let Err(error) = evm_resource::pay(&mut self.budget, work) {
            self.stopped = true;
            self.status = match error {
                ResourceError::Cancelled => TextExportStatus::Cancelled,
                ResourceError::Work => TextExportStatus::Limited,
            };
            return false;
        }
        (self.observer)(self.budget.used());
        true
    }
    fn finish(mut self, mut text: String) -> TextExport {
        if self.status == TextExportStatus::Complete
            && let Err(error) = evm_resource::checkpoint(&self.budget)
        {
            self.status = match error {
                ResourceError::Cancelled => TextExportStatus::Cancelled,
                ResourceError::Work => TextExportStatus::Limited,
            };
        }
        if self.status != TextExportStatus::Complete {
            text.push_str("// export cancelled or work limit exhausted\n");
        }
        TextExport {
            text,
            status: self.status,
            work: self.budget.used(),
        }
    }
    fn value(&self, id: EntityId) -> Option<&'a Value> {
        self.values.get(&id).copied()
    }
    fn block(&self, id: EntityId) -> Option<&'a Block> {
        self.blocks.get(&id).copied()
    }
    fn edge(&self, id: EntityId) -> Option<&'a Edge> {
        self.edges.get(&id).copied()
    }
    fn first_block(&self, node: &StructuredNode) -> Option<EntityId> {
        self.source_ir
            .and_then(|ir| crate::execute::first_block(node, ir))
    }
}
fn indent(out: &mut String, depth: usize) {
    out.push_str(&"    ".repeat(depth));
}
fn expression(ir: &RenderIndex<'_, impl FnMut(usize)>, id: EntityId) -> String {
    match ir.value(id).map(|v| &v.kind) {
        Some(ValueKind::Constant(word)) => format!("0x{word:x}"),
        _ => format!("v{}", id.0),
    }
}
fn body(
    out: &mut String,
    block: &Block,
    ir: &mut RenderIndex<'_, impl FnMut(usize)>,
    depth: usize,
) {
    for (position, op) in block.operations.iter().enumerate() {
        if !ir.pay(1 + op.inputs.len() + op.outputs.len()) {
            return;
        }
        if position + 1 == block.operations.len()
            && matches!(block.terminator, Terminator::Trap { .. })
        {
            break;
        }
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
                    .and_then(|v| match v.kind {
                        ValueKind::Constant(word) => Some(word),
                        _ => v.fact.singleton(),
                    })
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
fn primitive_text(out: &mut String, op: &Operation, ir: &RenderIndex<'_, impl FnMut(usize)>) {
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
fn transfer(
    out: &mut String,
    id: EntityId,
    ir: &mut RenderIndex<'_, impl FnMut(usize)>,
    depth: usize,
) {
    if let Some(edge) = ir.edge(id)
        && let Some(target) = ir.block(edge.to)
        && !target.parameters.is_empty()
    {
        if !ir.pay(edge.arguments.len().saturating_add(1)) {
            return;
        }
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
fn syntax(
    out: &mut String,
    n: &StructuredNode,
    ir: &mut RenderIndex<'_, impl FnMut(usize)>,
    depth: usize,
) {
    if !ir.pay(1) {
        return;
    }
    match &n.kind {
        StructuredKind::Sequence(v) => {
            let outer_next = ir.next_block;
            for (position, n) in v.iter().enumerate() {
                ir.next_block = v
                    .get(position + 1)
                    .map(|next| ir.first_block(next))
                    .unwrap_or(outer_next);
                syntax(out, n, ir, depth);
                if ir.stopped {
                    break;
                }
            }
            ir.next_block = outer_next;
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
            syntax(out, then_branch, ir, depth + 1);
            if visible(else_branch, ir) {
                indent(out, depth);
                out.push_str("} else {\n");
                syntax(out, else_branch, ir, depth + 1);
            }
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
            let _ = writeln!(out, "'loop{}: while (true) {{", n.id.0);
            syntax(out, header, ir, depth + 1);
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
            let outer_next = ir.next_block.take();
            let entry = ir.first_block(header).or_else(|| ir.first_block(body));
            ir.loops.push(LoopControl { node: n.id, entry });
            syntax(out, body, ir, depth + 1);
            ir.loops.pop();
            ir.next_block = outer_next;
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
            if let Some(b) = ir.block(*block) {
                control(out, b, ir, depth, None);
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
                if !ir.pay(1) {
                    break;
                }
                indent(out, depth + 1);
                let _ = writeln!(out, "0x{:x} => {{", a.target_pc);
                syntax(out, &a.body, ir, depth + 2);
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
            if !ir.pay(cases.len().saturating_mul(2)) {
                return;
            }
            let dispatch = DispatchControl {
                node: n.id,
                cases: cases.iter().map(|case| case.block).collect(),
                continuation: ir.next_block,
                outer: ir.loops.last().copied(),
            };
            indent(out, depth);
            let _ = writeln!(out, "state_d{} = b{};", n.id.0, entry.0);
            indent(out, depth);
            let _ = writeln!(out, "'dispatch{}: while (true) {{", n.id.0);
            indent(out, depth + 1);
            let _ = writeln!(out, "match state_d{} {{", n.id.0);
            for c in cases {
                if !ir.pay(1 + c.transitions.len()) {
                    break;
                }
                indent(out, depth + 2);
                let _ = writeln!(out, "b{} => {{", c.block.0);
                if let Some(b) = ir.block(c.block) {
                    body(out, b, ir, depth + 3);
                    control(out, b, ir, depth + 3, Some(&dispatch));
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

fn visible(node: &StructuredNode, ir: &RenderIndex<'_, impl FnMut(usize)>) -> bool {
    match &node.kind {
        StructuredKind::Sequence(nodes) => nodes.iter().any(|n| visible(n, ir)),
        StructuredKind::Transfer { edge } => ir
            .edge(*edge)
            .and_then(|e| ir.block(e.to))
            .is_some_and(|b| !b.parameters.is_empty()),
        _ => true,
    }
}

fn edge_control(
    out: &mut String,
    id: EntityId,
    ir: &mut RenderIndex<'_, impl FnMut(usize)>,
    depth: usize,
    dispatch: Option<&DispatchControl>,
) {
    transfer(out, id, ir, depth);
    if let Some(edge) = ir.edge(id) {
        indent(out, depth);
        if let Some(dispatch) = dispatch {
            // An external target yields Flow::Escaped in the AST runner. Keep
            // its phi transfer, then resume a following region or natural loop.
            let action = if dispatch.cases.contains(&edge.to) {
                format!("continue 'dispatch{};", dispatch.node.0)
            } else if Some(edge.to) == dispatch.continuation {
                format!("break 'dispatch{};", dispatch.node.0)
            } else if let Some(outer) = dispatch.outer {
                format!(
                    "{} 'loop{};",
                    if Some(edge.to) == outer.entry {
                        "continue"
                    } else {
                        "break"
                    },
                    outer.node.0
                )
            } else {
                format!("break 'dispatch{};", dispatch.node.0)
            };
            let _ = writeln!(
                out,
                "state_d{} = b{}; {action} // edge{}",
                dispatch.node.0, edge.to.0, id.0
            );
        } else {
            let _ = writeln!(out, "goto b{}; // edge{}", edge.to.0, id.0);
        }
    }
}

fn jump_control(
    out: &mut String,
    target: EntityId,
    jump: (&[EntityId], bool),
    ir: &mut RenderIndex<'_, impl FnMut(usize)>,
    depth: usize,
    dispatch: Option<&DispatchControl>,
) {
    let (edges, unresolved) = jump;
    indent(out, depth);
    let value = expression(ir, target);
    if edges.is_empty() {
        let _ = writeln!(
            out,
            "{}({value});",
            if unresolved {
                "partial_indirect_exit"
            } else {
                "invalid_jump"
            }
        );
        return;
    }
    let _ = writeln!(out, "match {value} {{");
    for id in edges {
        if !ir.pay(1) {
            return;
        }
        if let Some(edge) = ir.edge(*id)
            && let Some(span) = ir.block(edge.to).and_then(|b| b.source)
        {
            indent(out, depth + 1);
            let _ = writeln!(out, "0x{:x} => {{", span.pc);
            edge_control(out, *id, ir, depth + 2, dispatch);
            indent(out, depth + 1);
            out.push_str("}\n");
        }
    }
    indent(out, depth + 1);
    let _ = writeln!(
        out,
        "_ => {}({value});",
        if unresolved {
            "partial_indirect_exit"
        } else {
            "invalid_jump"
        }
    );
    indent(out, depth);
    out.push_str("}\n");
}

fn control(
    out: &mut String,
    block: &Block,
    ir: &mut RenderIndex<'_, impl FnMut(usize)>,
    depth: usize,
    dispatch: Option<&DispatchControl>,
) {
    if !ir.pay(1) {
        return;
    }
    match &block.terminator {
        Terminator::Exit { kind, values } => {
            // The terminal operation body already prints its EVM primitive.
            if block
                .operations
                .last()
                .is_some_and(|op| matches!(op.opcode, 0x00 | 0xf3 | 0xfd | 0xff))
            {
                return;
            }
            indent(out, depth);
            if *kind == ExitKind::EndOfCode {
                out.push_str("stop(); // end of runtime\n");
            } else {
                let primitive = match kind {
                    ExitKind::Stop => "stop",
                    ExitKind::Return => "return",
                    ExitKind::Revert => "revert",
                    ExitKind::SelfDestruct => "selfdestruct",
                    ExitKind::EndOfCode => unreachable!(),
                };
                let args = values
                    .iter()
                    .map(|v| expression(ir, *v))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(out, "{primitive}({args});");
            }
        }
        Terminator::Trap { reason } => {
            indent(out, depth);
            let _ = write!(out, "trap({reason:?});");
            if let Some(op) = block.operations.last() {
                let _ = write!(out, " // fault op{}", op.id.0);
            }
            out.push('\n');
        }
        Terminator::Pending => {
            indent(out, depth);
            let _ = writeln!(out, "partial_recovery(b{});", block.id.0);
        }
        Terminator::Fallthrough { edge } => edge_control(out, *edge, ir, depth, dispatch),
        Terminator::Jump {
            target,
            edges,
            unresolved,
            ..
        } => jump_control(out, *target, (edges, *unresolved), ir, depth, dispatch),
        Terminator::Branch {
            condition,
            target,
            then_edges,
            else_edge,
            unresolved,
            ..
        } => {
            indent(out, depth);
            let _ = writeln!(out, "if ({} != 0) {{", expression(ir, *condition));
            jump_control(
                out,
                *target,
                (then_edges, *unresolved),
                ir,
                depth + 1,
                dispatch,
            );
            indent(out, depth);
            out.push_str("} else {\n");
            if let Some(edge) = else_edge {
                edge_control(out, *edge, ir, depth + 1, dispatch);
            } else {
                indent(out, depth + 1);
                out.push_str("stop(); // end of runtime\n");
            }
            indent(out, depth);
            out.push_str("}\n");
        }
    }
}
