//! CLI 只负责参数、文件和输出；分析与渲染逻辑在库中，便于逐层学习和复用。

mod world;

use clap::{Args, Parser, Subcommand, ValueEnum};
use evm_abstract::{
    Fork,
    analysis::{self, Config, ExecutionConfig, Status},
    bytecode::Program,
    render, ssa,
    world::{ByteArray, Entry},
};
use std::{
    error::Error,
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
    /// Analyze an offline multi-account world, including calls, returns and state effects.
    Analyze {
        #[command(flatten)]
        args: WorldArgs,
        #[arg(long, value_enum, default_value = "text")]
        format: CfgFormat,
        /// Build and verify SSA for the complete cross-contract execution graph.
        #[arg(long)]
        ssa: bool,
    },
    /// Decode legacy runtime bytecode under the selected fork (default: Osaka).
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
    /// Show disassembly, abstract states and SSA together for a learning example.
    Explain {
        #[command(flatten)]
        args: AnalysisArgs,
    },
}

#[derive(Args)]
struct WorldArgs {
    /// Offline JSON world snapshot; analysis never fetches missing facts from a node.
    #[arg(long)]
    world: PathBuf,
    /// Entry account address (20 bytes of hex).
    #[arg(long)]
    entry: String,
    /// Caller address (20 bytes of hex).
    #[arg(long, default_value = "0x0000000000000000000000000000000000001000")]
    caller: String,
    /// Concrete input bytes, with an optional 0x prefix.
    #[arg(long, default_value = "0x")]
    calldata: String,
    /// Entry CALLVALUE as a 0x-prefixed 256-bit word.
    #[arg(long, default_value = "0x0")]
    value: String,
    /// Start with a static frame; descendants inherit the restriction.
    #[arg(long = "static")]
    is_static: bool,
    /// Maximum live frames, including entry; reaching the cap retains a frontier.
    #[arg(long, default_value_t = 32)]
    max_call_depth: usize,
    /// Maximum cumulative execution and domain work.
    #[arg(long, default_value_t = 2_000_000)]
    max_work: usize,
    /// Maximum tracked memory bytes per frame.
    #[arg(long, default_value_t = 65_536)]
    max_memory_bytes: usize,
    /// Constants retained per value before promoting to Top.
    #[arg(long, default_value_t = 8)]
    max_constants: usize,
    /// Recent jump-source blocks retained within each frame.
    #[arg(long, default_value_t = 0)]
    context_depth: usize,
    /// Maximum abstract machine states.
    #[arg(long, default_value_t = 4096)]
    max_states: usize,
    /// Maximum block transfers, including revisits.
    #[arg(long, default_value_t = 100_000)]
    max_transfers: usize,
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
    /// Recent jump-source blocks retained in the context (0..=3).
    #[arg(long, default_value_t = 0)]
    context_depth: usize,
    /// Constants retained per stack slot before promoting to Top (1..=64).
    #[arg(long, default_value_t = 8)]
    max_constants: usize,
    /// Maximum abstract states; exhaustion returns exit code 2.
    #[arg(long, default_value_t = 4096)]
    max_states: usize,
    /// Maximum block transfers, including revisits; exhaustion returns exit code 2.
    #[arg(long, default_value_t = 100_000)]
    max_transfers: usize,
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
    fn load(self) -> Result<Program, Box<dyn Error>> {
        let text = match (self.hex, self.file) {
            (Some(hex), None) => hex,
            (None, Some(file)) => fs::read_to_string(file)?,
            _ => unreachable!("clap requires exactly one bytecode source"),
        };
        Ok(Program::from_hex_with_fork(&text, self.fork)?)
    }
}

impl AnalysisArgs {
    fn analyze(self) -> Result<analysis::Analysis, Box<dyn Error>> {
        let config = Config {
            max_constants: self.max_constants,
            context_depth: self.context_depth,
            max_states: self.max_states,
            max_transfers: self.max_transfers,
        };
        Ok(analysis::analyze(self.input.load()?, config)?)
    }
}

impl WorldArgs {
    fn analyze(self) -> Result<analysis::WorldAnalysis, Box<dyn Error>> {
        let world = world::load(&self.world)?;
        let entry = Entry {
            address: world::address(&self.entry, "entry")?,
            caller: world::address(&self.caller, "caller")?,
            value: evm_abstract::domain::Value::constant(world::word(&self.value, "value")?),
            calldata: ByteArray::exact(&world::calldata(&self.calldata)?),
            is_static: self.is_static,
        };
        let config = ExecutionConfig {
            analysis: Config {
                max_constants: self.max_constants,
                context_depth: self.context_depth,
                max_states: self.max_states,
                max_transfers: self.max_transfers,
            },
            max_work: self.max_work,
            max_call_depth: self.max_call_depth,
            max_memory_bytes: self.max_memory_bytes,
            symbolic_entry_environment: false,
        };
        Ok(analysis::analyze_world(world, entry, config)?)
    }
}

fn run() -> Result<ExitCode, Box<dyn Error>> {
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
                            output.push_str("\nVerified cross-contract SSA:\n");
                            output.push_str(&serde_json::to_string_pretty(&ir)?);
                        }
                        output
                    }
                    CfgFormat::Json => match ir {
                        Some(ir) => serde_json::to_string_pretty(
                            &serde_json::json!({"analysis": analysis, "ssa": ir}),
                        )?,
                        None => serde_json::to_string_pretty(&analysis)?,
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
        Command::Explain { args } => {
            let analysis = args.analyze()?;
            let mut text = render::disassembly(analysis.program());
            text.push('\n');
            text.push_str(&render::cfg(&analysis));
            text.push('\n');
            if analysis.status() == Status::Converged {
                text.push_str(&render::ssa(&analysis, &ssa::build(&analysis)?));
            } else {
                text.push_str("SSA unavailable: analysis frontiers remain\n");
            }
            (text, analysis.status() == Status::Converged)
        }
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
