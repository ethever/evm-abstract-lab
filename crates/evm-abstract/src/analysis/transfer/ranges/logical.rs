//! Logical-memory success and the conservative gas-failure envelope.
//!
//! Gas argument for excluding a nonempty end above B = MAX & !31:
//! such an access needs at least 2^251 words. The locally pinned revm 43.0.2
//! `revm-interpreter/src/gas.rs` uses u64 limits/remaining gas; the default
//! `revm-context-interface/src/cfg/gas_params.rs` memory_cost is positive linear
//! plus quadratic, with `cfg/gas.rs::MEMORY = 3`. These are the trusted default
//! costs for this analyzer's Cancun/Prague/Osaka forks, without custom gas tables.
//! Even one gas per word at 2^251 words exceeds u64::MAX. This is a cumulative
//! frame invariant: a frame starts with empty memory (calls::initial and child
//! entry), pays the successive differences of the increasing memory cost, and
//! cannot already have reached that extent with u64 gas. Memory is NOT assumed
//! fresh at each access; refunds do not replenish the executing gas counter.
//!
//! This only justifies the excluded representability domain. We do not prove
//! exact affordability inside B: retain success and possible failure there,
//! independently of host memory caps and of the analysis work ledger.

use crate::domain::EvmNumericAlgebra;

use super::{EvmExecution, TransferContext};

use crate::{analysis::OutcomeKind, world::EvmByteArray};
use alloy_primitives::U256;

pub(super) const MAX_END: U256 = U256::from_limbs([u64::MAX - 31, u64::MAX, u64::MAX, u64::MAX]);

/// Reserve the failure envelope's payload copies before normal frame completion.
/// Root completion meters rollback and its actual finalization plan in
/// calls::finish; child completion retains its output-copy reservation there.
pub(super) fn failure<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> bool {
    let payload = &result.payload;
    // Input + completed-call snapshots, restored store, output-store snapshot,
    // normalization and their retained scalar/state terms are covered by these
    // passes over the complete payload (including every saved checkpoint).
    let cost = payload.work_size().saturating_mul(8).saturating_add(1);
    if !context.budget.charge(cost) {
        super::super::boundary(result, pc, crate::analysis::FrontierReason::Work);
        return false;
    }
    let before = result.frontiers.len();
    super::finish(
        result,
        result.payload.clone(),
        OutcomeKind::Failure,
        EvmByteArray::<D::Element>::empty(),
        context,
        pc,
    );
    result.frontiers.len() == before
}

#[cfg(test)]
mod tests;
