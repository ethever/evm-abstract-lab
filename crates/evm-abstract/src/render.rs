//! 面向学习的文本和图输出。所有顺序稳定，便于 diff 和重复实验。

pub mod json;
pub mod world;

mod instruction;
mod normalized;

use crate::domain::WordElement;
use evm_abstract_notation::Symbol;

use crate::{
    analysis::{DiagnosticKind, EvmAnalysis, FrameCode},
    bytecode::Program,
    domain::EvmValue,
    ssa::Ssa,
};
use instruction::InstructionLayout;
use petgraph::{dot::Dot, graph::DiGraph};
use std::fmt::Write;

fn diagnostic_kind(kind: &DiagnosticKind) -> String {
    match kind {
        DiagnosticKind::InvalidOpcode(detail) => format!("InvalidOpcode · {detail}"),
        _ => format!("{kind:?}"),
    }
}

fn diagnostic_location(diagnostic: &crate::analysis::Diagnostic) -> String {
    if matches!(
        diagnostic.kind,
        DiagnosticKind::InvalidOpcode(
            crate::analysis::InvalidOpcodeDiagnostic::DelegationEntry { .. }
        )
    ) {
        "delegation entry (synthetic PC 0)".to_owned()
    } else {
        format!("0x{:04x}", diagnostic.pc)
    }
}

const DIAGNOSTIC_RECORDS: &str = "  Records belong to abstract states; repeated PCs do not count distinct instructions or failed transactions.\n";

fn stack<N: WordElement>(values: &[EvmValue<N>]) -> String {
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
pub fn cfg<N: WordElement, D: crate::domain::EvmNumericAlgebra<Element = N>>(
    analysis: &EvmAnalysis<N>,
    algebra: &crate::domain::ValueAlgebra<D>,
) -> String {
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
        "numeric capacity={} | domain schema=3 | reduction rounds={} | fact atoms={}",
        analysis.execution().domain_spec().capacity(),
        analysis.execution().domain_spec().reduction_rounds(),
        analysis.execution().domain_spec().fact_limit()
    )
    .unwrap();
    let relations = analysis.execution().domain_spec().relations();
    writeln!(
        output,
        "relations={} | local propagation | expression nodes={} | depth={} | constraints={}",
        relations.enabled, relations.max_nodes, relations.max_depth, relations.max_constraints
    )
    .unwrap();
    world::environment::write(
        &mut output,
        analysis.environment(),
        None,
        false,
        |input| input.to_string(),
        algebra,
    );
    for state in analysis.states() {
        let block = &analysis.program().blocks()[state.key.basic_block_index];
        writeln!(
            output,
            "{} | native={} | {} @ 0x{:04x} | stack height={} | context={:?}",
            Symbol::ProjectedState(state.id),
            Symbol::State(
                analysis
                    .execution_state_id(state.id)
                    .expect("root projection retains native ID")
            ),
            Symbol::Block(block.id),
            block.start_pc,
            state.key.stack_height,
            state.key.context
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
            writeln!(
                output,
                "  → {} {:?}",
                Symbol::ProjectedState(edge.to),
                edge.kind
            )
            .unwrap();
        }
    }
    if !analysis.boundary_edges().is_empty() {
        output.push_str("\nBoundary transitions (native state and edge IDs)\n");
        for (index, edge) in analysis
            .execution()
            .edges()
            .iter()
            .enumerate()
            .filter(|(_, edge)| analysis.boundary_edges().contains(edge))
        {
            writeln!(
                output,
                "  t{index}: {} → {} {:?}",
                Symbol::State(edge.from),
                Symbol::State(edge.to),
                edge.kind
            )
            .unwrap();
        }
    }
    if !analysis.diagnostics().is_empty() {
        output.push_str(DIAGNOSTIC_RECORDS);
    }
    for diagnostic in analysis.diagnostics() {
        writeln!(
            output,
            "diagnostic {} @ {}: {}",
            Symbol::ProjectedState(diagnostic.state),
            diagnostic_location(diagnostic),
            diagnostic_kind(&diagnostic.kind)
        )
        .unwrap();
    }
    for (index, frontier) in analysis.frontiers().iter().enumerate() {
        writeln!(
            output,
            "frontier {} {:?}: from={} target={} | stack height={} | context={:?} reason={:?}",
            Symbol::Frontier(index),
            frontier.limit,
            frontier.from.map_or_else(
                || "none".to_owned(),
                |state| Symbol::ProjectedState(state).to_string()
            ),
            Symbol::Block(frontier.target.basic_block_index),
            frontier.target.stack_height,
            frontier.target.context,
            frontier.reason
        )
        .unwrap();
    }
    output
}

/// petgraph 负责 DOT 的字符串转义和图格式，避免自行拼接图语言。
pub fn dot<N: WordElement, D: crate::domain::EvmNumericAlgebra<Element = N>>(
    analysis: &EvmAnalysis<N>,
    algebra: &crate::domain::ValueAlgebra<D>,
) -> String {
    let mut graph = DiGraph::<String, String>::new();
    let nodes: Vec<_> = analysis
        .states()
        .iter()
        .map(|state| {
            let pc = analysis.program().blocks()[state.key.basic_block_index].start_pc;
            graph.add_node(format!(
                "{} pc=0x{:x}\nstack height={} ctx={:?}\nstack in {}",
                Symbol::ProjectedState(state.id),
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
    let root_ids = analysis
        .states()
        .iter()
        .filter_map(|state| {
            analysis
                .execution_state_id(state.id)
                .map(|native| (native, state.id))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut external = std::collections::BTreeMap::new();
    for (index, edge) in analysis
        .execution()
        .edges()
        .iter()
        .enumerate()
        .filter(|(_, edge)| analysis.boundary_edges().contains(edge))
    {
        let mut endpoint = |native| {
            if let Some(local) = root_ids.get(&native) {
                return nodes[*local];
            }
            *external.entry(native).or_insert_with(|| {
                graph.add_node(format!(
                    "{} (outside root projection)",
                    Symbol::State(native)
                ))
            })
        };
        let from = endpoint(edge.from);
        let to = endpoint(edge.to);
        graph.add_edge(from, to, format!("t{index} {:?}", edge.kind));
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
        world::environment::summary(analysis.environment(), None, algebra),
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
pub fn ssa<N: WordElement, D: crate::domain::EvmNumericAlgebra<Element = N>>(
    analysis: &EvmAnalysis<N>,
    ssa: &Ssa,
    algebra: &crate::domain::ValueAlgebra<D>,
) -> String {
    let mut output = format!(
        "stack SSA: fork={} values={} status={:?}\n",
        analysis.program().fork(),
        ssa.value_count(),
        analysis.status()
    );
    world::environment::write(
        &mut output,
        analysis.environment(),
        None,
        false,
        |input| input.to_string(),
        algebra,
    );
    output.push_str("\nRoot program state map (local → native)\n");
    for block in ssa.blocks() {
        writeln!(
            output,
            "  {} → {}",
            Symbol::ProjectedState(block.state),
            Symbol::State(block.native_state)
        )
        .unwrap();
    }
    output.push('\n');
    output.push_str(&world::ssa(analysis.execution(), ssa.execution()));
    output
}

/// Native partial SSA for a single-program view, with the real state-ID mapping.
/// Child frames and continuation states need not appear in the projected CFG.
pub fn partial_ssa<N: WordElement>(
    analysis: &EvmAnalysis<N>,
    ir: &crate::ssa::PartialWorldSsa<N>,
) -> String {
    let mut output = world::partial_ssa(analysis.execution(), ir);
    output.push_str("\nSingle-program state mapping\n");
    output.push_str("  The CFG uses projected state IDs; the partial SSA above uses native machine state IDs.\n");
    for state in analysis.states() {
        writeln!(
            output,
            "  projected {} → machine {}",
            Symbol::ProjectedState(state.id),
            Symbol::State(
                analysis
                    .execution_state_id(state.id)
                    .expect("each projected state retains its machine identity")
            )
        )
        .unwrap();
    }
    output
}

/// Render the final native artifact with explicit single-program state mapping.
/// A wholly projected graph retains the single-program labels and block layout.
pub fn normalized_ssa<N: WordElement, D: crate::domain::EvmNumericAlgebra<Element = N>>(
    analysis: &EvmAnalysis<N>,
    ir: &crate::ssa::NormalizedSsa<N>,
    algebra: &crate::domain::ValueAlgebra<D>,
) -> String {
    let projected = ir.complete()
        && analysis.execution().edges().iter().all(|edge| {
            matches!(
                edge.kind,
                crate::analysis::MachineEdgeKind::Intraprocedural(_)
            )
        })
        && analysis.states().len() == ir.blocks().len()
        && analysis.states().iter().all(|state| {
            let machine = &analysis.execution().states()[analysis
                .execution_state_id(state.id)
                .expect("each projected state retains its machine identity")];
            // A projected index can also name native execution with no program.
            // Only a matching captured runtime block supplies a bytecode PC.
            machine.active().mode == FrameCode::Runtime
                && machine
                    .program()
                    .and_then(|program| program.blocks().get(machine.active().basic_block_index))
                    .is_some_and(|source| {
                        analysis.program().blocks().get(state.key.basic_block_index) == Some(source)
                    })
        });
    let mut output = if projected {
        normalized_program(analysis, ir, algebra)
    } else {
        world::normalized_ssa(analysis.execution(), ir)
    };
    output.push_str("\nSingle-program state mapping\n");
    if projected {
        output.push_str("  CFG and SSA block labels use projected state IDs; value definitions come from the native machine SSA.\n");
    } else {
        output.push_str(
            "  The CFG uses projected state IDs; the SSA above uses native machine state IDs.\n",
        );
    }
    for state in analysis.states() {
        writeln!(
            output,
            "  projected {} → machine {}",
            Symbol::ProjectedState(state.id),
            Symbol::State(
                analysis
                    .execution_state_id(state.id)
                    .expect("each projected state retains its machine identity")
            )
        )
        .unwrap();
    }
    output
}

fn normalized_program<N: WordElement, D: crate::domain::EvmNumericAlgebra<Element = N>>(
    analysis: &EvmAnalysis<N>,
    ir: &crate::ssa::NormalizedSsa<N>,
    algebra: &crate::domain::ValueAlgebra<D>,
) -> String {
    let mut output = format!(
        "stack SSA: fork={} values={} status={:?}\n",
        analysis.program().fork(),
        ir.value_count(),
        analysis.status()
    );
    world::environment::write(
        &mut output,
        analysis.environment(),
        None,
        false,
        |input| input.to_string(),
        algebra,
    );
    let projected: std::collections::BTreeMap<_, _> = analysis
        .states()
        .iter()
        .map(|state| {
            (
                analysis
                    .execution_state_id(state.id)
                    .expect("projected machine identity"),
                state.id,
            )
        })
        .collect();
    for state in analysis.states() {
        let machine = analysis
            .execution_state_id(state.id)
            .expect("projected machine identity");
        let block = &ir.blocks()[machine];
        let source = &analysis.program().blocks()[state.key.basic_block_index];
        writeln!(
            output,
            "{} | context={:?}:",
            Symbol::ProjectedState(state.id),
            state.key.context
        )
        .unwrap();
        for phi in &block.phis {
            let inputs = phi
                .inputs
                .iter()
                .map(|(edge, value)| {
                    let from = analysis.execution().edges()[*edge].from;
                    format!("{}: %{value}", Symbol::ProjectedState(projected[&from]))
                })
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                output,
                "  %{} = φ({inputs}) ; slot {}, abstract {}",
                phi.result, phi.slot, state.entry_stack[phi.slot]
            )
            .unwrap();
        }
        let layout = InstructionLayout::block(&mut output, source.id, source.start_pc);
        for row in &block.instructions {
            layout.write_pc(&mut output, row.origin.pc);
            instruction::write_normalized_body(&mut output, row);
            if row.origin.fault {
                output.push_str(" ; exceptional halt");
            }
            output.push('\n');
        }
        let values = block.exit_frames[0]
            .iter()
            .map(|value| format!("%{value}"))
            .collect::<Vec<_>>()
            .join(", ");
        layout.indent(&mut output);
        writeln!(output, "stack out [{values}]").unwrap();
    }
    output
}
