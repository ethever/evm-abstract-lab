//! Thin CLI adapter for the independent local decompiler library.

use crate::{Input, TextFormat, error::CliError};
use clap::{Args, ValueEnum};
use evm_abstract::analysis::control::Control;

#[derive(Clone, Copy, Default, ValueEnum)]
enum Strategy {
    #[default]
    Dispatcher,
    CodeDuplication,
}

#[derive(Args)]
pub(crate) struct DecompileArgs {
    #[command(flatten)]
    input: Input,
    /// Text contains structured code; JSON retains all IR stages and origins.
    #[arg(long, value_enum, default_value = "text")]
    format: TextFormat,
    /// Strategy for known irreducible control flow, independent of unknown jumps.
    #[arg(long, value_enum, default_value = "dispatcher")]
    strategy: Strategy,
    /// Cumulative recovery and transformation work.
    #[arg(long, default_value_t = evm_decompile::Limits::default().max_work)]
    max_work: usize,
    /// Maximum input code bytes, checked before local recovery.
    #[arg(long, default_value_t = evm_decompile::Limits::default().max_bytecode_bytes)]
    max_bytecode_bytes: usize,
    /// Maximum local recovery blocks, including distinct stack contexts.
    #[arg(long, default_value_t = evm_decompile::Limits::default().max_blocks)]
    max_blocks: usize,
    /// Maximum instruction operations retained in one stage.
    #[arg(long, default_value_t = evm_decompile::Limits::default().max_operations)]
    max_operations: usize,
    /// Maximum SSA word/effect definitions.
    #[arg(long, default_value_t = evm_decompile::Limits::default().max_values)]
    max_values: usize,
    /// Maximum local block evaluations before retaining an incomplete result.
    #[arg(long, default_value_t = evm_decompile::Limits::default().max_iterations)]
    max_iterations: usize,
    /// Finite candidate capacity; exceeding it produces an unknown fact.
    #[arg(long, default_value_t = evm_decompile::Limits::default().max_constant_set)]
    max_constants: usize,
    /// Maximum blocks introduced by the code-duplication strategy.
    #[arg(long, default_value_t = evm_decompile::Limits::default().max_cloned_blocks)]
    max_cloned_blocks: usize,
    /// Maximum recursive structured-region depth.
    #[arg(long, default_value_t = evm_decompile::Limits::default().max_structuring_depth)]
    max_structuring_depth: usize,
    /// Maximum entity occurrences retained across immutable history stages.
    #[arg(long, default_value_t = evm_decompile::Limits::default().max_history_entities)]
    max_history_entities: usize,
}

pub(crate) fn run(args: DecompileArgs, control: &Control) -> Result<(String, bool), CliError> {
    let (text, fork) = args.input.load_text()?;
    let input = evm_decompile::Input::new(evm_bytecode::parse_hex(&text)?, fork);
    let config = evm_decompile::Config {
        strategy: match args.strategy {
            Strategy::Dispatcher => evm_decompile::Strategy::Dispatcher,
            Strategy::CodeDuplication => evm_decompile::Strategy::CodeDuplication,
        },
        limits: evm_decompile::Limits {
            max_bytecode_bytes: args.max_bytecode_bytes,
            max_blocks: args.max_blocks,
            max_operations: args.max_operations,
            max_values: args.max_values,
            max_work: args.max_work,
            max_iterations: args.max_iterations,
            max_constant_set: args.max_constants,
            max_cloned_blocks: args.max_cloned_blocks,
            max_structuring_depth: args.max_structuring_depth,
            max_history_entities: args.max_history_entities,
        },
    };
    let artifact = evm_decompile::decompile_with_cancel(&input, &config, control.cancellation());
    let complete = artifact.completion == evm_decompile::Completion::Complete;
    let (output, exported) = match args.format {
        TextFormat::Text => {
            let export = evm_decompile::export_text(&artifact, control.cancellation());
            (
                export.text,
                export.status == evm_decompile::TextExportStatus::Complete,
            )
        }
        TextFormat::Json => (serde_json::to_string_pretty(&artifact)?, true),
    };
    Ok((output, complete && exported))
}
