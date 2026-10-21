//! Default identity routing moves complete inputs; refusals retain them.

use crate::{
    analysis::{
        engine::policy::{MachinePartition, PolicyContext, PolicySite, StructuralPartition},
        machine::{MachineError, fixtures},
        transfer,
    },
    domain::{ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
};

#[test]
fn default_owned_preparation_moves_payload_and_preserves_it_on_refusal() {
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let mut input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    input.active_mut().stack = vec![fixtures::word(17)];
    let pointer = input.active().stack.as_ptr();
    let mut budget = WorkBudget::new(1_000_000);
    let prepared = StructuralPartition
        .prepare_owned(
            input,
            PolicySite::World,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap();
    assert_eq!(pointer, prepared.payload.active().stack.as_ptr());
    assert_eq!(prepared.key.0, prepared.payload.key());
    assert!(budget.used() > 0);

    let mut budget = WorkBudget::new(0);
    let refused = StructuralPartition
        .prepare_owned(
            prepared.payload,
            PolicySite::World,
            (),
            &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
        )
        .unwrap_err();
    assert_eq!(refused.error, MachineError::Work);
    assert_eq!(pointer, refused.input.active().stack.as_ptr());
    assert_eq!(refused.input.active().stack, vec![fixtures::word(17)]);
}
