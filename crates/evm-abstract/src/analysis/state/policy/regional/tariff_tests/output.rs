//! Owned reuse of an already admitted Process output, not an EVM oracle.

use super::*;
use crate::analysis::KnownProcessSummaries;

#[test]
fn equal_process_output_keeps_private_predicate_with_less_actual_copy_work() {
    let values = ValueAlgebra::new(Algebra::<64, 4096>);
    let (entry, _, original) = fixture(&values);
    let mut setup = WorkBudget::new(100_000_000);
    let current = KnownProcessSummaries
        .admit_output(
            None,
            &original,
            Acceleration::Join,
            &mut PolicyContext::new(values.borrowed(), &entry, &mut setup),
        )
        .unwrap();
    for acceleration in [Acceleration::Join, Acceleration::Widen] {
        let mut reused_budget = WorkBudget::new(100_000_000);
        let (reused, copies) = measure(|| {
            KnownProcessSummaries.admit_output(
                Some(&current),
                &current,
                acceleration,
                &mut PolicyContext::new(values.borrowed(), &entry, &mut reused_budget),
            )
        });
        let reused = reused.unwrap();
        let mut full_budget = WorkBudget::new(100_000_000);
        let (full, full_copies) = measure(|| {
            current.regional_join(
                &current,
                acceleration,
                &mut PolicyContext::new(values.borrowed(), &entry, &mut full_budget),
            )
        });
        // Regional re-normalization may erase representation metadata. The
        // Process policy keeps its already admitted bound exactly; both paths
        // still preserve the independently chosen private exclusion below.
        assert_eq!(reused, current);
        let full = full.unwrap();
        assert!(reused_budget.used() < full_budget.used());
        assert!(copies.clones < full_copies.clones);
        for output in [reused, full] {
            assert!(output.data().len().contains(U256::ZERO));
            assert!(!output.data().len().contains(U256::from(1)));
            let restored = output.store().instantiate(values.borrowed()).unwrap();
            let balance = restored.read_balance(entry.address);
            assert!(!balance.contains(U256::from(17)));
            assert!(balance.numeric().observation().contains(U256::from(17)));
        }
    }
}

#[test]
fn equal_public_observation_does_not_reuse_different_private_process_outputs() {
    let values = ValueAlgebra::new(Algebra::<64, 64>);
    let (entry, _, original) = fixture(&values);
    let mut setup = WorkBudget::new(100_000_000);
    let current = KnownProcessSummaries
        .admit_output(
            None,
            &original,
            Acceleration::Join,
            &mut PolicyContext::new(values.borrowed(), &entry, &mut setup),
        )
        .unwrap();
    let mut store = current.store().instantiate(values.borrowed()).unwrap();
    store.write_balance(
        entry.address,
        EvmValue::from_numeric(Word {
            facts: WordObservation::top(),
            excluded: Some(U256::from(23)),
        }),
    );
    let incoming = ProcedureOutput::from_summaries(
        current.data.clone_with_payment(&mut setup).unwrap(),
        store.summary_projection(values.borrowed()).unwrap(),
        &mut PolicyContext::new(values.borrowed(), &entry, &mut setup),
    )
    .unwrap();
    assert_eq!(
        current
            .store()
            .instantiate(values.borrowed())
            .unwrap()
            .read_balance(entry.address)
            .numeric()
            .observation(),
        incoming
            .store()
            .instantiate(values.borrowed())
            .unwrap()
            .read_balance(entry.address)
            .numeric()
            .observation()
    );
    let admitted = KnownProcessSummaries
        .admit_output(
            Some(&current),
            &incoming,
            Acceleration::Join,
            &mut PolicyContext::new(values.borrowed(), &entry, &mut setup),
        )
        .unwrap();
    let balance = admitted
        .store()
        .instantiate(values.borrowed())
        .unwrap()
        .read_balance(entry.address);
    assert!(balance.contains(U256::from(17)) && balance.contains(U256::from(23)));
}

#[test]
fn equal_process_output_refuses_actual_retained_copy_before_native_clone() {
    let values = ValueAlgebra::new(Algebra::<64, 1_000_000>);
    let (entry, _, original) = fixture(&values);
    let mut setup = WorkBudget::new(1_000_000_000);
    let current = KnownProcessSummaries
        .admit_output(
            None,
            &original,
            Acceleration::Join,
            &mut PolicyContext::new(values.borrowed(), &entry, &mut setup),
        )
        .unwrap();
    let mut budget = WorkBudget::new(100_000);
    let (result, calls) = measure(|| {
        KnownProcessSummaries.admit_output(
            Some(&current),
            &current,
            Acceleration::Join,
            &mut PolicyContext::new(values.borrowed(), &entry, &mut budget),
        )
    });
    assert_eq!(result, Err(MachineError::Work));
    assert_eq!(calls.clones, 0);
    assert!(calls.clone_quotes > 0);
}
