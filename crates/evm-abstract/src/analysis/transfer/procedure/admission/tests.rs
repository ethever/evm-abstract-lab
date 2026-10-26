//! Discarded output staging differs from an actually retained publication.

use super::*;
use crate::{
    U256,
    analysis::{KnownProcessSummaries, engine::policy::RelativeState, transfer},
    domain::{EvmValue, ValueAlgebra},
    resource::{
        WorkBudget,
        clone_payment::tests::{RECEIPT, SummaryPolicy, Word, reset, summary_fixture},
    },
    world::{EvmByteArray, EvmEntry},
};
use std::cell::Cell;

fn fixture(fee: usize) -> (ValueAlgebra<SummaryPolicy>, EvmEntry<Word>, EvmOutput<Word>) {
    let (world, entry, _, algebra) = summary_fixture("00");
    let mut setup = WorkBudget::new(100_000_000);
    let mut payload = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    payload
        .store
        .write_balance(entry.address, EvmValue::from_numeric(Word::expensive(fee)));
    let incoming = ProcedureOutput::project(
        &EvmByteArray::empty(),
        &payload.store,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut setup),
    )
    .unwrap();
    let summary = KnownProcessSummaries
        .admit_output(
            None,
            &incoming,
            Acceleration::Join,
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut setup),
        )
        .unwrap();
    let work = summary.work_size();
    (
        algebra,
        entry,
        EvmOutput {
            summary,
            work,
            generation: 7,
            updates: 5,
        },
    )
}

/// It deliberately inherits the default change-admission method. Exact equal
/// inputs cannot authorize bypassing this custom policy's own operation.
struct Materializing<'a>(&'a Cell<usize>);
impl ProcedureSummaryPolicy<SummaryPolicy> for Materializing<'_> {
    fn admit_relative(
        &self,
        current: &RelativeState<Word>,
        incoming: &RelativeState<Word>,
        site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, SummaryPolicy>,
    ) -> Result<RelativeState<Word>, MachineError> {
        KnownProcessSummaries.admit_relative(current, incoming, site, acceleration, cx)
    }
    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<Word>>,
        incoming: &ProcedureOutput<Word>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, SummaryPolicy>,
    ) -> Result<ProcedureOutput<Word>, MachineError> {
        self.0.set(self.0.get() + 1);
        KnownProcessSummaries.admit_output(current, incoming, acceleration, cx)
    }
}

#[test]
fn unchanged_output_proof_avoids_discarded_private_copy_but_preserves_custom_hooks() {
    let _scope = RECEIPT.scope();
    let (algebra, entry, old) = fixture(1_000_000);
    for acceleration in [Acceleration::Join, Acceleration::Widen] {
        reset();
        let mut budget = WorkBudget::new(500_000);
        let result = prepare_output(
            &KnownProcessSummaries,
            Some(&old),
            &old.summary,
            acceleration,
            false,
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap();
        assert!(result.is_none());
        assert_eq!((RECEIPT.get().private_quotes, RECEIPT.get().bytes), (0, 0));
        assert!(budget.used() > 0 && !budget.exhausted());
        assert_eq!((old.generation, old.updates), (7, 5));

        let calls = Cell::new(0);
        reset();
        assert!(matches!(
            prepare_output(
                &Materializing(&calls),
                Some(&old),
                &old.summary,
                acceleration,
                false,
                &mut PolicyContext::new(algebra.borrowed(), &entry, &mut WorkBudget::new(500_000)),
            ),
            Err(MachineError::Work)
        ));
        assert_eq!(calls.get(), 1);
        assert_eq!((RECEIPT.get().private_quotes, RECEIPT.get().bytes), (1, 0));
    }
}

#[test]
fn first_output_still_pays_for_its_actual_retained_native_copy() {
    let _scope = RECEIPT.scope();
    let (algebra, entry, old) = fixture(1_000_000);
    reset();
    assert!(matches!(
        prepare_output(
            &KnownProcessSummaries,
            None,
            &old.summary,
            Acceleration::Join,
            false,
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut WorkBudget::new(500_000)),
        ),
        Err(MachineError::Work)
    ));
    assert_eq!((RECEIPT.get().private_quotes, RECEIPT.get().bytes), (1, 0));
    reset();
    let output = prepare_output(
        &KnownProcessSummaries,
        None,
        &old.summary,
        Acceleration::Join,
        false,
        &mut PolicyContext::new(
            algebra.borrowed(),
            &entry,
            &mut WorkBudget::new(100_000_000),
        ),
    )
    .unwrap()
    .expect("the initial output requires an owned retained summary");
    assert_eq!(RECEIPT.get().bytes, 4096);
    assert_eq!(output.summary, old.summary);
    assert_eq!((output.generation, output.updates), (1, 0));
}

#[test]
fn invalidation_still_pays_the_retained_copy_and_changes_only_delivery_generation() {
    let _scope = RECEIPT.scope();
    let (algebra, entry, old) = fixture(1_000_000);
    reset();
    assert!(matches!(
        prepare_output(
            &KnownProcessSummaries,
            Some(&old),
            &old.summary,
            Acceleration::Join,
            true,
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut WorkBudget::new(500_000)),
        ),
        Err(MachineError::Work)
    ));
    assert_eq!((RECEIPT.get().private_quotes, RECEIPT.get().bytes), (1, 0));

    reset();
    let mut budget = WorkBudget::new(100_000_000);
    let output = prepare_output(
        &KnownProcessSummaries,
        Some(&old),
        &old.summary,
        Acceleration::Widen,
        true,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
    )
    .unwrap()
    .expect("actual invalidation installs a new owned output");
    assert_eq!(RECEIPT.get().bytes, 4096);
    assert_eq!(output.summary, old.summary);
    assert_eq!((output.generation, output.updates), (8, 5));
    assert_eq!((old.generation, old.updates), (7, 5));
    let retained = output
        .summary
        .store()
        .instantiate_with_payment(algebra.borrowed(), &mut budget)
        .unwrap();
    assert!(!retained.read_balance(entry.address).contains(U256::from(7)));
    assert!(retained.read_balance(entry.address).contains(U256::from(8)));
}

#[test]
fn equal_public_views_with_different_private_predicates_require_real_output_admission() {
    let _scope = RECEIPT.scope();
    let (algebra, entry, old) = fixture(64);
    let mut setup = WorkBudget::new(100_000_000);
    let mut store = old
        .summary
        .store()
        .instantiate_with_payment(algebra.borrowed(), &mut setup)
        .unwrap();
    let mut word = Word::expensive(64);
    word.private.fill(23);
    store.write_balance(entry.address, EvmValue::from_numeric(word));
    let incoming = ProcedureOutput::project(
        &EvmByteArray::empty(),
        &store,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut setup),
    )
    .unwrap();
    let output = prepare_output(
        &KnownProcessSummaries,
        Some(&old),
        &incoming,
        Acceleration::Join,
        false,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut setup),
    )
    .unwrap()
    .expect("different native predicates cannot certify an unchanged bound");
    assert_eq!((output.generation, output.updates), (8, 6));
    let joint = output
        .summary
        .store()
        .instantiate_with_payment(algebra.borrowed(), &mut setup)
        .unwrap();
    let balance = joint.read_balance(entry.address);
    assert!(balance.contains(U256::from(7)) && balance.contains(U256::from(23)));
}

#[test]
fn unchanged_output_proof_checks_both_cancellation_owners() {
    let _scope = RECEIPT.scope();
    let (algebra, entry, old) = fixture(64);
    for captured in [false, true] {
        let cancellation = analysis_control::Cancellation::new();
        let mut budget = if captured {
            analysis_control::with_cancellation(&cancellation, || WorkBudget::new(100_000_000))
        } else {
            WorkBudget::new(100_000_000)
        };
        cancellation.cancel();
        reset();
        let mut run = || {
            prepare_output(
                &KnownProcessSummaries,
                Some(&old),
                &old.summary,
                Acceleration::Join,
                false,
                &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
            )
        };
        let result = if captured {
            run()
        } else {
            analysis_control::with_cancellation(&cancellation, run)
        };
        assert!(matches!(
            result,
            Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled
            ))
        ));
        assert_eq!(budget.used(), 0);
        assert_eq!((RECEIPT.get().clones, RECEIPT.get().bytes), (0, 0));
        assert_eq!((old.generation, old.updates), (7, 5));
    }
}
