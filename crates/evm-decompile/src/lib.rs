//! Offline local EVM recovery, immutable semantic IR and traceable structuring.
//!
//! Every instruction/result identity is allocated once before dataflow. Numeric
//! facts use the shared finite EVM algebra; memory observations are scoped to
//! explicit dependencies. Unresolved jumps are represented exits, never evidence
//! that a recovered graph is closed. Historical stages are owned snapshots.

mod artifact;
mod effects;
mod execute;
mod recover;
mod render;
mod structure;
mod transform;
mod validate;

pub use alloy_primitives::U256;
pub use analysis_control::Cancellation;
pub use evm_bytecode::Fork;
pub use evm_decompile_model::*;
pub use execute::*;
pub use render::{
    TextExport, TextExportStatus, export_text, export_text_with_observer, render_text,
    render_text_with_cancel, render_text_with_observer,
};
pub use validate::validate_stage;

/// Recover and structure one runtime bytecode input with an explicit fork.
pub fn decompile(input: &Input, config: &Config) -> Artifact {
    decompile_with_cancel(input, config, &Cancellation::default())
}

/// Cooperatively cancellable decompilation; interrupted attempts retain stages.
pub fn decompile_with_cancel(input: &Input, config: &Config, cancel: &Cancellation) -> Artifact {
    decompile_with_progress(input, config, cancel, |_| {})
}

/// Publish typed phase/counter observations through a statically composed callback.
pub fn decompile_with_progress(
    input: &Input,
    config: &Config,
    cancel: &Cancellation,
    progress: impl FnMut(Progress),
) -> Artifact {
    analysis_control::with_cancellation(cancel, || {
        let mut builder = artifact::Builder::new(input, config, cancel, progress);
        builder.notify_phase(Phase::Decode);
        if recover::run(&mut builder) {
            builder.notify_phase(Phase::Simplify);
            transform::simplify(&mut builder);
            builder.notify_phase(Phase::ControlFlow);
            if config.strategy == Strategy::CodeDuplication {
                transform::split_irreducible(&mut builder);
            }
            builder.notify_phase(Phase::Structure);
            structure::run(&mut builder);
        }
        builder.notify_phase(Phase::Complete);
        builder.finish()
    })
}
