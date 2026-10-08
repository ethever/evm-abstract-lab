//! 从实际帧捕获的代码开始解释整个事务，再展示原始机器证据与已验证 SSA。
//!
//! 代码目录不重新查询初始账户：创建部署的覆盖层、委托调用的代码身份和
//! 暂停帧都必须保留。反汇编是语法指令列表，不能冒充抽象状态图的具体执行轨迹。

use crate::{
    analysis::{FrameCode, MachinePayload, MachineState, Status, WorldAnalysis},
    bytecode::Program,
    ssa::{self, SsaError},
    world::Code,
};
use alloy_primitives::{Address, B256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

pub(super) mod teaching;

type CodeIdentity = (Address, B256, FrameCode);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CapturePhase {
    Entry,
    Exit,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct FrameReference {
    state: usize,
    frame: usize,
    phase: CapturePhase,
    active: bool,
}

#[derive(Default)]
struct CapturedCode<'a> {
    program: Option<&'a Program>,
    owners: BTreeSet<Address>,
    references: BTreeSet<FrameReference>,
    transfers: Vec<&'a MachineState>,
}

/// 完整解释允许保留未完成分析；只有收敛图才构建并验证 SSA。
pub(super) fn render(analysis: &WorldAnalysis) -> Result<String, SsaError> {
    render_with_mode(analysis, false)
}

pub(super) fn render_partial(analysis: &WorldAnalysis) -> Result<String, SsaError> {
    render_with_mode(analysis, true)
}

fn render_with_mode(analysis: &WorldAnalysis, allow_partial_ssa: bool) -> Result<String, SsaError> {
    let mut output = String::from("Cross-contract explanation\n");
    writeln!(
        output,
        "  fork={} | domain={:?} | status={:?}",
        analysis.world().fork(),
        analysis.domain_spec().profile(),
        analysis.status(),
    )
    .unwrap();
    writeln!(
        output,
        "  snapshot={}",
        super::identity(analysis.world().identity())
    )
    .unwrap();
    output
        .push_str("  Context-sensitive abstract state graph; not a concrete instruction trace.\n");
    write_code(&mut output, analysis);
    output.push('\n');
    output.push_str(&super::text(analysis));
    output.push('\n');
    if analysis.status() == Status::Converged {
        let ir = ssa::build_world(analysis)?;
        output.push_str(&super::ssa::render(analysis, &ir));
    } else if allow_partial_ssa {
        output.push_str(&super::partial_ssa_verbose(
            analysis,
            &ssa::build_partial_world(analysis)?,
        ));
    } else {
        output.push_str("SSA unavailable: cross-contract frontiers remain\n");
    }
    Ok(output)
}

fn capture<'a>(
    codes: &mut BTreeMap<CodeIdentity, CapturedCode<'a>>,
    payload: &'a MachinePayload,
    state: usize,
    phase: CapturePhase,
) {
    let active = payload.call_stack.depth() - 1;
    for (frame_index, frame) in payload.call_stack.iter().enumerate() {
        let key = (frame.key.code_address, frame.key.code_hash, frame.code);
        let code = codes.entry(key).or_default();
        // 借用帧已捕获的 Program；Store 的后续代码变化不能改写此前执行身份。
        if code.program.is_none() {
            code.program = frame.program.as_ref();
        }
        code.owners.insert(frame.key.address);
        code.references.insert(FrameReference {
            state,
            frame: frame_index,
            phase,
            active: frame_index == active,
        });
    }
}

fn write_code(output: &mut String, analysis: &WorldAnalysis) {
    let mut codes = BTreeMap::<CodeIdentity, CapturedCode<'_>>::new();
    for state in analysis.states() {
        capture(&mut codes, &state.entry, state.id, CapturePhase::Entry);
        if let Some(exit) = &state.exit {
            capture(&mut codes, exit, state.id, CapturePhase::Exit);
        }
        let frame = state.entry.active();
        codes
            .get_mut(&(frame.key.code_address, frame.key.code_hash, frame.code))
            .expect("active entry frame was captured")
            .transfers
            .push(state);
    }

    output.push_str("\nExecution code\n");
    output.push_str("  Instruction lists are syntactic and can include unvisited instructions.\n");
    output.push_str("  executed_pcs records each state's last abstract block transfer, not a transaction trace.\n");
    if codes.is_empty() {
        output.push_str(
            "  No executable frame was captured; see Frontiers for the unresolved entry.\n",
        );
    }
    let captured_program = codes.values().any(|code| code.program.is_some());
    for (index, ((address, hash, mode), code)) in codes.iter_mut().enumerate() {
        writeln!(
            output,
            "\n  C{index} | code={} | code_hash={hash} | mode={mode:?}",
            super::environment::owner(analysis, *address),
        )
        .unwrap();
        let owners = code
            .owners
            .iter()
            .map(|address| super::environment::owner(analysis, *address))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(output, "    state owners: {owners}").unwrap();
        output.push_str("    captured frames:\n");
        for reference in &code.references {
            let phase = match reference.phase {
                CapturePhase::Entry => "entry",
                CapturePhase::Exit => "exit",
            };
            let role = if reference.active {
                "active"
            } else {
                "suspended"
            };
            writeln!(
                output,
                "      S{} frame={} {phase} {role}",
                reference.state, reference.frame
            )
            .unwrap();
        }
        match (mode, code.program) {
            (FrameCode::Runtime | FrameCode::InitCode, Some(program)) => {
                writeln!(
                    output,
                    "    Captured instruction list ({} bytes):",
                    program.byte_len()
                )
                .unwrap();
                output.push_str(&crate::render::disassembly(program));
            }
            (FrameCode::Precompile(address), _) => {
                writeln!(
                    output,
                    "    Native precompile {address}; no bytecode instruction list."
                )
                .unwrap();
            }
            (FrameCode::Empty, _) => {
                output.push_str("    Empty executable code: implicit successful halt; no bytecode instruction list.\n");
            }
            (FrameCode::InvalidDelegation, _) => {
                output.push_str("    Invalid nested delegation: exceptional halt at the second marker; no captured bytecode instruction list.\n");
            }
            (_, None) => {
                output
                    .push_str("    No captured bytecode program; instruction list unavailable.\n");
            }
        }
        code.transfers.sort_unstable_by_key(|state| state.id);
        if !code.transfers.is_empty() {
            output.push_str("    Active abstract transfers:\n");
        }
        for state in &code.transfers {
            let block = state.active().basic_block_index;
            let location = state.program().map_or_else(
                || "non-bytecode frame".to_owned(),
                |program| {
                    if block == program.blocks().len() {
                        "synthetic end-of-code continuation (no instruction)".to_owned()
                    } else {
                        format!("B{block}")
                    }
                },
            );
            let pcs = state
                .executed_pcs
                .iter()
                .map(|pc| format!("0x{pc:04x}"))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                output,
                "      S{} | {location} | executed_pcs=[{pcs}]",
                state.id
            )
            .unwrap();
        }
    }
    if !captured_program {
        write_input_code(output, analysis);
    }
}

fn write_input_code(output: &mut String, analysis: &WorldAnalysis) {
    // 初始化预算可能耗尽而没有帧。快照已知代码仍可供学习，但不能算作执行证据。
    output.push_str("\nInput code observations (not execution evidence)\n");
    output.push_str(
        "  Initial snapshot facts only; these instruction lists do not establish execution.\n",
    );
    if analysis.world().accounts().is_empty() {
        output.push_str("  No input account code observations.\n");
    }
    for (address, account) in analysis.world().accounts() {
        writeln!(
            output,
            "  account={} | observed_code_hash={}",
            super::environment::owner(analysis, *address),
            super::hash_label(analysis.world().code_hash(*address))
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
                    "    Delegation indicator targeting {target}; no runtime instruction list captured."
                )
                .unwrap();
            }
            Code::Empty => {
                output.push_str("    Observed empty code; no bytecode instruction list.\n");
            }
            Code::Unknown => {
                output.push_str("    Code unknown; no bytecode instruction list.\n");
            }
        }
    }
}

#[cfg(test)]
mod tests;
