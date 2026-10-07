//! One symbolic/relational policy shared by raw-bytecode and world entrypoints.

use clap::Args;
use evm_abstract::domain::relational::RelationLimits;

#[derive(Args)]
pub(crate) struct SymbolicArgs {
    /// Disable symbolic expressions and path constraints for a numerical comparison.
    #[arg(long)]
    no_relations: bool,
    /// Maximum expression/query nodes; overflow keeps a conservative result.
    #[arg(long, default_value_t = 1024)]
    max_symbolic_nodes: usize,
    /// Maximum symbolic expression depth.
    #[arg(long, default_value_t = 64)]
    max_symbolic_depth: usize,
    /// Maximum constraints retained in one machine state.
    #[arg(long, default_value_t = 128)]
    max_relations: usize,
    /// Deterministic Z3 resource limit per query.
    #[arg(long, default_value_t = 10_000)]
    smt_rlimit: u32,
}

impl SymbolicArgs {
    pub(crate) fn limits(&self) -> RelationLimits {
        RelationLimits {
            enabled: !self.no_relations,
            max_nodes: self.max_symbolic_nodes,
            max_depth: self.max_symbolic_depth,
            max_constraints: self.max_relations,
            rlimit: self.smt_rlimit,
        }
    }
}
