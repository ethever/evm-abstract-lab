//! Concrete effects share the revm collector; instruction/SSA obligations stay here.
use evm_abstract::{
    analysis::EvmWorldAnalysis,
    domain::NumericValue,
    world::{EvmEntry, EvmWorld},
};
#[path = "../support/revm.rs"]
mod concrete;
pub(super) use concrete::Observation as Concrete;

pub(super) fn compare(
    world: &EvmWorld<NumericValue>,
    entry: &EvmEntry<NumericValue>,
    analysis: &EvmWorldAnalysis<NumericValue>,
) -> Concrete {
    assert!(
        world
            .accounts()
            .iter()
            .all(|(_, account)| !account.storage_unknown),
        "symbolic-state revm fixtures require complete initial storage"
    );
    let observed = concrete::execute(world, entry);
    observed.covers(analysis);
    // Also check concrete block-entry stacks, so outcome-only Unknown cannot pass.
    for step in &observed.steps {
        let matching: Vec<_> = analysis
            .states()
            .iter()
            .filter(|state| {
                state.active().address == step.owner
                    && step
                        .code_address
                        .is_none_or(|code| state.active().code_address == code)
                    && state.active().caller.as_concrete() == Some(step.caller)
                    && state.active().is_static == step.is_static
                    && state.executed_pcs.contains(&step.pc)
            })
            .collect();
        assert!(
            !matching.is_empty(),
            "concrete frame/instruction missing: {step:?}"
        );
        if matching
            .iter()
            .any(|state| state.executed_pcs.first() == Some(&step.pc))
        {
            assert!(
                matching.iter().any(|state| {
                    let stack = &state.entry.active().stack;
                    stack.len() == step.stack.len()
                        && stack
                            .iter()
                            .zip(&step.stack)
                            .all(|(value, concrete)| value.contains(*concrete))
                }),
                "concrete block-entry stack missing: {step:?}"
            );
        }
    }
    evm_abstract::ssa::build_world(analysis)
        .unwrap()
        .verify(analysis)
        .unwrap();
    observed
}
