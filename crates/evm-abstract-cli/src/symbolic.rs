//! One symbolic/relational policy shared by raw-bytecode and world entrypoints.

use crate::defaults;
use clap::Args;
use evm_abstract::domain::relational::RelationLimits;

#[derive(Args)]
pub(crate) struct SymbolicArgs {
    /// Disable local path reasoning for a numerical comparison.
    #[arg(long)]
    no_relations: bool,
    /// Maximum retained expression nodes; overflow keeps a conservative result.
    #[arg(long, default_value_t = defaults::MAX_EXPRESSION_NODES)]
    max_symbolic_nodes: usize,
    /// Maximum symbolic expression depth.
    #[arg(long, default_value_t = defaults::MAX_EXPRESSION_DEPTH)]
    max_symbolic_depth: usize,
    /// Maximum constraints retained in one machine state.
    #[arg(long, default_value_t = defaults::MAX_CONSTRAINTS)]
    max_relations: usize,
}

impl SymbolicArgs {
    pub(crate) fn limits(&self) -> RelationLimits {
        RelationLimits {
            enabled: !self.no_relations,
            max_nodes: self.max_symbolic_nodes,
            max_depth: self.max_symbolic_depth,
            max_constraints: self.max_relations,
        }
    }
}
