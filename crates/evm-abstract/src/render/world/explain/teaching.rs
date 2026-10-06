//! 默认教学视图先展示代码、状态图和结果，再给出可验证的调用 SSA。
//!
//! 目录使用帧实际捕获的程序；简略效果只用于阅读，不能作为结果去重依据。

use super::super::teaching::References;
use crate::{
    analysis::{FrameCode, FrameKey, MachinePayload, MachineState, Status, WorldAnalysis},
    bytecode::Program,
    domain::{Domain, Value},
    ssa::{self, SsaError},
    world::{ByteArray, Code, Store},
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
pub(in crate::render::world) fn render(analysis: &WorldAnalysis) -> Result<String, SsaError> {
    let refs = References::new(analysis);
    let mut output = String::from("Cross-contract explanation\n");
    writeln!(
        output,
        "  status={:?} | fork={} | domain={:?} | states={} | edges={} | outcomes={}",
        analysis.status(),
        analysis.world().fork(),
        analysis.domain_spec().profile(),
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
    writeln!(
        output,
        "  entry={} | caller={} | static={} | value={} | calldata length={}",
        address(&refs, analysis.entry().address),
        address(&refs, analysis.entry().caller),
        analysis.entry().is_static,
        analysis.entry().value,
        length(analysis.entry().calldata.len()),
    )
    .unwrap();
    output
        .push_str("  Context-sensitive abstract state graph; not a concrete instruction trace.\n");
    output.push_str("  S# = state; B# = frame-local block; F# = frame within a state; C# = captured code; U# = unresolved frontier; stacks are bottom-to-top.\n");
    write_rpc(&mut output, analysis, &refs);
    write_addresses(&mut output, &refs);
    write_code(&mut output, analysis, &refs);
    write_cfg(&mut output, analysis, &refs);
    write_outcomes(&mut output, analysis, &refs);
    write_diagnostics(&mut output, analysis);
    write_frontiers(&mut output, analysis, &refs);
    output.push('\n');
    if analysis.status() == Status::Converged {
        output.push_str(&super::super::ssa::teaching::render(
            analysis,
            &ssa::build_world(analysis)?,
        ));
    } else {
        output.push_str("SSA unavailable: cross-contract frontiers remain\n");
    }
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

fn write_rpc(output: &mut String, analysis: &WorldAnalysis, refs: &References) {
    let Some(acquisition) = analysis.rpc_acquisition() else {
        return;
    };
    writeln!(
        output,
        "  RPC acquisition: rounds={} | requests={} | fetched={} | failed={} | states created={}",
        acquisition.rounds,
        acquisition.requests,
        acquisition.fetched_accounts.len(),
        acquisition.failed_accounts.len(),
        acquisition.states_created,
    )
    .unwrap();
    output.push_str("  RPC policy: trusted source, fixed canonical block hash; unrequested storage remains unknown.\n");
    // 后续预算可能阻止再次到达失败调用，累计获取失败仍要显示。
    for failed in &acquisition.failures {
        writeln!(
            output,
            "  RPC failure {}: {:?} resource={:?} limit={:?} | {} | {}",
            address(refs, failed.address),
            failed.failure.kind,
            failed.failure.resource,
            failed.failure.limit,
            failed.failure.context,
            failed.failure.message,
        )
        .unwrap();
    }
}

fn capture<'a>(codes: &mut BTreeMap<CodeIdentity, CapturedCode<'a>>, payload: &'a MachinePayload) {
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

fn write_code(output: &mut String, analysis: &WorldAnalysis, refs: &References) {
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
                for line in crate::render::disassembly(program).lines() {
                    writeln!(output, "      {line}").unwrap();
                }
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

fn write_input_code(output: &mut String, analysis: &WorldAnalysis, refs: &References) {
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
                for line in crate::render::disassembly(program).lines() {
                    writeln!(output, "      {line}").unwrap();
                }
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

fn location(state: &MachineState) -> String {
    let frame = state.active();
    if let Some(block) = state
        .program()
        .and_then(|program| program.blocks().get(frame.basic_block_index))
    {
        return format!("B{} @ 0x{:04x}", block.id, block.start_pc);
    }
    match frame.mode {
        FrameCode::Runtime | FrameCode::InitCode => {
            "synthetic end-of-code continuation (no instruction)".to_owned()
        }
        FrameCode::Precompile(_) => "native precompile (no bytecode block)".to_owned(),
        FrameCode::Empty => "empty code (implicit halt)".to_owned(),
        FrameCode::InvalidDelegation => "invalid nested delegation (exceptional halt)".to_owned(),
    }
}

fn write_cfg(output: &mut String, analysis: &WorldAnalysis, refs: &References) {
    output.push_str("\nCFG\n");
    output.push_str("  pcs records the last abstract block transfer, not a transaction trace.\n");
    if analysis.states().is_empty() {
        output.push_str("  (no captured states)\n");
    }
    for state in analysis.states() {
        let frame = state.entry.active();
        write!(
            output,
            "  S{} | {} | F{} active | code={}",
            state.id,
            location(state),
            state.entry.call_stack.depth() - 1,
            refs.code(&frame.key)
                .expect("captured frame has code reference"),
        )
        .unwrap();
        if frame.key.address != frame.key.code_address {
            write!(
                output,
                " | state owner={}",
                address(refs, frame.key.address)
            )
            .unwrap();
        }
        writeln!(
            output,
            " | caller={} | static={} | context={:?}",
            address(refs, frame.key.caller),
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
            writeln!(output, "    -> S{} {:?}", edge.to, edge.kind).unwrap();
        }
    }
}

fn length(value: &Value) -> String {
    match value.constants() {
        Some(values) if values.len() == 1 => format!(
            "{value} ({} bytes)",
            values.first().expect("nonempty constants"),
        ),
        Some(_) => format!("{value} (possible byte counts)"),
        None => format!("{value} (unknown byte count)"),
    }
}

fn data_summary(data: &ByteArray) -> String {
    if let Some(bytes) = data.exact_bytes_bounded(32) {
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

fn write_outcomes(output: &mut String, analysis: &WorldAnalysis, refs: &References) {
    output.push_str("\nOutcomes\n");
    output.push_str(
        "  Each O# keeps its own state and effects; equal returndata does not merge outcomes.\n",
    );
    output.push_str("  Selected effect facts below; complete storage, balances, lifecycle, logs and byte facts: --verbose or analyze.\n");
    if analysis.outcomes().is_empty() {
        output.push_str("  (none)\n");
    }
    let domain = Domain::from_spec(analysis.domain_spec());
    let mut initial = Store::new(analysis.world());
    // 与执行入口使用同一域表示，避免把初始事实的投影误当成程序写入。
    initial.project(domain);
    for (index, outcome) in analysis.outcomes().iter().enumerate() {
        writeln!(
            output,
            "  O{index} | S{} | {:?} | returndata length={} | {}",
            outcome.state,
            outcome.kind,
            length(outcome.data.len()),
            data_summary(&outcome.data),
        )
        .unwrap();
        let changed = outcome
            .store
            .slots()
            .iter()
            .filter(|((owner, slot), value)| {
                !same_numeric_fact(
                    &initial.read(*owner, &Value::constant(*slot), domain),
                    value,
                )
            })
            .collect::<Vec<_>>();
        write!(output, "    abstract explicit storage changes:").unwrap();
        for ((owner, slot), value) in changed.iter().take(4) {
            write!(output, " {}[{slot:#x}]={value}", address(refs, *owner)).unwrap();
        }
        if changed.is_empty() {
            output.push_str(" (none)");
        } else if changed.len() > 4 {
            write!(output, " | {} more in --verbose", changed.len() - 4).unwrap();
        }
        output.push('\n');
        let balances = outcome
            .store
            .addresses()
            .into_iter()
            .filter_map(|owner| {
                let balance = outcome.store.read_balance(owner);
                (!same_numeric_fact(&balance, &initial.read_balance(owner)))
                    .then_some((owner, balance))
            })
            .collect::<Vec<_>>();
        if !balances.is_empty() {
            output.push_str("    changed balances:");
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

fn same_numeric_fact(left: &Value, right: &Value) -> bool {
    // 来源改变不是余额或存储数值改变；相同单点无需比较其来源标签。
    if let Some(value) = left.singleton()
        && right.singleton() == Some(value)
    {
        return true;
    }
    left.constants() == right.constants()
        && left.known_bits() == right.known_bits()
        && left.interval() == right.interval()
        && left.congruence() == right.congruence()
        && left.may_be_zero() == right.may_be_zero()
}

fn write_diagnostics(output: &mut String, analysis: &WorldAnalysis) {
    output.push_str("\nDiagnostics\n");
    if analysis.diagnostics().is_empty() {
        output.push_str("  (none)\n");
    }
    for diagnostic in analysis.diagnostics() {
        writeln!(
            output,
            "  S{} @ 0x{:04x}: {:?}",
            diagnostic.state, diagnostic.pc, diagnostic.kind,
        )
        .unwrap();
    }
}

fn write_frontiers(output: &mut String, analysis: &WorldAnalysis, refs: &References) {
    output.push_str("\nFrontiers\n");
    if analysis.frontiers().is_empty() {
        output.push_str("  (none)\n");
    }
    for (index, frontier) in analysis.frontiers().iter().enumerate() {
        writeln!(
            output,
            "  U{index} | from={} | pc={} | reason={:?}",
            frontier
                .from
                .map_or_else(|| "none".to_owned(), |state| format!("S{state}")),
            frontier
                .pc
                .map_or_else(|| "none".to_owned(), |pc| format!("0x{pc:04x}")),
            frontier.reason,
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
    write!(output, "      F{frame} | B{} | ", key.basic_block_index).unwrap();
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
        address(refs, key.address),
        address(refs, key.caller),
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
