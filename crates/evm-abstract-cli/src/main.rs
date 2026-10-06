//! CLI 只负责参数、文件和输出；分析与渲染逻辑在库中，便于逐层学习和复用。

mod error;
mod explain;
mod number;
mod world;

#[cfg(test)]
mod tests;

use alloy_primitives::U256;
use clap::{Args, Parser, Subcommand, ValueEnum};
use error::CliError;
use evm_abstract::{
    Fork,
    analysis::{self, Config, ExecutionConfig, Status},
    bytecode::Program,
    domain::Profile,
    render, ssa,
    world::{
        ByteArray, Entry,
        rpc::{self, AccountRequest, RpcBlock, RpcInput},
    },
};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

#[derive(Parser)]
#[command(
    version,
    about = "Analyze cross-contract EVM execution: world → call frames → graph → SSA"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze an offline or fixed RPC world, including calls, returns and state effects.
    Analyze {
        #[command(flatten)]
        args: Box<WorldArgs>,
        #[arg(long, value_enum, default_value = "text")]
        format: CfgFormat,
        /// Build and verify SSA for the complete cross-contract execution graph.
        #[arg(long)]
        ssa: bool,
    },
    /// Decode ordinary EVM runtime bytecode under the selected fork (default: Osaka).
    Disasm {
        #[command(flatten)]
        input: Input,
        #[arg(long, value_enum, default_value = "text")]
        format: TextFormat,
    },
    /// Recover a conservative CFG using a finite-set abstract stack.
    Cfg {
        #[command(flatten)]
        args: AnalysisArgs,
        #[arg(long, value_enum, default_value = "text")]
        format: CfgFormat,
    },
    /// Build and verify stack SSA, including predecessor-indexed phi nodes.
    Ssa {
        #[command(flatten)]
        args: AnalysisArgs,
        #[arg(long, value_enum, default_value = "text")]
        format: TextFormat,
    },
    /// Explain bytecode or a multi-account world with disassembly, states and verified SSA.
    Explain {
        #[command(flatten)]
        args: Box<explain::ExplainArgs>,
    },
}

#[derive(Args)]
struct WorldArgs {
    /// Offline JSON world snapshot; analysis never fetches missing facts from a node.
    #[arg(long, required_unless_present = "rpc", conflicts_with = "rpc")]
    world: Option<PathBuf>,
    /// Trusted HTTP(S) RPC; discover chain ID and pin the selected block once.
    #[arg(long, conflicts_with = "world")]
    rpc: Option<String>,
    /// Disable on-demand RPC acquisition of concrete missing callees.
    #[arg(long, requires = "rpc", conflicts_with = "world")]
    no_rpc_discovery: bool,
    /// Maximum initial and discovered RPC accounts in one fixed snapshot.
    #[arg(
        long,
        requires = "rpc",
        conflicts_with = "world",
        default_value_t = 256
    )]
    max_rpc_accounts: usize,
    /// Maximum cumulative RPC requests, including identity checks and failures.
    #[arg(
        long,
        requires = "rpc",
        conflicts_with = "world",
        default_value_t = 16_384
    )]
    max_rpc_requests: usize,
    /// Exact 32-byte block hash; defaults to pinning the RPC's latest block.
    #[arg(long, requires = "rpc", conflicts_with_all = ["block_number", "world"])]
    block_hash: Option<String>,
    /// Block height, decimal or 0x/0X hexadecimal; resolve once to a fixed hash.
    #[arg(long, requires = "rpc", conflicts_with_all = ["block_hash", "world"], value_parser = number::block)]
    block_number: Option<u64>,
    /// Additional account to fetch before analysis (repeatable); entry is automatic.
    #[arg(long, requires = "rpc", conflicts_with = "world")]
    account: Vec<String>,
    /// Storage observation ADDRESS:SLOT (repeatable); SLOT is decimal or 0x/0X hex.
    #[arg(long, requires = "rpc", conflicts_with = "world", value_parser = parse_slot)]
    slot: Vec<StorageSlot>,
    /// RPC execution rules, selected explicitly or defaulting to Osaka.
    #[arg(long, requires = "rpc", conflicts_with = "world")]
    fork: Option<Fork>,
    /// Disable completed call-summary reuse for oracle comparisons.
    #[arg(long)]
    no_summaries: bool,
    /// Entry account address (20 bytes of hex).
    #[arg(long)]
    entry: String,
    /// Caller address (20 bytes of hex).
    #[arg(long, default_value = "0x0000000000000000000000000000000000001000")]
    caller: String,
    /// Concrete input bytes, with an optional 0x prefix.
    #[arg(long, default_value = "0x")]
    calldata: String,
    /// Entry CALLVALUE in wei: decimal or 0x/0X-prefixed hexadecimal (256 bits).
    #[arg(long, default_value = "0", value_parser = number::parse)]
    value: U256,
    /// Start with a static frame; descendants inherit the restriction.
    #[arg(long = "static")]
    is_static: bool,
    /// Maximum live frames, including entry; reaching the cap retains a frontier.
    #[arg(long, default_value_t = 32)]
    max_call_depth: usize,
    /// Maximum cumulative execution and domain work.
    #[arg(long, default_value_t = 20_000_000)]
    max_work: usize,
    /// Maximum tracked memory bytes per frame.
    #[arg(long, default_value_t = 65_536)]
    max_memory_bytes: usize,
    /// Constants retained per value; positive usize, default 8, without an additional cap.
    #[arg(long, default_value_t = 8)]
    max_constants: usize,
    /// Numerical domain: combined facts or the constants-only comparison.
    #[arg(long, value_enum, default_value_t = DomainProfile::Product)]
    domain: DomainProfile,
    /// Maximum complete rounds per temporary fact exchange.
    #[arg(long, default_value_t = 4)]
    reduction_rounds: usize,
    /// Maximum semantic atoms per temporary fact lattice.
    #[arg(long, default_value_t = 256)]
    max_facts: usize,
    /// Recent jump-source blocks retained within each frame; 0 disables context sensitivity.
    #[arg(long, default_value_t = 8)]
    context_depth: usize,
    /// Maximum abstract machine states.
    #[arg(long, default_value_t = 4096)]
    max_states: usize,
    /// Maximum block transfers, including revisits.
    #[arg(long, default_value_t = 100_000)]
    max_transfers: usize,
}

#[derive(Clone)]
struct StorageSlot {
    address: alloy_primitives::Address,
    slot: alloy_primitives::U256,
}

fn parse_slot(input: &str) -> Result<StorageSlot, String> {
    let (account, slot) = input
        .split_once(':')
        .ok_or_else(|| "expected ADDRESS:SLOT".to_owned())?;
    Ok(StorageSlot {
        address: world::address(account, "storage account").map_err(|error| error.to_string())?,
        slot: number::parse(slot).map_err(|error| format!("invalid storage slot: {error}"))?,
    })
}

#[derive(Args)]
#[group(skip)]
struct Input {
    /// Hex runtime bytecode (optional 0x prefix).
    #[arg(long, required_unless_present = "file", conflicts_with = "file")]
    hex: Option<String>,
    /// UTF-8 file containing hex bytecode; whitespace is accepted.
    #[arg(long, required_unless_present = "hex", conflicts_with = "hex")]
    file: Option<PathBuf>,
    /// Mainnet execution rules: cancun, prague or osaka (Fusaka).
    #[arg(long, default_value_t = Fork::default())]
    fork: Fork,
}

#[derive(Args)]
struct AnalysisArgs {
    #[command(flatten)]
    input: Input,
    /// Recent jump-source blocks retained in the context; 0 disables context sensitivity.
    #[arg(long, default_value_t = 8)]
    context_depth: usize,
    /// Constants retained per stack slot; positive usize, default 8, without an additional cap.
    #[arg(long, default_value_t = 8)]
    max_constants: usize,
    /// Numerical domain: combined facts or the constants-only comparison.
    #[arg(long, value_enum, default_value_t = DomainProfile::Product)]
    domain: DomainProfile,
    /// Maximum complete rounds per temporary fact exchange.
    #[arg(long, default_value_t = 4)]
    reduction_rounds: usize,
    /// Maximum semantic atoms per temporary fact lattice.
    #[arg(long, default_value_t = 256)]
    max_facts: usize,
    /// Maximum abstract states; exhaustion returns exit code 2.
    #[arg(long, default_value_t = 4096)]
    max_states: usize,
    /// Maximum block transfers, including revisits; exhaustion returns exit code 2.
    #[arg(long, default_value_t = 100_000)]
    max_transfers: usize,
}

#[derive(Clone, Copy, ValueEnum)]
enum DomainProfile {
    Product,
    ConstantsOnly,
}
impl From<DomainProfile> for Profile {
    fn from(value: DomainProfile) -> Self {
        match value {
            DomainProfile::Product => Self::Product,
            DomainProfile::ConstantsOnly => Self::ConstantsOnly,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum TextFormat {
    Text,
    Json,
}
#[derive(Clone, Copy, ValueEnum)]
enum CfgFormat {
    Text,
    Json,
    Dot,
}

impl Input {
    fn load(self) -> Result<Program, CliError> {
        let text = match (self.hex, self.file) {
            (Some(hex), None) => hex,
            (None, Some(file)) => fs::read_to_string(file)?,
            _ => unreachable!("clap requires exactly one bytecode source"),
        };
        Ok(Program::from_hex_with_fork(&text, self.fork)?)
    }
}

impl AnalysisArgs {
    fn analyze(self) -> Result<analysis::Analysis, CliError> {
        let config = Config {
            domain_profile: self.domain.into(),
            reduction_rounds: self.reduction_rounds,
            max_facts: self.max_facts,
            max_constants: self.max_constants,
            context_depth: self.context_depth,
            max_states: self.max_states,
            max_transfers: self.max_transfers,
        };
        Ok(analysis::analyze(self.input.load()?, config)?)
    }
}

impl WorldArgs {
    fn analyze(self) -> Result<analysis::WorldAnalysis, CliError> {
        let entry_address = world::address(&self.entry, "entry")?;
        let (world, rpc_input) = match (self.world, self.rpc) {
            (Some(path), None) => (Some(world::load(&path)?), None),
            (None, Some(endpoint)) => {
                let mut input = RpcInput::new(endpoint, self.fork.unwrap_or_default());
                input.block = match (self.block_hash, self.block_number) {
                    (Some(hash), None) => RpcBlock::Hash(world::hash(&hash, "block")?),
                    (None, Some(number)) => RpcBlock::Number(number),
                    (None, None) => RpcBlock::Latest,
                    (Some(_), Some(_)) => unreachable!("clap makes block selectors exclusive"),
                };
                let mut accounts = std::collections::BTreeMap::new();
                accounts.insert(entry_address, std::collections::BTreeSet::new());
                for account in self.account {
                    accounts
                        .entry(world::address(&account, "RPC account")?)
                        .or_default();
                }
                for slot in self.slot {
                    accounts.entry(slot.address).or_default().insert(slot.slot);
                }
                input.accounts = accounts
                    .into_iter()
                    .map(|(address, slots)| AccountRequest { address, slots })
                    .collect();
                input.max_accounts = self.max_rpc_accounts;
                input.max_requests = self.max_rpc_requests;
                (None, Some(input))
            }
            _ => unreachable!("clap requires exactly one world input"),
        };
        let entry = Entry {
            address: entry_address,
            caller: world::address(&self.caller, "caller")?,
            value: evm_abstract::domain::Value::constant(self.value),
            calldata: ByteArray::exact(&world::calldata(&self.calldata)?),
            is_static: self.is_static,
        };
        let config = ExecutionConfig {
            analysis: Config {
                domain_profile: self.domain.into(),
                reduction_rounds: self.reduction_rounds,
                max_facts: self.max_facts,
                max_constants: self.max_constants,
                context_depth: self.context_depth,
                max_states: self.max_states,
                max_transfers: self.max_transfers,
            },
            max_work: self.max_work,
            max_call_depth: self.max_call_depth,
            max_memory_bytes: self.max_memory_bytes,
            symbolic_entry_environment: false,
            use_summaries: !self.no_summaries,
        };
        match (world, rpc_input) {
            (Some(world), None) => Ok(analysis::analyze_world(world, entry, config)?),
            (None, Some(input)) if self.no_rpc_discovery => {
                Ok(analysis::analyze_world(rpc::load(&input)?, entry, config)?)
            }
            (None, Some(input)) => {
                let result = analysis::analyze_rpc(&input, entry, config)?;
                for failure in result.failures() {
                    eprintln!("RPC discovery: {failure}");
                }
                Ok(result.into_analysis())
            }
            _ => unreachable!("clap requires exactly one world input"),
        }
    }
}

fn run() -> Result<ExitCode, CliError> {
    let (output, complete) = match Cli::parse().command {
        Command::Analyze { args, format, ssa } => {
            let analysis = args.analyze()?;
            let complete = analysis.status() == Status::Converged;
            let ir = if ssa && complete {
                Some(ssa::build_world(&analysis)?)
            } else {
                None
            };
            if ssa && !complete {
                eprintln!("SSA unavailable: cross-contract frontiers remain");
            }
            (
                match format {
                    CfgFormat::Text => {
                        let mut output = render::world::text(&analysis);
                        if let Some(ir) = ir {
                            output.push('\n');
                            output.push_str(&render::world::ssa(&analysis, &ir));
                        }
                        output
                    }
                    CfgFormat::Json => match ir {
                        Some(ir) => serde_json::to_string_pretty(
                            &serde_json::json!({"analysis": render::world::json(&analysis)?, "ssa": ir}),
                        )?,
                        None => serde_json::to_string_pretty(&render::world::json(&analysis)?)?,
                    },
                    CfgFormat::Dot => render::world::dot(&analysis),
                },
                complete,
            )
        }
        Command::Disasm { input, format } => {
            let program = input.load()?;
            (
                match format {
                    TextFormat::Text => render::disassembly(&program),
                    TextFormat::Json => serde_json::to_string_pretty(&program)?,
                },
                true,
            )
        }
        Command::Cfg { args, format } => {
            let analysis = args.analyze()?;
            (
                match format {
                    CfgFormat::Text => render::cfg(&analysis),
                    CfgFormat::Json => serde_json::to_string_pretty(&analysis)?,
                    CfgFormat::Dot => render::dot(&analysis),
                },
                analysis.status() == Status::Converged,
            )
        }
        Command::Ssa { args, format } => {
            let analysis = args.analyze()?;
            if analysis.status() == Status::Incomplete {
                eprintln!(
                    "{}SSA unavailable: analysis frontiers remain",
                    render::cfg(&analysis)
                );
                return Ok(ExitCode::from(2));
            }
            let ssa = ssa::build(&analysis)?;
            (
                match format {
                    TextFormat::Text => render::ssa(&analysis, &ssa),
                    TextFormat::Json => serde_json::to_string_pretty(
                        &serde_json::json!({"analysis": analysis, "ssa": ssa}),
                    )?,
                },
                true,
            )
        }
        Command::Explain { args } => args.run()?,
    };
    let mut stdout = io::stdout().lock();
    match writeln!(stdout, "{output}") {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
        Err(error) => return Err(error.into()),
    }
    Ok(if complete {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
