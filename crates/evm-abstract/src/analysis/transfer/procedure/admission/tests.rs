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
    fixture_with_data(fee, &EvmByteArray::empty())
}

fn fixture_with_data(
    fee: usize,
    data: &EvmByteArray<Word>,
) -> (ValueAlgebra<SummaryPolicy>, EvmEntry<Word>, EvmOutput<Word>) {
    let (world, entry, _, algebra) = summary_fixture("00");
    let mut setup = WorkBudget::new(100_000_000);
    let mut payload = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    payload
        .store
        .write_balance(entry.address, EvmValue::from_numeric(Word::expensive(fee)));
    let incoming = ProcedureOutput::project(
        data,
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

fn copy_invoice(summary: &ProcedureOutput<Word>) -> usize {
    let mut quotation = WorkBudget::new(100_000_000);
    let work = summary.clone_work_with(&mut quotation).unwrap();
    assert_eq!((RECEIPT.get().clones, RECEIPT.get().bytes), (0, 0));
    quotation.used() + work
}

fn retained_balance_copy_bytes(
    original: &ProcedureOutput<Word>,
    retained: &ProcedureOutput<Word>,
    entry: &EvmEntry<Word>,
) -> usize {
    let original = original.store().view().balances[&entry.address].numeric();
    let retained = retained.store().view().balances[&entry.address].numeric();
    assert_eq!(original.private.len(), 4096);
    assert_eq!(retained.private, original.private);
    let shared = std::ptr::eq(original, retained);
    // Check the actual retained entry and its allocation, not just a
    // feature-selected counter: imbl shares immutable nodes; std copies them.
    assert_eq!(shared, cfg!(feature = "imbl"));
    if shared {
        assert_eq!(original.private.as_ptr(), retained.private.as_ptr());
        0
    } else {
        assert_ne!(original.private.as_ptr(), retained.private.as_ptr());
        original.private.len()
    }
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
    // Setup is complete. Quotation itself must not execute a native clone.
    reset();
    let invoice = copy_invoice(&old.summary);
    assert!(invoice > 1_000_000);
    assert_eq!(RECEIPT.get().private_quotes, 1);
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
    assert_eq!(RECEIPT.get().clones, 0);
    reset();
    let mut budget = WorkBudget::new(100_000_000);
    let output = prepare_output(
        &KnownProcessSummaries,
        None,
        &old.summary,
        Acceleration::Join,
        false,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
    )
    .unwrap()
    .expect("the initial output requires an owned retained summary");
    let receipt = RECEIPT.get();
    assert_eq!(receipt.private_quotes, 1);
    assert!(receipt.clones > 0);
    assert_eq!(
        receipt.bytes,
        retained_balance_copy_bytes(&old.summary, &output.summary, &entry)
    );
    // The current invoice conservatively funds a deep map copy on either
    // backend, even when immutable nodes make that native copy unnecessary.
    assert_eq!(budget.used(), invoice + old.work);
    assert_eq!(output.summary, old.summary);
    assert_eq!((output.generation, output.updates), (1, 0));
}

#[test]
fn invalidation_still_pays_the_retained_copy_and_changes_only_delivery_generation() {
    let _scope = RECEIPT.scope();
    let (algebra, entry, old) = fixture(1_000_000);
    reset();
    let mut proof = WorkBudget::new(100_000_000);
    assert!(
        old.summary
            .eq_with_payment(&old.summary, algebra.borrowed(), &mut proof)
            .unwrap()
    );
    let invoice = copy_invoice(&old.summary);
    assert!(invoice > 1_000_000);
    assert_eq!(RECEIPT.get().private_quotes, 1);
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
    assert_eq!(RECEIPT.get().clones, 0);

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
    let receipt = RECEIPT.get();
    assert_eq!(receipt.private_quotes, 1);
    assert!(receipt.clones > 0);
    assert_eq!(
        receipt.bytes,
        retained_balance_copy_bytes(&old.summary, &output.summary, &entry)
    );
    assert_eq!(budget.used(), proof.used() + 8 + invoice + old.work * 2 + 1);
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
fn retained_owned_scalar_refuses_underpayment_before_cloning_on_both_backends() {
    let _scope = RECEIPT.scope();
    // Exact empty content is zero-padded. Give its independently owned length
    // a private predicate; projection discards the unused byte default. Unlike
    // a balance map entry, the retained length cannot share a persistent node.
    let mut data = EvmByteArray::empty();
    data.update_values(&mut |value| {
        *value = EvmValue::from_numeric(Word::expensive(1_000_000));
    });
    let (algebra, entry, old) = fixture_with_data(64, &data);
    let length = old.summary.data().len().numeric();
    assert_eq!(length.private.len(), 4096);
    reset();
    let invoice = copy_invoice(&old.summary);
    assert!(invoice > 1_000_000);
    assert_eq!(RECEIPT.get().private_quotes, 2);

    for invalidate in [false, true] {
        let current = invalidate.then_some(&old);
        reset();
        let mut refused = WorkBudget::new(invoice - 1);
        assert!(matches!(
            prepare_output(
                &KnownProcessSummaries,
                current,
                &old.summary,
                Acceleration::Join,
                invalidate,
                &mut PolicyContext::new(algebra.borrowed(), &entry, &mut refused),
            ),
            Err(MachineError::Work)
        ));
        assert!(refused.exhausted());
        assert_eq!(RECEIPT.get().private_quotes, 2);
        assert_eq!((RECEIPT.get().clones, RECEIPT.get().bytes), (0, 0));
        assert_eq!((old.generation, old.updates), (7, 5));

        reset();
        let mut budget = WorkBudget::new(100_000_000);
        let output = prepare_output(
            &KnownProcessSummaries,
            current,
            &old.summary,
            Acceleration::Join,
            invalidate,
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap()
        .expect("first publication and invalidation retain an owned scalar");
        let receipt = RECEIPT.get();
        let retained_length = output.summary.data().len().numeric();
        assert_eq!(receipt.private_quotes, 2);
        assert_ne!(length.private.as_ptr(), retained_length.private.as_ptr());
        assert_eq!(retained_length.private, length.private);
        assert_eq!(
            receipt.bytes,
            length.private.len()
                + retained_balance_copy_bytes(&old.summary, &output.summary, &entry)
        );
        assert_eq!(output.summary, old.summary);
        assert_eq!(
            (output.generation, output.updates),
            if invalidate { (8, 5) } else { (1, 0) }
        );
        assert_eq!((old.generation, old.updates), (7, 5));
    }
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

#[test]
fn covered_non_equal_process_output_keeps_private_predicate_and_required_publication() {
    let _scope = RECEIPT.scope();
    let (algebra, entry, old) = fixture(1_000_000);
    let mut setup = WorkBudget::new(100_000_000);
    let mut store = old
        .summary
        .store()
        .instantiate_with_payment(algebra.borrowed(), &mut setup)
        .unwrap();
    store.write_balance(
        crate::Address::repeat_byte(99),
        EvmValue::constant(U256::ZERO),
    );
    let incoming = ProcedureOutput::project(
        &EvmByteArray::empty(),
        &store,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut setup),
    )
    .unwrap();
    assert_ne!(old.summary, incoming);
    assert!(
        old.summary
            .store()
            .covers_with_payment(incoming.store(), algebra.borrowed(), &mut setup)
            .unwrap()
    );
    reset();
    let mut budget = WorkBudget::new(500_000);
    assert!(
        prepare_output(
            &KnownProcessSummaries,
            Some(&old),
            &incoming,
            Acceleration::Join,
            false,
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget)
        )
        .unwrap()
        .is_none()
    );
    assert_eq!((RECEIPT.get().private_quotes, RECEIPT.get().bytes), (0, 0));
    assert_eq!((old.generation, old.updates), (7, 5));
    reset();
    assert!(matches!(
        prepare_output(
            &KnownProcessSummaries,
            Some(&old),
            &incoming,
            Acceleration::Join,
            true,
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut WorkBudget::new(500_000))
        ),
        Err(MachineError::Work)
    ));
    assert_eq!((RECEIPT.get().private_quotes, RECEIPT.get().bytes), (1, 0));
    let calls = Cell::new(0);
    reset();
    assert!(matches!(
        prepare_output(
            &Materializing(&calls),
            Some(&old),
            &incoming,
            Acceleration::Join,
            false,
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut WorkBudget::new(500_000))
        ),
        Err(MachineError::Work)
    ));
    assert_eq!(calls.get(), 1);
    assert_eq!((RECEIPT.get().private_quotes, RECEIPT.get().bytes), (1, 0));
    let mut budget = WorkBudget::new(100_000_000);
    let retained = prepare_output(
        &KnownProcessSummaries,
        Some(&old),
        &incoming,
        Acceleration::Join,
        true,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
    )
    .unwrap()
    .unwrap();
    assert_eq!((retained.generation, retained.updates), (8, 5));
    let store = retained
        .summary
        .store()
        .instantiate_with_payment(algebra.borrowed(), &mut budget)
        .unwrap();
    assert!(!store.read_balance(entry.address).contains(U256::from(7)));
    assert!(store.read_balance(entry.address).contains(U256::from(8)));
    assert!(
        KnownProcessSummaries
            .admit_output_change(
                Some(&old.summary),
                &incoming,
                Acceleration::Widen,
                &mut PolicyContext::new(
                    algebra.borrowed(),
                    &entry,
                    &mut WorkBudget::new(100_000_000)
                )
            )
            .unwrap()
            .is_some(),
        "non-equal widening must retain its selected hook"
    );
}
