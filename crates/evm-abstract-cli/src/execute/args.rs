//! Shared execution flags. Numeric policy and input-source flags have separate
//! owners; commands choose summary reuse explicitly when constructing config.

use crate::defaults;
use clap::Args;

#[derive(Args)]
#[group(skip)]
pub(crate) struct ExecutionArgs {
    #[command(flatten)]
    pub(crate) coverage: RootTerminalCoverageArgs,
    /// Recent jump-source blocks per frame; 0 disables context sensitivity.
    #[arg(long, default_value_t = defaults::CONTEXT_DEPTH)]
    pub(crate) context_depth: usize,
    /// Maximum abstract machine states; exhaustion returns exit code 2.
    #[arg(long, default_value_t = defaults::MAX_STATES)]
    pub(crate) max_states: usize,
    /// Maximum block transfers, including revisits.
    #[arg(long, default_value_t = defaults::MAX_TRANSFERS)]
    pub(crate) max_transfers: usize,
    /// Maximum cumulative execution and domain work.
    #[arg(long, default_value_t = defaults::MAX_WORK)]
    pub(crate) max_work: usize,
    /// Maximum live frames, including entry; reaching the cap retains a frontier.
    #[arg(long, default_value_t = defaults::MAX_CALL_DEPTH)]
    pub(crate) max_call_depth: usize,
    /// Maximum logical memory footprint per frame; copies retain lazy ranges.
    #[arg(long, default_value_t = defaults::MAX_MEMORY_BYTES)]
    pub(crate) max_memory_bytes: usize,
}

#[derive(Args)]
#[group(skip)]
pub(crate) struct RootTerminalCoverageArgs {
    /// Enable conservative root terminal coverage (default); detailed frontiers still exit 2.
    #[arg(long, conflicts_with = "no_root_terminal_coverage")]
    root_terminal_coverage: bool,
    /// Disable conservative root terminal coverage preparation.
    #[arg(long, conflicts_with = "root_terminal_coverage")]
    no_root_terminal_coverage: bool,
}

impl RootTerminalCoverageArgs {
    pub(crate) fn enabled(&self) -> bool {
        // Neither switch has a clap default, so an explicit opt-out stays valid.
        self.root_terminal_coverage || !self.no_root_terminal_coverage
    }
}
