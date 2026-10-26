//! Prepared Process carriers use their finite operational coordinates.

use super::*;
use crate::analysis::{
    KnownProcessSummaries, ProcessFrameKey, ProcessLocation, ProcessOwner, ProcessSource,
};
use alloy_primitives::B256;

fn prepared<const Q: usize, const C: usize>(
    values: &ValueAlgebra<Algebra<Q, C>>,
) -> (EvmEntry<Word<Q, C>>, EvmPayloadSummary<Word<Q, C>>) {
    let (entry, input, _) = fixture(values);
    let mut budget = WorkBudget::new(100_000_000);
    let native = input
        .instantiate(&mut PolicyContext::new(
            values.borrowed(),
            &entry,
            &mut budget,
        ))
        .unwrap();
    let mut input = crate::analysis::state::payload::caller::form_with_runtime(
        &native,
        &entry,
        values.borrowed(),
        &mut budget,
        true,
    )
    .unwrap();
    for frame in &mut input.frames {
        let key = ProcessFrameKey {
            epoch: B256::ZERO,
            owner: ProcessOwner::Named(frame.key.address),
            source: ProcessSource::Runtime {
                source: frame.key.code_address,
                hash: frame.key.code_hash,
                fork: Fork::Osaka,
            },
            is_static: frame.key.is_static,
            location: ProcessLocation::Block(0),
        };
        frame.key.install_process(key);
    }
    input.location.frames = input.frames.iter().map(|frame| frame.key.clone()).collect();
    input.location.code_identity = B256::ZERO;
    input.work = input.measure_work();
    (entry, input)
}

#[test]
fn prepared_process_reuses_exact_native_bound_without_repeating_layout_interpretation() {
    let values = ValueAlgebra::new(Algebra::<64, 4096>);
    let (entry, input) = prepared(&values);
    assert!(input.frames[0].stack.is_empty());
    for acceleration in [Acceleration::Join, Acceleration::Widen] {
        let mut fast_budget = WorkBudget::new(100_000_000);
        let (fast, copies) = measure(|| {
            KnownProcessSummaries.admit_relative(
                &input,
                &input,
                PolicySite::Body,
                acceleration,
                &mut PolicyContext::new(values.borrowed(), &entry, &mut fast_budget),
            )
        });
        let fast = fast.unwrap();
        assert_eq!(fast, input);
        let mut full_budget = WorkBudget::new(100_000_000);
        let (full, full_copies) = measure(|| {
            input.regional_join(
                &input,
                acceleration,
                &mut PolicyContext::new(values.borrowed(), &entry, &mut full_budget),
            )
        });
        assert!(fast_budget.used() < full_budget.used());
        assert!(copies.clones < full_copies.clones);
        for output in [fast, full.unwrap()] {
            for store in [&output.store, &output.frames[0].saved_store] {
                let balance = store
                    .instantiate(values.borrowed())
                    .unwrap()
                    .read_balance(entry.address);
                assert!(!balance.contains(U256::from(17)));
                assert!(balance.numeric().observation().contains(U256::from(17)));
            }
        }
    }
}

#[test]
fn unprepared_native_input_keeps_the_regional_policy_and_stack_aliases() {
    let values = ValueAlgebra::new(Algebra::<64, 64>);
    let (entry, input, _) = fixture(&values);
    assert!(input.frames[0].key.process.is_none());
    let mut budget = WorkBudget::new(100_000_000);
    let output = KnownProcessSummaries
        .admit_relative(
            &input,
            &input,
            PolicySite::Body,
            Acceleration::Join,
            &mut PolicyContext::new(values.borrowed(), &entry, &mut budget),
        )
        .unwrap();
    assert_eq!(output.frames[0].stack.len(), 2);
    assert!(output.frames[0].stack_aliases.same_class(0, 1));
    assert!(!output.frames[0].stack[0].contains(U256::from(17)));
}
