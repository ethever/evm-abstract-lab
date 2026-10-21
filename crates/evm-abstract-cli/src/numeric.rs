//! Application-owned numeric selection and limits. The engine receives an
//! already constructed algebra and execution budgets separately.

use crate::{defaults, symbolic::SymbolicArgs};
use clap::{Args, ValueEnum};
use evm_abstract::domain::DomainSpec;
use serde::Serialize;
use std::{fmt, num::NonZeroUsize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum NumericModel {
    Product,
    FiniteSet,
}

impl fmt::Display for NumericModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Product => "product",
            Self::FiniteSet => "finite-set",
        })
    }
}

#[derive(Args)]
#[group(skip)]
pub(crate) struct NumericArgs {
    /// Numeric algebra used throughout execution: product facts or finite sets.
    #[arg(long, value_enum, default_value_t = NumericModel::Product)]
    pub(crate) numeric_model: NumericModel,
    /// Constants retained per value; positive usize, without an additional cap.
    #[arg(long, default_value_t = defaults::MAX_CONSTANTS)]
    max_constants: usize,
    /// Maximum complete rounds per temporary fact exchange.
    #[arg(long, default_value_t = defaults::REDUCTION_ROUNDS)]
    reduction_rounds: usize,
    /// Maximum semantic atoms per temporary fact lattice.
    #[arg(long, default_value_t = defaults::MAX_FACTS)]
    max_facts: usize,
    #[command(flatten)]
    symbolic: SymbolicArgs,
}

#[derive(Debug, Error)]
pub(crate) enum NumericConfigError {
    #[error("max_constants must be positive")]
    Constants,
    #[error("reduction_rounds and max_facts must be positive")]
    Facts,
    #[error("symbolic node/depth and relation limits must be positive")]
    Relations,
}

impl NumericArgs {
    pub(crate) fn spec(&self) -> Result<DomainSpec, NumericConfigError> {
        let capacity =
            NonZeroUsize::new(self.max_constants).ok_or(NumericConfigError::Constants)?;
        let rounds = NonZeroUsize::new(self.reduction_rounds).ok_or(NumericConfigError::Facts)?;
        let facts = NonZeroUsize::new(self.max_facts).ok_or(NumericConfigError::Facts)?;
        let relations = self.symbolic.limits();
        if relations.max_nodes == 0 || relations.max_depth == 0 || relations.max_constraints == 0 {
            return Err(NumericConfigError::Relations);
        }
        Ok(DomainSpec::new(capacity, rounds, facts).with_relations(relations))
    }
}
