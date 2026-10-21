//! CLI 只负责参数、文件和输出；分析与渲染逻辑在库中，便于逐层学习和复用。

mod defaults;
mod error;
mod evm;
mod execute;
mod explain;
mod json;
mod number;
mod numeric;
mod symbolic;
mod world;

#[cfg(test)]
mod tests;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use error::CliError;
use evm::EvmArgs;
use evm_abstract::{
    Fork,
    analysis::{self, EvmAnalysis, EvmWorldAnalysis, FrameCode, Status, control::Control},
    bytecode::Program,
    domain::{EvmNumericAlgebra, FiniteSetAlgebra, ProductAlgebra, ValueAlgebra, WordElement},
    render,
    resource::WorkBudget,
    ssa,
};
use execute::ExecutionArgs;
use numeric::{NumericArgs, NumericModel};
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
        /// Emit recorded partial SSA when frontiers remain; the exit code stays 2.
        #[arg(long, requires = "ssa")]
        allow_partial_ssa: bool,
    },
    /// Decode ordinary EVM runtime bytecode under the selected fork (default: Osaka).
    Disasm {
        #[command(flatten)]
        input: Input,
        #[arg(long, value_enum, default_value = "text")]
        format: TextFormat,
    },
    /// Recover a conservative CFG using the selected native numeric algebra.
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
        /// Emit native machine SSA for recorded prefixes when frontiers remain.
        #[arg(long)]
        allow_partial_ssa: bool,
    },
    /// Explain bytecode or a multi-account world with disassembly, states and verified SSA.
    Explain {
        #[command(flatten)]
        args: Box<explain::ExplainArgs>,
    },
}

#[derive(Args)]
struct WorldArgs {
    #[command(flatten)]
    numeric: NumericArgs,
    /// Offline JSON world snapshot; analysis never fetches missing facts from a node.
    #[arg(
        long,
        required_unless_present = "rpc",
        conflicts_with = "rpc",
        requires = "evm.to"
    )]
    world: Option<PathBuf>,
    /// Trusted HTTP(S) RPC; discover chain ID and pin the selected block once.
    #[arg(long, conflicts_with = "world", requires = "evm.to")]
    rpc: Option<String>,
    /// Disable on-demand RPC acquisition of missing code and storage slots.
    #[arg(long, requires = "rpc", conflicts_with = "world")]
    no_rpc_discovery: bool,
    /// Maximum initial and discovered RPC accounts in one fixed snapshot.
    #[arg(
        long,
        requires = "rpc",
        conflicts_with = "world",
        default_value_t = defaults::MAX_RPC_ACCOUNTS
    )]
    max_rpc_accounts: usize,
    /// Maximum cumulative RPC requests, including identity checks and failures.
    #[arg(
        long,
        requires = "rpc",
        conflicts_with = "world",
        default_value_t = defaults::MAX_RPC_REQUESTS
    )]
    max_rpc_requests: usize,
    /// Maximum bytes accepted per RPC response.
    #[arg(long, requires = "rpc", conflicts_with = "world", default_value_t = defaults::MAX_RPC_RESPONSE_BYTES)]
    max_rpc_response_bytes: usize,
    /// Total timeout per RPC request, including its response body, in milliseconds.
    #[arg(long, requires = "rpc", conflicts_with = "world", default_value_t = defaults::RPC_TIMEOUT_MS)]
    rpc_timeout_ms: u64,
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
    #[command(flatten)]
    execution: ExecutionArgs,
    #[command(flatten)]
    evm: EvmArgs,
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
    execution: ExecutionArgs,
    #[command(flatten)]
    numeric: NumericArgs,
    #[command(flatten)]
    input: Input,
    #[command(flatten)]
    evm: EvmArgs,
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

impl Command {
    fn numeric(&self) -> Option<&NumericArgs> {
        match self {
            Self::Analyze { args, .. } => Some(&args.numeric),
            Self::Cfg { args, .. } | Self::Ssa { args, .. } => Some(&args.numeric),
            Self::Explain { args } => Some(&args.numeric),
            Self::Disasm { .. } => None,
        }
    }
}

fn run() -> Result<ExitCode, CliError> {
    let command = Cli::parse().command;
    let (output, complete) = execute(command, &Control::default())?;
    let mut stdout = io::stdout().lock();
    if output.is_empty() {
        return Ok(if complete {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(2)
        });
    }
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

/// Choose the actual carrier once. Every command below stays generic through
/// input preparation, execution, projection and rendering.
fn execute(command: Command, control: &Control) -> Result<(String, bool), CliError> {
    control.scope(|| {
        let Some(numeric) = command.numeric() else {
            let Command::Disasm { input, format } = command else {
                unreachable!("only disassembly does not execute bytecode")
            };
            let program = input.load()?;
            return Ok((
                match format {
                    TextFormat::Text => render::disassembly(&program),
                    TextFormat::Json => render::json::pretty(&program)?,
                },
                true,
            ));
        };
        let text_output = matches!(
            &command,
            Command::Analyze {
                format: CfgFormat::Text,
                ..
            } | Command::Cfg {
                format: CfgFormat::Text,
                ..
            } | Command::Ssa {
                format: TextFormat::Text,
                ..
            } | Command::Explain { .. }
        );
        let dot_output = matches!(
            &command,
            Command::Analyze {
                format: CfgFormat::Dot,
                ..
            } | Command::Cfg {
                format: CfgFormat::Dot,
                ..
            }
        );
        let model = numeric.numeric_model;
        let spec = numeric.spec()?;
        let (output, complete) = match model {
            NumericModel::Product => execute_with(
                command,
                model,
                &ValueAlgebra::new(ProductAlgebra::from_spec(spec)),
                control,
            ),
            NumericModel::FiniteSet => execute_with(
                command,
                model,
                &ValueAlgebra::new(FiniteSetAlgebra::from_spec(spec)),
                control,
            ),
        }?;
        Ok((
            if text_output && !output.is_empty() {
                format!("Numeric model: {model}\n{output}")
            } else if dot_output {
                format!("// Numeric model: {model}\n{output}")
            } else {
                output
            },
            complete,
        ))
    })
}

fn execute_with<D: EvmNumericAlgebra>(
    command: Command,
    model: NumericModel,
    algebra: &ValueAlgebra<D>,
    control: &Control,
) -> Result<(String, bool), CliError> {
    // Reporting has its own bounded ledger; a stopped execution must still be
    // able to export its recorded prefix under the normal application policy.
    let mut reporting = WorkBudget::new(defaults::DEFAULT_POSTPROCESS_WORK);
    let (output, complete) = match command {
        Command::Analyze {
            args,
            format,
            ssa,
            allow_partial_ssa,
        } => {
            if allow_partial_ssa && matches!(format, CfgFormat::Dot) {
                Cli::command()
                    .error(
                        clap::error::ErrorKind::ArgumentConflict,
                        "--allow-partial-ssa cannot be used with --format dot; use text or json",
                    )
                    .exit();
            }
            let analysis = args.analyze(algebra, control)?;
            world_output(
                &analysis,
                algebra,
                format,
                model,
                ssa,
                allow_partial_ssa,
                &mut reporting,
            )?
        }
        Command::Disasm { .. } => unreachable!("disassembly is handled before numeric dispatch"),
        Command::Cfg { args, format } => {
            let native = args.analyze(algebra, control)?;
            if native.root_code(&mut reporting)? != FrameCode::Runtime {
                world_output(
                    &native,
                    algebra,
                    format,
                    model,
                    false,
                    false,
                    &mut reporting,
                )?
            } else {
                let analysis = EvmAnalysis::from_execution(native, &mut reporting)?;
                (
                    match format {
                        CfgFormat::Text => text_with_root_terminal_coverage(
                            render::cfg(&analysis, algebra),
                            analysis.root_terminal_coverage(),
                        ),
                        CfgFormat::Json => json::cfg(&analysis, model, &reporting)?,
                        CfgFormat::Dot => render::dot(&analysis, algebra),
                    },
                    analysis.status() == Status::Converged,
                )
            }
        }
        Command::Ssa {
            args,
            format,
            allow_partial_ssa,
        } => {
            let native = args.analyze(algebra, control)?;
            if native.root_code(&mut reporting)? != FrameCode::Runtime {
                if native.status() == Status::Incomplete && !allow_partial_ssa {
                    eprintln!(
                        "{}SSA unavailable: analysis frontiers remain",
                        text_with_root_terminal_coverage(
                            render::world::text(&native, algebra),
                            native.root_terminal_coverage()
                        )
                    );
                    return Ok((String::new(), false));
                }
                let format = match format {
                    TextFormat::Text => CfgFormat::Text,
                    TextFormat::Json => CfgFormat::Json,
                };
                world_output(
                    &native,
                    algebra,
                    format,
                    model,
                    true,
                    allow_partial_ssa,
                    &mut reporting,
                )?
            } else {
                let analysis = EvmAnalysis::from_execution(native, &mut reporting)?;
                if analysis.status() == Status::Incomplete {
                    if !allow_partial_ssa {
                        eprintln!(
                            "{}SSA unavailable: analysis frontiers remain",
                            text_with_root_terminal_coverage(
                                render::cfg(&analysis, algebra),
                                analysis.root_terminal_coverage(),
                            )
                        );
                        return Ok((String::new(), false));
                    }
                    let ir =
                        ssa::build_normalized_partial_world(analysis.execution(), &mut reporting)?;
                    (
                        match format {
                            TextFormat::Text => text_with_root_terminal_coverage(
                                render::normalized_ssa(&analysis, &ir, algebra),
                                analysis.root_terminal_coverage(),
                            ),
                            TextFormat::Json => json::program(&analysis, &ir, model, &reporting)?,
                        },
                        false,
                    )
                } else {
                    let ssa = ssa::build_normalized(&analysis, &mut reporting)?;
                    (
                        match format {
                            TextFormat::Text => text_with_root_terminal_coverage(
                                render::normalized_ssa(&analysis, &ssa, algebra),
                                analysis.root_terminal_coverage(),
                            ),
                            TextFormat::Json => json::program(&analysis, &ssa, model, &reporting)?,
                        },
                        true,
                    )
                }
            }
        }
        Command::Explain { args } => args.run(algebra, control, &mut reporting)?,
    };
    Ok((output, complete))
}

fn world_output<D: EvmNumericAlgebra>(
    analysis: &EvmWorldAnalysis<D::Element>,
    algebra: &ValueAlgebra<D>,
    format: CfgFormat,
    model: NumericModel,
    ssa: bool,
    allow_partial_ssa: bool,
    reporting: &mut WorkBudget,
) -> Result<(String, bool), CliError> {
    let complete = analysis.status() == Status::Converged;
    let ir = if ssa && (complete || allow_partial_ssa) {
        Some(ssa::build_normalized_partial_world(analysis, reporting)?)
    } else {
        None
    };
    if ssa && !complete && ir.is_none() {
        eprintln!("SSA unavailable: cross-contract frontiers remain");
    }
    Ok((
        match format {
            CfgFormat::Text => {
                let mut output = render::world::text(analysis, algebra);
                if let Some(ir) = ir {
                    output.push('\n');
                    output.push_str(&if complete {
                        render::world::normalized_ssa_verbose(analysis, &ir)
                    } else {
                        render::world::normalized_ssa(analysis, &ir)
                    });
                }
                text_with_root_terminal_coverage(output, analysis.root_terminal_coverage())
            }
            CfgFormat::Json => json::world(analysis, ir.as_ref(), model, reporting)?,
            CfgFormat::Dot => render::world::dot(analysis, algebra),
        },
        complete,
    ))
}

fn text_with_root_terminal_coverage<N: WordElement>(
    mut output: String,
    coverage: Option<&analysis::EvmRootTerminalCoverage<N>>,
) -> String {
    if let Some(coverage) = coverage {
        use std::fmt::Write as _;
        let outcomes = coverage.outcomes();
        writeln!(
            output,
            "\nRoot terminal coverage (conservative): scope={:?}; alternatives={:?}, {:?}, {:?}\n  Original root input; terminal data and Stores are separate from detailed status, frontiers and SSA.",
            coverage.scope(),
            outcomes[0].kind(),
            outcomes[1].kind(),
            outcomes[2].kind(),
        )
        .expect("writing to a String is infallible");
    }
    output
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
