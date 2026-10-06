//! `explain` 的输入适配：程序与世界入口共享同一组显式 EVM 环境参数。
//! 四个输入来源互斥；世界/RPC 复用 analyze 的准入、采集与执行规则。

use crate::{
    AnalysisArgs, DomainProfile, Input, StorageSlot, WorldArgs, error::CliError, evm::EvmArgs,
    number, parse_slot,
};
use clap::{ArgGroup, Args};
use evm_abstract::{
    Fork,
    analysis::{ExecutionConfig, Status},
    render, ssa,
};
use std::path::PathBuf;

#[derive(Args)]
#[group(skip)]
#[command(group(ArgGroup::new("explain-source").args(["hex","file","world","rpc"]).required(true).multiple(false)))]
#[command(group(ArgGroup::new("world-input").args(["world","rpc"]).multiple(false)))]
pub(crate) struct ExplainArgs {
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
    /// Disable on-demand acquisition of concrete missing RPC callees.
    #[arg(long, requires = "rpc", conflicts_with_all = ["hex", "file", "world"])]
    no_rpc_discovery: bool,
    /// Maximum initial and discovered RPC accounts; default 256.
    #[arg(long, requires = "rpc", conflicts_with_all = ["hex", "file", "world"])]
    max_rpc_accounts: Option<usize>,
    /// Maximum cumulative RPC requests; default 16384.
    #[arg(long, requires = "rpc", conflicts_with_all = ["hex", "file", "world"])]
    max_rpc_requests: Option<usize>,
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
    /// Maximum live world call frames; default 32.
    #[arg(long, requires = "world-input")]
    max_call_depth: Option<usize>,
    /// Shared world work budget; default 20000000.
    #[arg(long, requires = "world-input")]
    max_work: Option<usize>,
    /// World memory/range byte budget; default 65536.
    #[arg(long, requires = "world-input")]
    max_memory_bytes: Option<usize>,
    /// Constants retained per value; positive usize, default 8, without an additional cap.
    #[arg(long, default_value_t = 8)]
    max_constants: usize,
    /// Numerical domain profile, shared across every call.
    #[arg(long,value_enum,default_value_t=DomainProfile::Product)]
    domain: DomainProfile,
    /// Maximum rounds in one temporary semantic fact exchange.
    #[arg(long, default_value_t = 4)]
    reduction_rounds: usize,
    /// Maximum semantic atoms in one temporary fact exchange.
    #[arg(long, default_value_t = 256)]
    max_facts: usize,
    /// Recent jump-source blocks per frame; zero disables context sensitivity.
    #[arg(long, default_value_t = 8)]
    context_depth: usize,
    /// Maximum abstract states; unfinished explanations exit with code 2.
    #[arg(long, default_value_t = 4096)]
    max_states: usize,
    /// Maximum block transfers, including revisits.
    #[arg(long, default_value_t = 100_000)]
    max_transfers: usize,
}

impl ExplainArgs {
    /// 同一份解释只运行一次分析。输入事实、准入错误及预算语义与原入口保持一致。
    pub(crate) fn run(self) -> Result<(String, bool), CliError> {
        if self.world.is_some() || self.rpc.is_some() {
            let defaults = ExecutionConfig::default();
            let args = WorldArgs {
                world: self.world,
                rpc: self.rpc,
                no_rpc_discovery: self.no_rpc_discovery,
                max_rpc_accounts: self.max_rpc_accounts.unwrap_or(256),
                max_rpc_requests: self.max_rpc_requests.unwrap_or(16_384),
                block_hash: self.block_hash,
                block_number: self.block_number,
                account: self.account,
                slot: self.slot,
                fork: self.fork,
                no_summaries: self.no_summaries,
                evm: self.evm,
                max_call_depth: self.max_call_depth.unwrap_or(defaults.max_call_depth),
                max_work: self.max_work.unwrap_or(defaults.max_work),
                max_memory_bytes: self.max_memory_bytes.unwrap_or(defaults.max_memory_bytes),
                max_constants: self.max_constants,
                domain: self.domain,
                reduction_rounds: self.reduction_rounds,
                max_facts: self.max_facts,
                context_depth: self.context_depth,
                max_states: self.max_states,
                max_transfers: self.max_transfers,
            };
            let analysis = args.analyze()?;
            let complete = analysis.status() == Status::Converged;
            let output = if self.verbose {
                render::world::explain_verbose(&analysis)?
            } else {
                render::world::explain(&analysis)?
            };
            return Ok((output, complete));
        }
        let args = AnalysisArgs {
            evm: self.evm,
            input: Input {
                hex: self.hex,
                file: self.file,
                fork: self.fork.unwrap_or_default(),
            },
            context_depth: self.context_depth,
            max_constants: self.max_constants,
            domain: self.domain,
            reduction_rounds: self.reduction_rounds,
            max_facts: self.max_facts,
            max_states: self.max_states,
            max_transfers: self.max_transfers,
        };
        let analysis = args.analyze()?;
        let mut text = render::disassembly(analysis.program());
        text.push('\n');
        text.push_str(&render::cfg(&analysis));
        text.push('\n');
        let complete = analysis.status() == Status::Converged;
        if complete {
            text.push_str(&render::ssa(&analysis, &ssa::build(&analysis)?));
        } else {
            text.push_str("SSA unavailable: analysis frontiers remain\n");
        }
        Ok((text, complete))
    }
}
