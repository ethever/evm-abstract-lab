//! Exact immutable handles and contradictory implications stay scoped.

use super::super::{Branch, ConditionalFacts, Facts, Scope};
use crate::{
    domain::{
        AbstractValue, Domain,
        relational::RelationLimits,
        symbolic::{ByteStateKind, ExprId},
    },
    resource::WorkBudget,
    world::ByteArray,
};
use alloy_primitives::U256;
use std::sync::Arc;

fn v(n: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(n))
}

#[test]
fn a_stale_source_cannot_narrow_a_new_copy_and_a_contradictory_arm_keeps_its_sibling() {
    let domain = Domain::default();
    let limits = RelationLimits::default().expressions();
    let stale = ByteArray::unknown();
    let current = ByteArray::unknown();
    let current_root = current.state_root().clone();
    let mut before = ByteArray::memory();
    before
        .copy_from(
            &v(32),
            &ByteArray::exact(&[0xaa; 32]),
            &v(0),
            &v(32),
            128,
            domain,
        )
        .unwrap();
    let mut memory = before.clone();
    memory
        .copy_return_data(&v(32), &current, &v(16), 128, domain)
        .unwrap();
    let history = memory.clone();
    let frozen = history.read_byte_checked(&v(33), domain).unwrap();
    let mut budget = WorkBudget::new(1_000_000);
    let proof = v(1).with_expression(stale.len().expression().unwrap().clone());
    assert_eq!(
        memory.import_length_bound(&proof, limits, &mut budget),
        Ok(true)
    );
    assert_eq!(memory.state_root(), history.state_root());
    let ByteStateKind::Copy { source, length, .. } = memory.state_root().kind() else {
        panic!("CALL output copy");
    };
    assert_eq!(source.state, current_root);
    assert_eq!(&source.length, length);
    let ByteStateKind::Input { length, .. } = source.state.kind() else {
        panic!("original immutable source");
    };
    assert_eq!(Some(length), current.len().expression());
    let byte = memory.byte_at(33, domain);
    assert!(byte.contains(U256::ZERO));
    assert!(byte.contains(U256::from(0xaa)));
    assert_eq!(history.read_byte_checked(&v(33), domain).unwrap(), frozen);
    // Even after a proof really applied to one source, a newly captured source
    // at the same numeric offsets has a different immutable length identity.
    let current_proof = v(1).with_expression(current.len().expression().unwrap().clone());
    assert_eq!(
        memory.import_length_bound(&current_proof, limits, &mut budget),
        Ok(true)
    );
    assert_eq!(
        memory.byte_at(33, domain).singleton(),
        Some(U256::from(0xaa))
    );
    let replacement = ByteArray::unknown();
    memory
        .copy_return_data(&v(32), &replacement, &v(16), 128, domain)
        .unwrap();
    assert!(memory.byte_at(33, domain).contains(U256::ZERO));
    assert_eq!(current.state_root(), &current_root);

    let selector = ExprId::fresh().unwrap();
    let mut impossible_return = ByteArray::unknown();
    impossible_return.length = impossible_return.length.clone().with_numeric(
        AbstractValue::unsigned_range(U256::from(2), U256::MAX)
            .unwrap()
            .numeric()
            .clone(),
    );
    let possible_revert = ByteArray::unknown();
    let mut yes = before.clone();
    let mut no = before;
    yes.copy_return_data(&v(32), &impossible_return, &v(16), 128, domain)
        .unwrap();
    let ByteStateKind::Copy { length, .. } = yes.state_root().kind() else {
        panic!("return arm copy");
    };
    let yes_minimum = length.clone();
    no.copy_return_data(&v(32), &possible_revert, &v(16), 128, domain)
        .unwrap();
    let returned = impossible_return
        .join_guarded(&possible_revert, domain, selector.clone(), limits)
        .unwrap();
    let mut joined = yes
        .join_guarded(&no, domain, selector.clone(), limits)
        .unwrap();
    let proof = returned.len().clone().with_numeric(v(1).numeric().clone());
    // A pre-existing unconditional whole-R fact is not evidence that its
    // guarded arm implications were imported. It must not suppress this pass.
    let whole_only = ConditionalFacts::from_branches(vec![Arc::new(Branch {
        condition: ExprId::constant(U256::ONE),
        then_facts: Scope {
            facts: Facts::from([(proof.expression().unwrap().clone(), v(1))]),
            conditional: ConditionalFacts::default(),
        },
        else_facts: super::empty(),
    })]);
    joined.conditional.extend(&whole_only, limits).unwrap();
    assert_eq!(
        joined.import_length_bound(&proof, limits, &mut budget),
        Ok(true)
    );
    let context = super::super::Context::new(&joined.operands, Some(&joined.conditional));
    let yes = context.branch(&selector, true);
    let no = context.branch(&selector, false);
    assert!(
        !super::super::super::numeric::scalar_in(&yes_minimum, &yes, domain).contains(U256::ONE)
    );
    assert_eq!(
        no.value(possible_revert.len().expression().unwrap(), domain)
            .unwrap()
            .singleton(),
        Some(U256::ONE)
    );
    assert!(
        joined
            .refine_guards(domain, limits, &mut |condition| Ok::<_, ()>(
                (condition == &selector).then_some(false)
            ))
            .unwrap()
    );
    assert_eq!(
        joined.byte_at(33, domain).singleton(),
        Some(U256::from(0xaa))
    );
    assert!(!budget.exhausted());
}
