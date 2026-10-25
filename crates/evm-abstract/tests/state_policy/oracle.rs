//! Shared coherent revm observations, reused across each strategy/cache mode.
use super::support::{entry, input};
use evm_abstract::{domain::NumericValue, world::EvmWorld};
#[path = "../support/revm.rs"]
mod concrete;
pub(super) use concrete::{Observation as Witness, execute};

/// Concrete execution depends on the fixture and input, never on the policy
/// graph being checked. Validate each witness once, then reuse the complete
/// tuple for every strategy/cache mode; `covers` still selects one terminal.
pub(super) fn witnesses(
    world: &EvmWorld<NumericValue>,
    inputs: &[u64],
    check: impl Fn(u64, &Witness),
) -> Vec<Witness> {
    inputs
        .iter()
        .map(|&n| {
            let witness = execute(world, &entry(input(n)));
            check(n, &witness);
            witness
        })
        .collect()
}
