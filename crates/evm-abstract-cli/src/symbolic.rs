//! One symbolic/relational policy shared by raw-bytecode and world entrypoints.

use clap::{Args, ValueEnum};
use evm_abstract::domain::relational::{RelationLimits, SmtProvider};

#[derive(Clone, Copy, ValueEnum)]
enum Provider {
    Z3,
    Bitwuzla,
    Cvc5,
}

impl From<Provider> for SmtProvider {
    fn from(value: Provider) -> Self {
        match value {
            Provider::Z3 => Self::Z3,
            Provider::Bitwuzla => Self::Bitwuzla,
            Provider::Cvc5 => Self::Cvc5,
        }
    }
}

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
    /// In-process SMT provider; resource units differ between providers.
    #[arg(long = "smt.provider", value_enum, default_value = "z3")]
    smt_provider: Provider,
    /// Per-check allowance in provider-specific units, not a time limit.
    #[arg(
        long = "smt.rlimit",
        default_value_t = 100_000,
        long_help = "Per-check allowance, not seconds: native resource units for Z3/cvc5, cooperative termination checks for Bitwuzla. Units are not comparable across providers."
    )]
    smt_rlimit: u32,
}

impl SymbolicArgs {
    pub(crate) fn limits(&self) -> RelationLimits {
        RelationLimits {
            enabled: !self.no_relations,
            max_nodes: self.max_symbolic_nodes,
            max_depth: self.max_symbolic_depth,
            max_constraints: self.max_relations,
            provider: self.smt_provider.into(),
            rlimit: self.smt_rlimit,
        }
    }
}
