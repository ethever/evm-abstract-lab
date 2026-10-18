//! Coverage may discard an incoming alternative only after a complete proof.

use super::{MachineError, covers, same_byte_roots, shape};
use crate::{
    analysis::{Continuation, MachinePayload, RootFrame, machine::fixtures},
    domain::{
        AbstractValue, Domain,
        relational::{CheckResult, QueryReason},
        symbolic::ExprId,
    },
    resource::WorkBudget,
    world::ByteArray,
};
use alloy_primitives::U256;
use analysis_control::{Cancellation, with_cancellation};
use revm_bytecode::opcode;

fn pair(equal: bool) -> (Domain, MachinePayload, MachinePayload, ExprId, ExprId) {
    let config = fixtures::config();
    let domain = config.domain().unwrap();
    let old = ExprId::fresh().unwrap();
    let new = ExprId::fresh().unwrap();
    let mut previous = fixtures::payload(domain);
    previous.active_mut().stack = vec![AbstractValue::top().with_expression(old.clone())];
    let mut incoming = previous.clone();
    incoming.active_mut().stack = vec![AbstractValue::top().with_expression(new.clone())];
    if equal {
        assert_eq!(
            incoming
                .relations
                .add_equal(&old, &new, &domain.spec().relations()),
            CheckResult::MayBeFeasible
        );
    }
    (domain, previous, incoming, old, new)
}

#[test]
fn direct_coverage_proofs_pay_local_work_and_preserve_both_inputs() {
    let (domain, previous, incoming, _, _) = pair(true);
    let old_previous = previous.clone();
    let old_incoming = incoming.clone();
    let mut budget = WorkBudget::new(100_000_000);
    for _ in 0..2 {
        let before = budget.used();
        assert!(covers(&previous, &incoming, domain, &mut budget).unwrap());
        assert!(budget.used() > before);
        assert_eq!(previous, old_previous);
        assert_eq!(incoming, old_incoming);
        assert!(!budget.exhausted());
    }
}

#[test]
fn unknown_coverage_keeps_the_complete_incoming_alternative() {
    let (domain, previous, incoming, old, new) = pair(false);
    let equality = ExprId::operation(
        opcode::EQ,
        &[old.clone(), new.clone()],
        domain.spec().relations().expressions(),
    )
    .unwrap();
    assert_eq!(
        incoming
            .relations
            .can_equal(&equality, U256::ZERO, &domain.spec().relations()),
        CheckResult::MayBeFeasible
    );
    let before_previous = previous.clone();
    let before_incoming = incoming.clone();
    let mut budget = WorkBudget::new(100_000_000);
    assert!(!covers(&previous, &incoming, domain, &mut budget).unwrap());

    let mut alternatives = super::super::EntryAlternatives::new(&previous);
    let joined = alternatives
        .join(&previous, &incoming, domain, &mut budget)
        .unwrap()
        .expect("an undecided coverage check must retain the incoming alternative");
    let retained = joined.fresh_leaves();
    assert!(retained.contains(&old));
    assert!(retained.contains(&new));
    assert_eq!(previous, before_previous);
    assert_eq!(incoming, before_incoming);

    assert!(!budget.exhausted());
}

#[test]
fn guardless_and_cancelled_coverage_cannot_reuse_a_warm_proof() {
    let (domain, previous, incoming, _, _) = pair(true);
    let mut budget = WorkBudget::new(100_000_000);
    assert!(covers(&previous, &incoming, domain, &mut budget).unwrap());

    let mut guardless = previous.clone();
    guardless.active_mut().stack[0].forget_expression();
    guardless.active_mut().stack[0].forget_identity();
    let before_guardless = guardless.clone();
    let before_incoming = incoming.clone();
    assert!(!covers(&guardless, &incoming, domain, &mut budget).unwrap());

    let token = Cancellation::new();
    token.cancel();
    with_cancellation(&token, || {
        assert_eq!(
            covers(&previous, &incoming, domain, &mut budget),
            Err(MachineError::Relations(QueryReason::Cancelled))
        );
    });

    assert_eq!(guardless, before_guardless);
    assert_eq!(incoming, before_incoming);
    let mut empty = WorkBudget::new(0);
    assert_eq!(
        covers(&previous, &incoming, domain, &mut empty),
        Err(MachineError::Work)
    );
    assert_eq!(empty.used(), 0);
    assert!(empty.exhausted());
}

#[test]
fn every_frame_byte_plane_mismatch_rejects_before_the_full_shape_fee() {
    let (domain, mut previous, _, _, _) = pair(true);
    let child = RootFrame {
        state: previous.active().clone(),
    }
    .into_child(Continuation {
        return_block: Some(1),
        output_offset: fixtures::word(0),
        output_size: fixtures::word(32),
        creation: None,
    });
    previous.call_stack.push_child(child);
    for frame_index in 0..2 {
        for plane in 0..3 {
            let mut incoming = previous.clone();
            let frame = incoming.call_stack.iter_mut().nth(frame_index).unwrap();
            match plane {
                0 => frame
                    .memory
                    .write_word(&fixtures::word(0), &fixtures::word(7), 4096, domain)
                    .unwrap(),
                1 => frame.calldata = ByteArray::exact(&[7]),
                _ => frame.returndata = ByteArray::exact(&[7]),
            }
            let before = (previous.clone(), incoming.clone());
            // Compare against both original normalization paths, including
            // their independent saved-Store clones and structural Eq wrappers.
            assert_ne!(shape(&previous).unwrap().0, shape(&incoming).unwrap().0);
            assert_ne!(
                previous.content_form().unwrap(),
                incoming.content_form().unwrap()
            );
            let mut budget = WorkBudget::new(8);
            assert!(!covers(&previous, &incoming, domain, &mut budget).unwrap());
            assert_eq!(budget.used(), 8);
            assert!(!budget.exhausted());
            assert_eq!((&previous, &incoming), (&before.0, &before.1));
        }
    }
    let mut shorter = previous.clone();
    shorter.call_stack.pop_child().unwrap();
    let mut metadata_only = WorkBudget::new(2);
    assert!(!covers(&previous, &shorter, domain, &mut metadata_only).unwrap());
    assert_eq!(metadata_only.used(), 2);
    assert!(!metadata_only.exhausted());
}

#[test]
fn matching_byte_roots_still_pay_for_and_compare_the_complete_shape() {
    let (domain, previous, mut incoming, _, _) = pair(true);
    incoming.active_mut().environment_calldata = !previous.active().environment_calldata;
    let mut roots = WorkBudget::new(5);
    assert!(same_byte_roots(&previous, &incoming, &mut roots).unwrap());
    let full_fee = previous
        .work_size()
        .saturating_add(incoming.work_size())
        .saturating_mul(4);
    let before = (previous.clone(), incoming.clone());
    let mut short = WorkBudget::new(5 + full_fee - 1);
    assert_eq!(
        covers(&previous, &incoming, domain, &mut short),
        Err(MachineError::Work)
    );
    assert_eq!(short.used(), 5);
    assert!(short.exhausted());
    let mut paid = WorkBudget::new(5 + full_fee);
    assert!(!covers(&previous, &incoming, domain, &mut paid).unwrap());
    assert_eq!(paid.used(), 5 + full_fee);
    assert!(!paid.exhausted());
    assert_eq!((&previous, &incoming), (&before.0, &before.1));
}

#[test]
fn byte_root_rejection_requires_payment_and_preserves_typed_interruptions() {
    let (domain, previous, mut incoming, _, _) = pair(true);
    incoming.active_mut().returndata = ByteArray::exact(&[7]);
    let before = (previous.clone(), incoming.clone());
    for maximum in [0, 1, 2, 4] {
        let mut budget = WorkBudget::new(maximum);
        assert_eq!(
            covers(&previous, &incoming, domain, &mut budget),
            Err(MachineError::Work)
        );
        assert_eq!(budget.used(), if maximum < 2 { 0 } else { 2 });
        assert!(budget.exhausted());
    }
    let mut exhausted = WorkBudget::new(100);
    assert!(!exhausted.charge(101));
    assert_eq!(
        covers(&previous, &incoming, domain, &mut exhausted),
        Err(MachineError::Work)
    );
    assert_eq!(exhausted.used(), 0);
    let mut saturated = WorkBudget::new(usize::MAX);
    assert_eq!(
        super::reserve(&mut saturated, usize::MAX),
        Err(MachineError::Work)
    );
    assert_eq!(saturated.used(), 0);

    // The warm budget captured a different token: current cancellation must
    // still win before an otherwise provable negative result is returned.
    let mut warm = WorkBudget::new(100);
    let token = Cancellation::new();
    token.cancel();
    with_cancellation(&token, || {
        assert_eq!(
            covers(&previous, &incoming, domain, &mut warm),
            Err(MachineError::Relations(QueryReason::Cancelled))
        );
    });
    assert_eq!((&previous, &incoming), (&before.0, &before.1));
}
