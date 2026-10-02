//! CLI 只负责参数、文件和输出；分析与渲染逻辑在库中，便于逐层学习和复用。

use clap::{Args, Parser, Subcommand, ValueEnum};
use evm_abstract::{
    analysis::{self, Config, Status},
    bytecode::Program,
    render, ssa,
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
    about = "Learn EVM abstract execution: bytecode → contextual CFG → stack SSA"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decode Cancun legacy runtime bytecode and show basic blocks.
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
#[group(required = true, multiple = false)]
struct Input {
    /// Hex runtime bytecode (optional 0x prefix).
    #[arg(long)]
    hex: Option<String>,
    /// UTF-8 file containing hex bytecode; whitespace is accepted.
    #[arg(long)]
    file: Option<PathBuf>,
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
            _ => unreachable!("clap validates the input group"),
        };
        Ok(Program::from_hex(&text)?)
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

fn run() -> Result<ExitCode, Box<dyn Error>> {
    let (output, complete) = match Cli::parse().command {
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
                    "{}SSA unavailable: budget frontiers remain",
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
                text.push_str("SSA unavailable: budget frontiers remain\n");
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
