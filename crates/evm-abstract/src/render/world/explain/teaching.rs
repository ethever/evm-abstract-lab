//! 默认教学视图先展示代码、状态图和结果，再给出可验证的调用 SSA。
//!
//! 目录使用帧实际捕获的程序；简略效果只用于阅读，不能作为结果去重依据。

use crate::domain::WordElement;
use evm_abstract_notation::Symbol;

use super::super::teaching::References;
use crate::{
    analysis::{EvmMachinePayload, EvmMachineState, EvmWorldAnalysis, FrameCode, FrameKey, Status},
    bytecode::Program,
    domain::EvmValue,
    ssa::{self, SsaError},
    world::{Code, EvmByteArray},
};
use alloy_primitives::{Address, B256, U256, hex};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

type CodeIdentity = (Address, B256, FrameCode);

#[derive(Default)]
struct CapturedCode<'a> {
    program: Option<&'a Program>,
    owners: BTreeSet<Address>,
}

/// 未完成的图仍保留捕获代码、结果和每个前沿；SSA 仅由收敛图构建。
pub(in crate::render::world) fn render<
    N: WordElement,
    D: crate::domain::EvmNumericAlgebra<Element = N>,
>(
    analysis: &EvmWorldAnalysis<N>,
    algebra: &crate::domain::ValueAlgebra<D>,
    budget: &mut crate::resource::WorkBudget,
) -> Result<String, SsaError> {
    render_with_mode(analysis, false, algebra, budget)
}

pub(in crate::render::world) fn render_partial<
    N: WordElement,
    D: crate::domain::EvmNumericAlgebra<Element = N>,
>(
    analysis: &EvmWorldAnalysis<N>,
    algebra: &crate::domain::ValueAlgebra<D>,
    budget: &mut crate::resource::WorkBudget,
) -> Result<String, SsaError> {
    render_with_mode(analysis, true, algebra, budget)
}

fn render_with_mode<N: WordElement, D: crate::domain::EvmNumericAlgebra<Element = N>>(
    analysis: &EvmWorldAnalysis<N>,
    allow_partial_ssa: bool,
    algebra: &crate::domain::ValueAlgebra<D>,
    budget: &mut crate::resource::WorkBudget,
) -> Result<String, SsaError> {
    crate::ssa::work::explanation(analysis, budget)?;
    // SSA admission/replay precedes report allocation. A rejected structural
    // operation cannot first build a large code/reference report.
    let normalized = if analysis.status() == Status::Converged {
        Some(ssa::build_normalized_world(analysis, budget)?)
    } else if allow_partial_ssa {
        Some(ssa::build_normalized_partial_world(analysis, budget)?)
    } else {
        None
    };
    let refs = References::new(analysis);
    let mut output = String::from("Cross-contract explanation\n");
    writeln!(
        output,
        "  status={:?} | fork={} | numeric capacity={} | states={} | edges={} | outcomes={}",
        analysis.status(),
        analysis.world().fork(),
        analysis.domain_spec().capacity(),
        analysis.states().len(),
        analysis.edges().len(),
        analysis.outcomes().len(),
    )
    .unwrap();
    writeln!(
        output,
        "  transfers={} | work={}/{} | context depth={} | reduction rounds={} | fact atoms={}",
        analysis.transfers(),
        analysis.work(),
        analysis.config().max_work,
        analysis.config().analysis.context_depth,
        analysis.domain_spec().reduction_rounds(),
        analysis.domain_spec().fact_limit(),
    )
    .unwrap();
    writeln!(
        output,
        "  snapshot={} | source={:?}",
        super::super::identity(analysis.world().identity()),
        analysis.world().provenance(),
    )
    .unwrap();
    super::super::environment::write(
        &mut output,
        &analysis.entry().environment,
        Some(analysis.world().identity()),
        false,
        |input| refs.address_input(input),
        algebra,
    );
    output
        .push_str("  Context-sensitive abstract state graph; not a concrete instruction trace.\n");
    output.push_str("  σᵢ = state; Bᵢ = frame-local block; fᵢ = frame within a state; Cᵢ = captured code; Uᵢ = unresolved frontier; stacks are bottom-to-top.\n");
    write_rpc(&mut output, analysis, &refs);
    let relations = analysis.domain_spec().relations();
    writeln!(
        output,
        "  relations={} | local propagation | expression nodes={} | depth={} | constraints={}",
        relations.enabled, relations.max_nodes, relations.max_depth, relations.max_constraints
    )
    .unwrap();
    write_addresses(&mut output, &refs);
    write_code(&mut output, analysis, &refs);
    write_cfg(&mut output, analysis, &refs);
    write_outcomes(&mut output, analysis, &refs, algebra);
    write_diagnostics(&mut output, analysis);
    write_frontiers(&mut output, analysis, &refs);
    super::super::text::procedure_summaries::write(&mut output, analysis);
    output.push('\n');
    if let Some(ir) = &normalized {
        output.push_str(&super::super::normalized_ssa(analysis, ir));
    } else {
        output.push_str("SSA unavailable: cross-contract frontiers remain\n");
    }
    crate::ssa::pay(budget, 0)?;
    Ok(output)
}

fn address(refs: &References, value: Address) -> String {
    refs.address(value)
        .map_or_else(|| value.to_string(), str::to_owned)
}

fn write_addresses(output: &mut String, refs: &References) {
    output.push_str("\nAddresses\n");
    for (value, reference) in refs.addresses() {
        writeln!(output, "  {reference} = {value}").unwrap();
    }
}

fn write_rpc<N: WordElement>(
    output: &mut String,
    analysis: &EvmWorldAnalysis<N>,
    refs: &References,
) {
    let Some(acquisition) = analysis.rpc_acquisition() else {
        return;
    };
    writeln!(
        output,
        "  RPC acquisition: rounds={} | requests={} | fetched={} | failed={} | fetched slots={} | failed slots={} | states created={}",
        acquisition.rounds,
        acquisition.requests,
        acquisition.fetched_accounts.len(),
        acquisition.failed_accounts.len(),
        acquisition.fetched_storage.len(),
        acquisition.failed_storage.len(),
        acquisition.states_created,
    )
    .unwrap();
    output.push_str("  RPC policy: trusted source, fixed canonical block hash; concrete/finite missing storage reads are acquired on demand; open keys remain abstract.\n");
    // 后续预算可能阻止再次到达失败调用，累计获取失败仍要显示。
    for failed in &acquisition.failures {
        writeln!(
            output,
            "  RPC failure {} slot={:?}: {:?} resource={:?} limit={:?} | {} | {}",
            address(refs, failed.address),
            failed.slot,
            failed.failure.kind,
            failed.failure.resource,
            failed.failure.limit,
            failed.failure.context,
            failed.failure.message,
        )
        .unwrap();
    }
}

fn capture<'a, N: WordElement>(
    codes: &mut BTreeMap<CodeIdentity, CapturedCode<'a>>,
    payload: &'a EvmMachinePayload<N>,
) {
    for frame in payload.call_stack.iter() {
        let code = codes
            .entry((frame.key.code_address, frame.key.code_hash, frame.code))
            .or_default();
        if code.program.is_none() {
            code.program = frame.program.as_ref();
        }
        code.owners.insert(frame.key.address);
    }
}

fn write_code<N: WordElement>(
    output: &mut String,
    analysis: &EvmWorldAnalysis<N>,
    refs: &References,
) {
    let mut codes = BTreeMap::<CodeIdentity, CapturedCode<'_>>::new();
    for state in analysis.states() {
        capture(&mut codes, &state.entry);
        if let Some(exit) = &state.exit {
            capture(&mut codes, exit);
        }
    }
    output.push_str("\nExecution code\n");
    output.push_str("  Instruction lists are syntactic and can include unvisited instructions.\n");
    if codes.is_empty() {
        output.push_str(
            "  No executable frame was captured; see Frontiers for the unresolved entry.\n",
        );
    }
    for ((code_address, hash, mode), reference) in refs.codes() {
        let code = &codes[&(*code_address, *hash, *mode)];
        writeln!(
            output,
            "\n  {reference} | code={} | code_hash={hash} | mode={}",
            address(refs, *code_address),
            mode_label(refs, *mode),
        )
        .unwrap();
        writeln!(
            output,
            "    state owners: {}",
            code.owners
                .iter()
                .map(|owner| address(refs, *owner))
                .collect::<Vec<_>>()
                .join(", "),
        )
        .unwrap();
        match (mode, code.program) {
            (FrameCode::Runtime | FrameCode::InitCode, Some(program)) => {
                writeln!(
                    output,
                    "    Captured instruction list ({} bytes):",
                    program.byte_len(),
                )
                .unwrap();
                output.push_str(&crate::render::disassembly(program));
            }
            (FrameCode::Precompile(native), _) => {
                writeln!(
                    output,
                    "    Native precompile {}; no bytecode instruction list.",
                    address(refs, *native),
                )
                .unwrap();
            }
            (FrameCode::Empty, _) => output.push_str(
                "    Empty executable code: implicit successful halt; no bytecode instruction list.\n",
            ),
            (FrameCode::Unknown, _) => output.push_str(
                "    Abstract environment: interactions represented by summary effects; no captured bytecode instruction list.\n",
            ),
            (FrameCode::InvalidDelegation, _) => output.push_str(
                "    Invalid nested delegation: exceptional halt at the second marker; no bytecode instruction list.\n",
            ),
            (_, None) => output.push_str(
                "    No captured bytecode program; instruction list unavailable.\n",
            ),
        }
    }
    if codes.is_empty() {
        write_input_code(output, analysis, refs);
    }
}

fn write_input_code<N: WordElement>(
    output: &mut String,
    analysis: &EvmWorldAnalysis<N>,
    refs: &References,
) {
    // 初始化预算耗尽时，初始快照的语法代码仍可学习，但不能当作执行证据。
    output.push_str("\nInput code observations (not execution evidence)\n");
    output.push_str(
        "  Initial snapshot facts only; these instruction lists do not establish execution.\n",
    );
    if analysis.world().accounts().is_empty() {
        output.push_str("  No input account code observations.\n");
    }
    for (owner, account) in analysis.world().accounts() {
        writeln!(
            output,
            "  account={} | observed_code_hash={}",
            address(refs, *owner),
            super::super::hash_label(analysis.world().code_hash(*owner)),
        )
        .unwrap();
        match &account.code {
            Code::Runtime(program) => {
                output.push_str("    Observed instruction list (execution not established):\n");
                output.push_str(&crate::render::disassembly(program));
            }
            Code::Delegation(target) => {
                writeln!(
                    output,
                    "    Delegation indicator targeting {}; no runtime instruction list captured.",
                    address(refs, *target),
                )
                .unwrap();
            }
            Code::Empty => {
                output.push_str("    Observed empty code; no bytecode instruction list.\n")
            }
            Code::Unknown => output.push_str("    Code unknown; no bytecode instruction list.\n"),
        }
    }
}

fn location<N: WordElement>(state: &EvmMachineState<N>) -> String {
    let frame = state.active();
    if let Some(block) = state
        .program()
        .and_then(|program| program.blocks().get(frame.basic_block_index))
    {
        return format!("{} @ 0x{:04x}", Symbol::Block(block.id), block.start_pc);
    }
    match frame.mode {
        FrameCode::Runtime | FrameCode::InitCode => {
            "synthetic end-of-code continuation (no instruction)".to_owned()
        }
        FrameCode::Precompile(_) => "native precompile (no bytecode block)".to_owned(),
        FrameCode::Empty => "empty code (implicit halt)".to_owned(),
        FrameCode::Unknown => "abstract environment (summary effects)".to_owned(),
        FrameCode::InvalidDelegation => "invalid nested delegation (exceptional halt)".to_owned(),
    }
}

fn write_cfg<N: WordElement>(
    output: &mut String,
    analysis: &EvmWorldAnalysis<N>,
    refs: &References,
) {
    output.push_str("\nCFG\n");
    output.push_str("  pcs records the last abstract block transfer, not a transaction trace.\n");
    if analysis.states().is_empty() {
        output.push_str("  (no captured states)\n");
    }
    for state in analysis.states() {
        let frame = state.entry.active();
        write!(
            output,
            "  {} | {} | {} active | code={}",
            Symbol::State(state.id),
            location(state),
            Symbol::Frame(state.entry.call_stack.depth() - 1),
            refs.code(&frame.key)
                .expect("captured frame has code reference")
        )
        .unwrap();
        if frame.key.address != frame.key.code_address {
            write!(
                output,
                " | state owner={}",
                refs.address_input(frame.key.address_value)
            )
            .unwrap();
        }
        writeln!(
            output,
            " | caller={} | static={} | context={:?}",
            refs.address_input(frame.key.caller),
            frame.key.is_static,
            frame.key.jump_history,
        )
        .unwrap();
        writeln!(
            output,
            "    stack in  {}\n    stack out {}",
            crate::render::stack(&frame.stack),
            crate::render::stack(&state.exit_stack),
        )
        .unwrap();
        let pcs = state
            .executed_pcs
            .iter()
            .map(|pc| format!("0x{pc:04x}"))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(output, "    pcs=[{pcs}]").unwrap();
        for edge in analysis.edges().iter().filter(|edge| edge.from == state.id) {
            writeln!(output, "    → {} {:?}", Symbol::State(edge.to), edge.kind).unwrap();
        }
    }
}

fn length<N: WordElement>(value: &EvmValue<N>) -> String {
    if let Some(exact) = value.singleton() {
        return format!("{value} ({exact} bytes)");
    }
    match value.constants() {
        Some(values) if values.len() == 1 => format!(
            "{value} ({} bytes)",
            values.first().expect("nonempty constants"),
        ),
        Some(_) => format!("{value} (possible byte counts)"),
        None => format!("{value} (unknown byte count)"),
    }
}

fn data_summary<N: WordElement, D: crate::domain::EvmNumericAlgebra<Element = N>>(
    data: &EvmByteArray<N>,
    algebra: &crate::domain::ValueAlgebra<D>,
) -> String {
    if let Some(bytes) = data.exact_bytes_bounded(32, algebra.borrowed()) {
        if bytes.len() == 32 {
            return format!("word=0x{:x}", U256::from_be_slice(&bytes));
        }
        return format!("exact hex=0x{}", hex::encode(bytes));
    }
    format!(
        "abstract or long bytes | default byte={} | full byte facts: --verbose",
        data.default_byte(),
    )
}

fn write_outcomes<N: WordElement, D: crate::domain::EvmNumericAlgebra<Element = N>>(
    output: &mut String,
    analysis: &EvmWorldAnalysis<N>,
    refs: &References,
    algebra: &crate::domain::ValueAlgebra<D>,
) {
    output.push_str("\nOutcomes\n");
    output.push_str(
        "  Each Oᵢ keeps its own state and effects; equal returndata does not merge outcomes.\n",
    );
    output.push_str("  Selected effect facts below; complete storage, balances, lifecycle, logs and byte facts: --verbose or analyze.\n");
    if analysis.outcomes().is_empty() {
        output.push_str("  (none)\n");
    }
    for (index, outcome) in analysis.outcomes().iter().enumerate() {
        writeln!(
            output,
            "  {} | {} | {:?} | returndata length={} | {}",
            Symbol::Outcome(index),
            Symbol::State(outcome.state),
            outcome.kind,
            length(outcome.data.len()),
            data_summary(&outcome.data, algebra)
        )
        .unwrap();
        let retained = outcome.store.slots().iter().collect::<Vec<_>>();
        write!(output, "    abstract explicit storage facts:").unwrap();
        for ((owner, slot), value) in retained.iter().take(4) {
            write!(output, " {}[{slot:#x}]={value}", address(refs, *owner)).unwrap();
        }
        if retained.is_empty() {
            output.push_str(" (none)");
        } else if retained.len() > 4 {
            write!(output, " | {} more in --verbose", retained.len() - 4).unwrap();
        }
        output.push('\n');
        let balances = outcome
            .store
            .addresses()
            .into_iter()
            .map(|owner| (owner, outcome.store.read_balance(owner)))
            .collect::<Vec<_>>();
        if !balances.is_empty() {
            output.push_str("    retained balances:");
            for (owner, balance) in balances.iter().take(4) {
                write!(output, " {}={balance}", address(refs, *owner)).unwrap();
            }
            if balances.len() > 4 {
                write!(output, " | {} more in --verbose", balances.len() - 4).unwrap();
            }
            output.push('\n');
        }
        if !outcome.store.possible_logs().is_empty() || outcome.store.logs_unknown() {
            writeln!(
                output,
                "    possible log sites={} | logs_unknown={} | full log facts: --verbose",
                outcome.store.possible_logs().len(),
                outcome.store.logs_unknown(),
            )
            .unwrap();
        }
    }
    let stats = analysis.summary_stats();
    writeln!(
        output,
        "  Call summaries: hits={} | misses={} | published={} | rejected incomplete={} | imported states={}",
        stats.hits,
        stats.misses,
        stats.published,
        stats.rejected_incomplete,
        stats.imported_states,
    )
    .unwrap();
}

fn write_diagnostics<N: WordElement>(output: &mut String, analysis: &EvmWorldAnalysis<N>) {
    output.push_str("\nDiagnostics\n");
    if analysis.diagnostics().is_empty() {
        output.push_str("  (none)\n");
    } else {
        output.push_str(crate::render::DIAGNOSTIC_RECORDS);
    }
    for diagnostic in analysis.diagnostics() {
        writeln!(
            output,
            "  {} @ {}: {}",
            Symbol::State(diagnostic.state),
            crate::render::diagnostic_location(diagnostic),
            crate::render::diagnostic_kind(&diagnostic.kind)
        )
        .unwrap();
    }
}

fn write_frontiers<N: WordElement>(
    output: &mut String,
    analysis: &EvmWorldAnalysis<N>,
    refs: &References,
) {
    output.push_str("\nFrontiers\n");
    if analysis.frontiers().is_empty() {
        output.push_str("  (none)\n");
    }
    for (index, frontier) in analysis.frontiers().iter().enumerate() {
        writeln!(
            output,
            "  {} | from={} | pc={} | reason={:?}",
            Symbol::Frontier(index),
            frontier.from.map_or_else(
                || "none".to_owned(),
                |state| format!(
                    "{symbol_state_state}",
                    symbol_state_state = Symbol::State(state)
                )
            ),
            frontier
                .pc
                .map_or_else(|| "none".to_owned(), |pc| format!("0x{pc:04x}")),
            frontier.reason
        )
        .unwrap();
        if let Some(target) = &frontier.target {
            output.push_str("    target frames (oldest caller first):\n");
            for (frame, key) in target.frames.iter().enumerate() {
                write_frontier_frame(output, refs, frame, key);
            }
        } else {
            output.push_str("    target=none\n");
        }
    }
}

fn write_frontier_frame(output: &mut String, refs: &References, frame: usize, key: &FrameKey) {
    write!(
        output,
        "      {} | {} | ",
        Symbol::Frame(frame),
        Symbol::Block(key.basic_block_index)
    )
    .unwrap();
    if let Some(code) = refs.code(key) {
        write!(output, "code={code}").unwrap();
    } else {
        write!(
            output,
            "uncaptured code={} | code_hash={} | mode={}",
            address(refs, key.code_address),
            key.code_hash,
            mode_label(refs, key.mode),
        )
        .unwrap();
    }
    writeln!(
        output,
        " | state owner={} | caller={} | static={} | stack height={} | context={:?}",
        refs.address_input(key.address_value),
        refs.address_input(key.caller),
        key.is_static,
        key.stack_height,
        key.jump_history,
    )
    .unwrap();
}

fn mode_label(refs: &References, mode: FrameCode) -> String {
    match mode {
        FrameCode::Precompile(native) => format!("Precompile({})", address(refs, native)),
        mode => format!("{mode:?}"),
    }
}

#[cfg(test)]
mod tests;
