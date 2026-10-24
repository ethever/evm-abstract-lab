//! `explain` 的输入适配：程序与世界入口共享同一组显式 EVM 环境参数。
//! 四个输入来源互斥；世界/RPC 复用 analyze 的准入、采集与执行规则。

use crate::{
    AnalysisArgs, ExecutionArgs, Input, NumericArgs, StorageSlot, WorldArgs, defaults,
    error::CliError, evm::EvmArgs, number, parse_slot, text_with_root_terminal_coverage,
};
use clap::{ArgGroup, Args};
use evm_abstract::{
    Fork,
    analysis::{EvmAnalysis, EvmWorldAnalysis, FrameCode, Status, control::Control},
    domain::{EvmNumericAlgebra, ValueAlgebra},
    render,
    resource::WorkBudget,
    ssa,
};
use std::path::PathBuf;

#[derive(Args)]
#[group(skip)]
#[command(group(ArgGroup::new("explain-source").args(["hex","file","world","rpc"]).required(true).multiple(false)))]
#[command(group(ArgGroup::new("world-input").args(["world","rpc"]).multiple(false)))]
pub(crate) struct ExplainArgs {
    #[command(flatten)]
    pub(crate) numeric: NumericArgs,
    /// Ordinary EVM runtime bytecode as hex.
    #[arg(long)]
    hex: Option<String>,
    /// File containing one ordinary EVM runtime program as hex.
    #[arg(long)]
    file: Option<PathBuf>,
    /// Offline world JSON; teaching view, with complete effects available via --verbose.
    #[arg(long, requires = "evm.to")]
    world: Option<PathBuf>,
    /// Trusted HTTP(S) RPC; discover chain ID and pin the selected block once.
    #[arg(long, requires = "evm.to")]
    rpc: Option<String>,
    /// Expand all captured frames, machine effects and reports for world/RPC input.
    #[arg(long, requires = "world-input")]
    verbose: bool,
    /// Emit recorded partial SSA when frontiers remain; the exit code stays 2.
    #[arg(long)]
    allow_partial_ssa: bool,
    #[command(flatten)]
    execution: ExecutionArgs,
    /// Disable on-demand acquisition of missing RPC code and storage slots.
    #[arg(long, requires = "rpc", conflicts_with_all = ["hex", "file", "world"])]
    no_rpc_discovery: bool,
    /// Maximum initial and discovered RPC accounts.
    #[arg(long, requires = "rpc", conflicts_with_all = ["hex", "file", "world"], default_value_t = defaults::MAX_RPC_ACCOUNTS)]
    max_rpc_accounts: usize,
    /// Maximum cumulative RPC requests, including identity checks and failures.
    #[arg(long, requires = "rpc", conflicts_with_all = ["hex", "file", "world"], default_value_t = defaults::MAX_RPC_REQUESTS)]
    max_rpc_requests: usize,
    /// Maximum bytes accepted per RPC response.
    #[arg(long, requires = "rpc", conflicts_with_all = ["hex", "file", "world"], default_value_t = defaults::MAX_RPC_RESPONSE_BYTES)]
    max_rpc_response_bytes: usize,
    /// Total timeout per RPC request, including its response body, in milliseconds.
    #[arg(long, requires = "rpc", conflicts_with_all = ["hex", "file", "world"], default_value_t = defaults::RPC_TIMEOUT_MS)]
    rpc_timeout_ms: u64,
    /// Exact 32-byte block hash; defaults to pinning the RPC's latest block.
    #[arg(long, requires = "rpc", conflicts_with_all = ["block_number", "hex", "file", "world"])]
    block_hash: Option<String>,
    /// Block height, decimal or 0x/0X hexadecimal; resolve once to a fixed hash.
    #[arg(long, requires = "rpc", conflicts_with_all = ["block_hash", "hex", "file", "world"], value_parser = number::block)]
    block_number: Option<u64>,
    /// Additional RPC account observation (repeatable).
    #[arg(long, requires = "rpc", conflicts_with_all = ["hex", "file", "world"])]
    account: Vec<String>,
    /// RPC storage observation ADDRESS:SLOT (repeatable).
    #[arg(long, requires = "rpc", conflicts_with_all = ["hex", "file", "world"], value_parser = parse_slot)]
    slot: Vec<StorageSlot>,
    /// Program/RPC rules; offline worlds carry their own fork.
    #[arg(long, conflicts_with = "world")]
    fork: Option<Fork>,
    #[command(flatten)]
    evm: EvmArgs,
    /// Disable complete callee summary reuse for the world.
    #[arg(long, requires = "world-input")]
    no_summaries: bool,
}

impl ExplainArgs {
    /// 同一份解释只运行一次分析。输入事实、准入错误及预算语义与原入口保持一致。
    pub(crate) fn run<D: EvmNumericAlgebra>(
        self,
        algebra: &ValueAlgebra<D>,
        control: &Control,
        reporting: &mut WorkBudget,
    ) -> Result<(String, bool), CliError> {
        if self.world.is_some() || self.rpc.is_some() {
            let args = WorldArgs {
                numeric: self.numeric,
                world: self.world,
                rpc: self.rpc,
                no_rpc_discovery: self.no_rpc_discovery,
                max_rpc_accounts: self.max_rpc_accounts,
                max_rpc_requests: self.max_rpc_requests,
                max_rpc_response_bytes: self.max_rpc_response_bytes,
                rpc_timeout_ms: self.rpc_timeout_ms,
                block_hash: self.block_hash,
                block_number: self.block_number,
                account: self.account,
                slot: self.slot,
                fork: self.fork,
                no_summaries: self.no_summaries,
                execution: self.execution,
                evm: self.evm,
            };
            let analysis = args.analyze_hybrid(algebra, control)?;
            let mut text = if analysis.native_chosen() {
                world_explanation(
                    analysis.chosen(),
                    algebra,
                    reporting,
                    self.verbose,
                    self.allow_partial_ssa,
                )?
                .0
            } else {
                render::world::process_explanation(analysis.chosen(), algebra, reporting)?
            };
            if let crate::hybrid::WorldAnalysis::Hybrid(hybrid) = &analysis {
                text.insert_str(0, &crate::hybrid::text(hybrid));
            } else if let crate::hybrid::WorldAnalysis::Rpc(rpc) = &analysis {
                text.insert_str(0, &crate::hybrid::rpc_text(rpc));
            }
            if let Some(detail) = analysis.companion_detail() {
                text.push_str(
                    "\nDetailed paths (Native state IDs; conditions and pending work above):\n",
                );
                text.push_str(
                    &world_explanation(
                        detail,
                        algebra,
                        reporting,
                        self.verbose,
                        self.allow_partial_ssa,
                    )?
                    .0,
                );
            }
            return Ok((text, analysis.status() == Status::Converged));
        }
        let args = AnalysisArgs {
            execution: self.execution,
            numeric: self.numeric,
            evm: self.evm,
            input: Input {
                hex: self.hex,
                file: self.file,
                fork: self.fork.unwrap_or_default(),
            },
        };
        let native = args.analyze(algebra, control)?;
        if native.root_code(reporting)? != FrameCode::Runtime {
            return world_explanation(&native, algebra, reporting, false, self.allow_partial_ssa);
        }
        let analysis = EvmAnalysis::from_execution(native, reporting)?;
        let mut text = render::disassembly(analysis.program());
        text.push('\n');
        text.push_str(&render::cfg(&analysis, algebra));
        text.push('\n');
        let complete = analysis.status() == Status::Converged;
        if complete {
            text.push_str(&render::normalized_ssa(
                &analysis,
                &ssa::build_normalized(&analysis, reporting)?,
                algebra,
            ));
        } else if self.allow_partial_ssa {
            text.push_str(&render::normalized_ssa(
                &analysis,
                &ssa::build_normalized_partial_world(analysis.execution(), reporting)?,
                algebra,
            ));
        } else {
            text.push_str("SSA unavailable: analysis frontiers remain\n");
        }
        Ok((
            text_with_root_terminal_coverage(text, analysis.root_terminal_coverage()),
            complete,
        ))
    }
}

fn world_explanation<D: EvmNumericAlgebra>(
    analysis: &EvmWorldAnalysis<D::Element>,
    algebra: &ValueAlgebra<D>,
    reporting: &mut WorkBudget,
    verbose: bool,
    allow_partial_ssa: bool,
) -> Result<(String, bool), CliError> {
    let output = match (verbose, allow_partial_ssa) {
        (true, true) => {
            render::world::explain_verbose_with_partial_ssa(analysis, algebra, reporting)?
        }
        (true, false) => render::world::explain_verbose(analysis, algebra, reporting)?,
        (false, true) => render::world::explain_with_partial_ssa(analysis, algebra, reporting)?,
        (false, false) => render::world::explain(analysis, algebra, reporting)?,
    };
    Ok((
        text_with_root_terminal_coverage(output, analysis.root_terminal_coverage()),
        analysis.status() == Status::Converged,
    ))
}
