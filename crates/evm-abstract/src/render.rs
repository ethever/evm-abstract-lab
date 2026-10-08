//! 面向学习的文本和图输出。所有顺序稳定，便于 diff 和重复实验。

pub mod world;

mod instruction;

use crate::{
    analysis::{Analysis, DiagnosticKind},
    bytecode::Program,
    domain::AbstractValue,
    ssa::Ssa,
};
use instruction::{InstructionLayout, write_ssa_body};
use petgraph::{dot::Dot, graph::DiGraph};
use std::fmt::Write;

fn stack(values: &[AbstractValue]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// 指令地址、PUSH 值和基本块边界。
pub fn disassembly(program: &Program) -> String {
    let mut output = String::new();
    writeln!(output, "fork={}", program.fork()).unwrap();
    for block in program.blocks() {
        let layout = InstructionLayout::block(&mut output, block.id, block.start_pc);
        for instruction in &block.instructions {
            layout.write_pc(&mut output, instruction.pc);
            write!(output, "{:<14}", instruction.name()).unwrap();
            if let Some(value) = instruction.immediate {
                write!(output, " 0x{value:x}").unwrap();
            }
            if !instruction.is_valid() {
                write!(output, " [invalid under {}]", program.fork()).unwrap();
            }
            output.push('\n');
        }
    }
    if program.blocks().is_empty() {
        output.push_str("(empty bytecode: implicit STOP)\n");
    }
    output
}

/// CFG 的状态栈、上下文、边和诊断。栈方向为底到顶。
pub fn cfg(analysis: &Analysis) -> String {
    let mut output = format!(
        "status={:?} fork={} states={} edges={} transfers={} context_depth={}\n",
        analysis.status(),
        analysis.program().fork(),
        analysis.states().len(),
        analysis.edges().len(),
        analysis.transfers(),
        analysis.config().context_depth,
    );
    writeln!(
        output,
        "domain={:?} | domain schema=2 | reduction rounds={} | fact atoms={}",
        analysis.config().domain_profile,
        analysis.config().reduction_rounds,
        analysis.config().max_facts
    )
    .unwrap();
    let relations = analysis.config().relations;
    writeln!(output, "relations={} | SMT=in-process {} | rlimit={} | resource unit={} | expression nodes={} | depth={} | constraints={}",
        relations.enabled, relations.provider, relations.rlimit, relations.provider.resource_unit(), relations.max_nodes, relations.max_depth, relations.max_constraints).unwrap();
    world::environment::write(&mut output, analysis.environment(), None, false, |input| {
        input.to_string()
    });
    for state in analysis.states() {
        let block = &analysis.program().blocks()[state.key.basic_block_index];
        writeln!(
            output,
            "S{} | B{} @ 0x{:04x} | stack height={} | context={:?}",
            state.id, block.id, block.start_pc, state.key.stack_height, state.key.context
        )
        .unwrap();
        writeln!(
            output,
            "  relations in={} out={}",
            state.entry_relations.len(),
            state.exit_relations.as_ref().map_or(0, |state| state.len())
        )
        .unwrap();
        writeln!(
            output,
            "  stack in  {}\n  stack out {}",
            stack(&state.entry_stack),
            stack(&state.exit_stack)
        )
        .unwrap();
        for edge in analysis.edges().iter().filter(|edge| edge.from == state.id) {
            writeln!(output, "  -> S{} {:?}", edge.to, edge.kind).unwrap();
        }
    }
    for diagnostic in analysis.diagnostics() {
        writeln!(
            output,
            "diagnostic S{} @ 0x{:04x}: {:?}",
            diagnostic.state, diagnostic.pc, diagnostic.kind
        )
        .unwrap();
    }
    for frontier in analysis.frontiers() {
        writeln!(
            output,
            "frontier {:?}: from={:?} target={:?} reason={:?}",
            frontier.limit, frontier.from, frontier.target, frontier.reason
        )
        .unwrap();
    }
    output
}

/// petgraph 负责 DOT 的字符串转义和图格式，避免自行拼接图语言。
pub fn dot(analysis: &Analysis) -> String {
    let mut graph = DiGraph::<String, String>::new();
    let nodes: Vec<_> = analysis
        .states()
        .iter()
        .map(|state| {
            let pc = analysis.program().blocks()[state.key.basic_block_index].start_pc;
            graph.add_node(format!(
                "S{} pc=0x{:x}\nstack height={} ctx={:?}\nstack in {}",
                state.id,
                pc,
                state.key.stack_height,
                state.key.context,
                stack(&state.entry_stack)
            ))
        })
        .collect();
    for edge in analysis.edges() {
        graph.add_edge(nodes[edge.from], nodes[edge.to], format!("{:?}", edge.kind));
    }
    // 把状态和前沿放在图注里；图片本身也不能隐藏预算截断。
    let label = format!(
        "fork={}; status={:?}; frontiers={}; unknown jumps={}\n{}",
        analysis.program().fork(),
        analysis.status(),
        analysis.frontiers().len(),
        analysis
            .diagnostics()
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UnknownJump)
            .count(),
        world::environment::summary(analysis.environment(), None),
    );
    let dot = Dot::new(&graph).to_string();
    // 输入与抽象值也进入图注；按字符串转义，节点/边仍由 petgraph 负责。
    dot.replacen(
        "digraph {\n",
        &format!("digraph {{\n    label={label:?};\n    labelloc=\"t\";\n"),
        1,
    )
}

/// 栈 SSA。φ 注明前驱；opcode 参数保持栈顶先弹出的 EVM 顺序。
pub fn ssa(analysis: &Analysis, ssa: &Ssa) -> String {
    let mut output = format!(
        "stack SSA: fork={} values={} status={:?}\n",
        analysis.program().fork(),
        ssa.value_count(),
        analysis.status()
    );
    world::environment::write(&mut output, analysis.environment(), None, false, |input| {
        input.to_string()
    });
    for block in ssa.blocks() {
        let state = &analysis.states()[block.state];
        let source = &analysis.program().blocks()[state.key.basic_block_index];
        writeln!(
            output,
            "S{} | context={:?}:",
            block.state, state.key.context
        )
        .unwrap();
        for phi in &block.phis {
            let inputs = phi
                .inputs
                .iter()
                .map(|i| format!("S{}: %{}", i.predecessor, i.value))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                output,
                "  %{} = phi({inputs}) ; slot {}, abstract {}",
                phi.result, phi.slot, state.entry_stack[phi.slot]
            )
            .unwrap();
        }
        let layout = InstructionLayout::block(&mut output, source.id, source.start_pc);
        for instruction in &block.instructions {
            layout.write_pc(&mut output, instruction.pc);
            write_ssa_body(&mut output, instruction);
            if instruction.fault {
                output.push_str(" ; exceptional halt");
            }
            output.push('\n');
        }
        let values = block
            .exit_stack
            .iter()
            .map(|v| format!("%{v}"))
            .collect::<Vec<_>>()
            .join(", ");
        layout.indent(&mut output);
        writeln!(output, "stack out [{values}]").unwrap();
    }
    output
}
