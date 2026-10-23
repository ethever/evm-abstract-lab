use super::*;
use crate::analysis::{StructuralPartition, machine::fixtures, transfer};

#[test]
fn relative_owned_preparation_checks_location_history_and_preserves_refused_input() {
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let algebra = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let initial = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let mut setup = WorkBudget::new(20_000_000);
    let mut input = RelativeState::project(
        &initial,
        &mut PolicyContext::new(algebra.borrowed(), &entry, &mut setup),
    )
    .unwrap();
    input.location.frames[0].jump_history = vec![111];
    assert!(input.frames[0].key.jump_history.is_empty());
    let policy = Selective {
        inner: &StructuralPartition,
        regions: &[],
    };
    let mut denied = WorkBudget::new(0);
    let refusal = policy
        .prepare_relative_owned(
            input.clone(),
            PolicySite::Body,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut denied),
        )
        .unwrap_err();
    assert_eq!(refusal.input.key().frames[0].jump_history, vec![111]);
    assert!(refusal.input.active_frame().jump_history.is_empty());
    let prepared = policy
        .prepare_relative_owned(
            input,
            PolicySite::Body,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut setup),
        )
        .unwrap();
    assert!(prepared.payload.key().frames[0].jump_history.is_empty());
    assert!(prepared.payload.active_frame().jump_history.is_empty());
    assert!(prepared.key.0.frames[0].jump_history.is_empty());
}
