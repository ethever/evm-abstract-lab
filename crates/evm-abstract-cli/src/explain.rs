//! `explain` 的输入适配：程序与世界入口共享同一组显式 EVM 环境参数。
//! 四个输入来源互斥；世界/RPC 复用 analyze 的准入、采集与执行规则。

use crate::{
    AnalysisArgs, DomainProfile, Input, StorageSlot, WorldArgs, defaults, error::CliError,
    evm::EvmArgs, number, parse_slot,
};
use clap::{ArgGroup, Args};
use evm_abstract::{Fork, analysis::Status, render, ssa};
use std::path::PathBuf;

#[derive(Args)]
#[group(skip)]
#[command(group(ArgGroup::new("explain-source").args(["hex","file","world","rpc"]).required(true).multiple(false)))]
#[command(group(ArgGroup::new("world-input").args(["world","rpc"]).multiple(false)))]
pub(crate) struct ExplainArgs {
    #[command(flatten)]
    symbolic: crate::symbolic::SymbolicArgs,
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
    /// Maximum live frames, including the entry frame.
    #[arg(long, default_value_t = defaults::MAX_CALL_DEPTH)]
    max_call_depth: usize,
    /// Maximum cumulative execution and domain work.
    #[arg(long, default_value_t = defaults::MAX_WORK)]
    max_work: usize,
    /// Maximum tracked memory bytes per frame and extracted byte-array range.
    #[arg(long, default_value_t = defaults::MAX_MEMORY_BYTES)]
    max_memory_bytes: usize,
    /// Constants retained per value; positive usize, without an additional cap.
    #[arg(long, default_value_t = defaults::MAX_CONSTANTS)]
    max_constants: usize,
    /// Numerical domain profile, shared across every call.
    #[arg(long,value_enum,default_value_t=DomainProfile::Product)]
    domain: DomainProfile,
    /// Maximum rounds in one temporary semantic fact exchange.
    #[arg(long, default_value_t = defaults::REDUCTION_ROUNDS)]
    reduction_rounds: usize,
    /// Maximum semantic atoms in one temporary fact exchange.
    #[arg(long, default_value_t = defaults::MAX_FACTS)]
    max_facts: usize,
    /// Recent jump-source blocks per frame; zero disables context sensitivity.
    #[arg(long, default_value_t = defaults::CONTEXT_DEPTH)]
    context_depth: usize,
    /// Maximum abstract states; unfinished explanations exit with code 2.
    #[arg(long, default_value_t = defaults::MAX_STATES)]
    max_states: usize,
    /// Maximum block transfers, including revisits.
    #[arg(long, default_value_t = defaults::MAX_TRANSFERS)]
    max_transfers: usize,
}

impl ExplainArgs {
    /// 同一份解释只运行一次分析。输入事实、准入错误及预算语义与原入口保持一致。
    pub(crate) fn run(self) -> Result<(String, bool), CliError> {
        if self.world.is_some() || self.rpc.is_some() {
            let args = WorldArgs {
                symbolic: self.symbolic,
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
                evm: self.evm,
                max_call_depth: self.max_call_depth,
                max_work: self.max_work,
                max_memory_bytes: self.max_memory_bytes,
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
                if self.allow_partial_ssa {
                    render::world::explain_verbose_with_partial_ssa(&analysis)?
                } else {
                    render::world::explain_verbose(&analysis)?
                }
            } else if self.allow_partial_ssa {
                render::world::explain_with_partial_ssa(&analysis)?
            } else {
                render::world::explain(&analysis)?
            };
            return Ok((output, complete));
        }
        let args = AnalysisArgs {
            max_work: self.max_work,
            max_call_depth: self.max_call_depth,
            max_memory_bytes: self.max_memory_bytes,
            symbolic: self.symbolic,
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
        } else if self.allow_partial_ssa {
            text.push_str(&render::partial_ssa(
                &analysis,
                &ssa::build_partial_world(analysis.execution())?,
            ));
        } else {
            text.push_str("SSA unavailable: analysis frontiers remain\n");
        }
        Ok((text, complete))
    }
}
