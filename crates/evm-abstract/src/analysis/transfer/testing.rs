//! Shared execution-context fixtures. Carriers keep their own independent
//! semantics and instruments; only the common runtime wiring lives here.

use super::{ExecutionConfig, TransferContext, WorkBudget};
use crate::domain::{AlgebraRef, EvmNumericAlgebra};

pub(super) fn context<'a, D: EvmNumericAlgebra>(
    config: &'a ExecutionConfig,
    budget: &'a mut WorkBudget,
    domain: AlgebraRef<'a, D>,
) -> TransferContext<'a, D> {
    TransferContext {
        config,
        domain,
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    }
}

pub(super) fn procedure_context<'a, D: EvmNumericAlgebra>(
    config: &'a ExecutionConfig,
    budget: &'a mut WorkBudget,
    domain: AlgebraRef<'a, D>,
) -> TransferContext<'a, D> {
    TransferContext {
        procedure: true,
        ..context(config, budget, domain)
    }
}
