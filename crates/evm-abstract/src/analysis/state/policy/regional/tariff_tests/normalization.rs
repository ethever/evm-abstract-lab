//! The former staged path is an accounting/control comparison, not an EVM oracle.

use super::*;
use crate::analysis::state::{bounded, payload};

fn staged<const Q: usize, const C: usize>(
    input: &EvmPayloadSummary<Word<Q, C>>,
    layout: &EvmPayloadSummary<Word<Q, C>>,
    coarse: bool,
    domain: crate::domain::AlgebraRef<'_, Algebra<Q, C>>,
    budget: &mut WorkBudget,
) -> Result<EvmPayloadSummary<Word<Q, C>>, MachineError> {
    let mut output = input.clone_with_payment(budget)?;
    output.store = output
        .store
        .project_to_layout_with_payment(&layout.store, domain, budget)?;
    for (frame, shape) in output.frames.iter_mut().zip(&layout.frames) {
        for (bytes, shape) in frame.byte_arrays_mut().into_iter().zip(shape.byte_arrays()) {
            *bytes = bytes.project_to_layout_with_payment(shape, domain, budget)?;
        }
        frame.saved_store =
            frame
                .saved_store
                .project_to_layout_with_payment(&shape.saved_store, domain, budget)?;
    }
    if coarse {
        output.store = output
            .store
            .forget_loop_numeric_with_payment(domain, budget)?;
        let scalar = |value: &mut EvmValue<Word<Q, C>>, budget: &mut WorkBudget| {
            let (lower, upper) = value.interval().unsigned_bounds();
            *value = crate::domain::summary::unsigned_range(lower, upper, domain, budget)?;
            Ok::<_, MachineError>(())
        };
        for frame in &mut output.frames {
            for value in frame.values_mut() {
                scalar(value, budget)?;
            }
            for bytes in frame.byte_arrays_mut() {
                *bytes = bytes.forget_loop_content_with_payment(domain, budget)?;
            }
            frame.saved_store = frame
                .saved_store
                .forget_loop_numeric_with_payment(domain, budget)?;
        }
        for continuation in &mut output.continuations {
            scalar(&mut continuation.output_offset, budget)?;
            scalar(&mut continuation.output_size, budget)?;
        }
    }
    let result = bounded::erase_guarantees(output)?;
    bounded::reserve(budget, result.work_size())?;
    Ok(result)
}

#[test]
fn direct_normalization_preserves_private_checkpoints_and_removes_staging_copies() {
    let algebra = ValueAlgebra::new(Algebra::<64, 4096>);
    let domain = algebra.borrowed();
    let (_, mut input, _) = fixture(&algebra);
    let mut saved = input.store.instantiate(domain).unwrap();
    let owner = input.frames[0].key.address;
    let private = EvmValue::from_numeric(Word {
        facts: WordObservation::top(),
        excluded: Some(U256::from(23)),
    });
    saved.write_balance(owner, private.clone());
    input.frames[0].saved_store = saved.summary_projection(domain).unwrap();
    input.frames[0].memory = EvmByteArray::exact(&[1, 0, 2, 0])
        .summary_projection(domain)
        .unwrap();
    input.frames[0].returndata = EvmByteArray::exact(&[3, 4])
        .summary_projection(domain)
        .unwrap();
    input.frames.push(input.frames[0].clone());
    input.continuations.push(crate::analysis::EvmContinuation {
        return_block: Some(3),
        output_offset: private.clone(),
        output_size: private,
        creation: Some(crate::analysis::CreationContinuation::Native { destination: owner }),
    });
    input.location.frames.push(input.frames[1].key.clone());
    input.work = input.measure_work();
    let original = input.clone();
    for coarse in [false, true] {
        let mut previous = WorkBudget::new(100_000_000);
        let (expected, staged_calls) =
            measure(|| staged(&input, &input, coarse, domain, &mut previous));
        let expected = expected.unwrap();
        let mut budget = WorkBudget::new(100_000_000);
        let (actual, direct_calls) =
            measure(|| input.project_for_bound_with_payment(&input, coarse, domain, &mut budget));
        let actual = actual.unwrap();
        assert_eq!(actual, expected);
        assert!(direct_calls.clones < staged_calls.clones);
        assert!(budget.used() < previous.used());
        assert_eq!(input, original);
        if !coarse {
            assert!(!actual.frames[0].stack[0].contains(U256::from(17)));
            let checkpoint = actual.frames[0].saved_store.instantiate(domain).unwrap();
            assert!(!checkpoint.read_balance(owner).contains(U256::from(23)));
            assert_eq!(
                actual.continuations[0].output_offset.numeric().excluded,
                Some(U256::from(23))
            );
        }
        let exact = budget.used();
        input
            .project_for_bound_with_payment(&input, coarse, domain, &mut WorkBudget::new(exact))
            .unwrap();
        assert!(matches!(
            input.project_for_bound_with_payment(
                &input,
                coarse,
                domain,
                &mut WorkBudget::new(exact - 1)
            ),
            Err(MachineError::Work)
        ));
        assert_eq!(input, original);
        eprintln!(
            "bound normalization coarse={coarse}: staged={} direct={} clones={}->{}",
            previous.used(),
            exact,
            staged_calls.clones,
            direct_calls.clones
        );
    }
    // Both normalizers intentionally erase optional must-alias guarantees.
    let normalized = input
        .project_for_bound_with_payment(&input, false, domain, &mut WorkBudget::new(100_000_000))
        .unwrap();
    assert!(!normalized.frames[0].stack_aliases.same_class(0, 1));
    assert!(payload::instantiate(&normalized, domain).is_ok());
}

#[test]
fn direct_normalization_honors_refusal_and_both_cancellation_owners() {
    let algebra = ValueAlgebra::new(Algebra::<64, 64>);
    let (_, input, _) = fixture(&algebra);
    let before = input.clone();
    for coarse in [false, true] {
        let (result, receipt) = measure(|| {
            input.project_for_bound_with_payment(
                &input,
                coarse,
                algebra.borrowed(),
                &mut WorkBudget::new(0),
            )
        });
        assert!(matches!(result, Err(MachineError::Work)));
        assert_eq!(receipt.clones, 0);
        assert_eq!(receipt.singletons, 0);
        for captured in [false, true] {
            let cancellation = analysis_control::Cancellation::new();
            let mut budget = if captured {
                analysis_control::with_cancellation(&cancellation, || WorkBudget::new(100_000_000))
            } else {
                WorkBudget::new(100_000_000)
            };
            cancellation.cancel();
            let (result, receipt) = measure(|| {
                let mut run = || {
                    input.project_for_bound_with_payment(
                        &input,
                        coarse,
                        algebra.borrowed(),
                        &mut budget,
                    )
                };
                if captured {
                    run()
                } else {
                    analysis_control::with_cancellation(&cancellation, run)
                }
            });
            assert!(matches!(
                result,
                Err(MachineError::Relations(
                    crate::domain::relational::QueryReason::Cancelled
                ))
            ));
            assert_eq!(budget.used(), 0);
            assert_eq!(receipt.clones, 0);
            assert_eq!(receipt.singletons, 0);
            assert_eq!(input, before);
        }
    }
}
