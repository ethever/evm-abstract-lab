//! Exact immutable handles and contradictory implications stay scoped.

use super::super::{Branch, ConditionalFacts, Facts, Scope};
use alloy_primitives::U256;
use std::sync::Arc;
use {
    crate::domain::relational::EvmValueQuery, crate::domain::relational::QueryReason,
    crate::domain::relational::RelationLimits, crate::domain::relational::RelationState,
    crate::domain::symbolic::ByteStateKind, crate::domain::symbolic::ExprId,
    crate::resource::WorkBudget, crate::world::test_support::TestAlgebra,
    crate::world::test_support::TestBytes, crate::world::test_support::TestValue,
};

fn v(n: usize) -> TestValue {
    TestValue::constant(U256::from(n))
}

#[test]
fn a_stale_source_cannot_narrow_a_new_copy_and_a_contradictory_arm_keeps_its_sibling() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let limits = RelationLimits::default().expressions();
    let stale = TestBytes::unknown();
    let current = TestBytes::unknown();
    let current_root = current.state_root().clone();
    let mut before = TestBytes::memory();
    before
        .copy_from(
            &v(32),
            &TestBytes::exact(&[0xaa; 32]),
            &v(0),
            &v(32),
            domain.borrowed(),
        )
        .unwrap();
    let mut memory = before.clone();
    memory
        .copy_return_data(&v(32), &current, &v(16), domain.borrowed())
        .unwrap();
    let history = memory.clone();
    let frozen = history
        .read_byte_checked(&v(33), domain.borrowed())
        .unwrap();
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
    let byte = memory.byte_at(33, domain.borrowed());
    assert!(byte.contains(U256::ZERO));
    assert!(byte.contains(U256::from(0xaa)));
    assert_eq!(
        history
            .read_byte_checked(&v(33), domain.borrowed())
            .unwrap(),
        frozen
    );
    // Even after a proof really applied to one source, a newly captured source
    // at the same numeric offsets has a different immutable length identity.
    let current_proof = v(1).with_expression(current.len().expression().unwrap().clone());
    assert_eq!(
        memory.import_length_bound(&current_proof, limits, &mut budget),
        Ok(true)
    );
    assert_eq!(
        memory.byte_at(33, domain.borrowed()).singleton(),
        Some(U256::from(0xaa))
    );
    let replacement = TestBytes::unknown();
    memory
        .copy_return_data(&v(32), &replacement, &v(16), domain.borrowed())
        .unwrap();
    assert!(memory.byte_at(33, domain.borrowed()).contains(U256::ZERO));
    assert_eq!(current.state_root(), &current_root);

    let selector = ExprId::fresh().unwrap();
    let mut impossible_return = TestBytes::unknown();
    impossible_return.length = impossible_return.length.clone().with_numeric(
        TestValue::unsigned_range(U256::from(2), U256::MAX)
            .unwrap()
            .numeric()
            .clone(),
    );
    let possible_revert = TestBytes::unknown();
    let mut yes = before.clone();
    let mut no = before;
    yes.copy_return_data(&v(32), &impossible_return, &v(16), domain.borrowed())
        .unwrap();
    let ByteStateKind::Copy { length, .. } = yes.state_root().kind() else {
        panic!("return arm copy");
    };
    let yes_minimum = length.clone();
    no.copy_return_data(&v(32), &possible_revert, &v(16), domain.borrowed())
        .unwrap();
    let returned = impossible_return
        .join_guarded(
            &possible_revert,
            domain.borrowed(),
            selector.clone(),
            limits,
        )
        .unwrap();
    let mut joined = yes
        .join_guarded(&no, domain.borrowed(), selector.clone(), limits)
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
    let context = super::super::Context::new(
        &joined.operands,
        Some(&joined.conditional),
        domain.borrowed(),
    );
    let yes = context.branch(&selector, true);
    let no = context.branch(&selector, false);
    assert!(
        !super::super::super::numeric::scalar_in(&yes_minimum, &yes, domain.borrowed())
            .contains(U256::ONE)
    );
    assert_eq!(
        no.value(
            possible_revert.len().expression().unwrap(),
            domain.borrowed()
        )
        .unwrap()
        .singleton(),
        Some(U256::ONE)
    );
    assert!(
        joined
            .refine_guards(domain.borrowed(), limits, &mut |condition| Ok::<_, ()>(
                (condition == &selector).then_some(false)
            ))
            .unwrap()
    );
    assert_eq!(
        joined.byte_at(33, domain.borrowed()).singleton(),
        Some(U256::from(0xaa))
    );
    assert!(!budget.exhausted());
}

fn nested_length_case() -> (TestBytes, TestValue, TestBytes, ExprId) {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let limits = RelationLimits::default().expressions();
    let [s, t] = std::array::from_fn::<_, 2, _>(|_| ExprId::fresh().unwrap());
    let source = TestBytes::unknown();
    let mut old = TestBytes::memory();
    for (offset, byte) in [(32, 0xaa), (64, 0xbb)] {
        old.write_word(
            &v(offset),
            &TestValue::constant(U256::from_be_bytes([byte; 32])),
            domain.borrowed(),
        )
        .unwrap();
    }
    let mut copied = old.clone();
    copied
        .copy_return_data(&v(32), &source, &v(16), domain.borrowed())
        .unwrap();
    let mut memory = old
        .join_guarded(&copied, domain.borrowed(), t.clone(), limits)
        .unwrap()
        .join_guarded(&old, domain.borrowed(), s.clone(), limits)
        .unwrap();
    let q = ExprId::ite(s.clone(), t.clone(), ExprId::constant(U256::ONE), limits).unwrap();
    let ByteStateKind::Ite { condition, .. } = memory.state_root().kind() else {
        panic!("factored content selector");
    };
    assert_eq!(condition, &q);
    // The scalar length retains its original two selectors: R=I(s,I(t,0,L),0).
    let returned = v(0)
        .join_guarded(source.len(), domain.borrowed(), t, limits)
        .unwrap()
        .join_guarded(&v(0), domain.borrowed(), s, limits)
        .unwrap();
    memory
        .write_word(&v(96), &returned, domain.borrowed())
        .unwrap();
    let output = memory.slice(&v(32), &v(96), domain.borrowed()).unwrap();
    (output, returned, source, q)
}

#[test]
fn nested_length_proof_follows_the_derived_content_selector_without_changing_history() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let limits = RelationLimits::default();
    let (mut output, returned, source, q) = nested_length_case();
    let source_snapshot = source.clone();
    let history = output.clone();
    let frozen = history.read_byte_checked(&v(1), domain.borrowed()).unwrap();
    assert!(frozen.contains(U256::ZERO));
    assert!(frozen.contains(U256::from(0xaa)));
    let expression = returned.expression().unwrap();
    let mut relations = RelationState::default();
    relations
        .observe(expression, v(1).numeric(), &limits)
        .unwrap();
    let EvmValueQuery::Exact(length) =
        relations.local_value(expression, &limits, domain.borrowed())
    else {
        panic!("the caller proves the whole returned length");
    };
    assert_eq!(length, U256::ONE);
    let proof = returned.with_numeric(TestValue::constant(length).numeric().clone());
    let mut budget = WorkBudget::new(1_000_000);
    assert_eq!(
        output.import_length_bound(&proof, limits.expressions(), &mut budget),
        Ok(true)
    );
    assert_eq!(output.state_root(), history.state_root());
    assert_eq!(output.len(), history.len());
    assert_eq!(output.fresh_leaves(), history.fresh_leaves());
    let context = super::super::Context::new(
        &output.operands,
        Some(&output.conditional),
        domain.borrowed(),
    );
    let source_length = source.len().expression().unwrap();
    assert_eq!(
        context
            .branch(&q, false)
            .value(source_length, domain.borrowed())
            .unwrap()
            .singleton(),
        Some(U256::ONE)
    );
    assert!(context.value(source_length, domain.borrowed()).is_none());
    assert!(
        context
            .branch(&q, true)
            .value(source_length, domain.borrowed())
            .is_none()
    );
    for at in 1..32 {
        assert_eq!(
            output.byte_at(at, domain.borrowed()).singleton(),
            Some(U256::from(0xaa))
        );
    }
    for at in 32..64 {
        assert_eq!(
            output.byte_at(at, domain.borrowed()).singleton(),
            Some(U256::from(0xbb))
        );
    }
    assert_eq!(
        output
            .read_word_checked(&v(64), domain.borrowed())
            .unwrap()
            .singleton(),
        Some(U256::ONE)
    );
    let once = output.clone();
    assert_eq!(
        output.import_length_bound(&proof, limits.expressions(), &mut budget),
        Ok(true)
    );
    assert_eq!(output, once);
    assert_eq!(
        history.read_byte_checked(&v(1), domain.borrowed()).unwrap(),
        frozen
    );
    assert_eq!(source, source_snapshot);

    // The whole-R prefix and its selected old-source leaf prove nothing about
    // a newly captured source, even at the same numeric copy coordinates.
    let replacement = TestBytes::unknown();
    assert_ne!(replacement.len().expression(), source.len().expression());
    output
        .copy_return_data(&v(0), &replacement, &v(16), domain.borrowed())
        .unwrap();
    let byte = output.read_byte_checked(&v(1), domain.borrowed()).unwrap();
    assert!(byte.contains(U256::ZERO));
    assert!(byte.contains(U256::from(0xaa)));
    let ByteStateKind::Copy { source: view, .. } = output.state_root().kind() else {
        panic!("new immutable source copy");
    };
    assert_eq!(&view.state, replacement.state_root());
    assert_eq!(source, source_snapshot);
    assert!(!budget.exhausted());
}

#[test]
fn optional_length_normalization_preserves_admission_stamps_and_cancellation() {
    let limits = RelationLimits::default().expressions();
    let (_, returned, _, _) = nested_length_case();
    let proof = returned.with_numeric(v(1).numeric().clone());
    let original = TestBytes::memory();
    let mut imported = original.clone();
    let mut paid = WorkBudget::new(super::IMPORT_WORK);
    assert_eq!(
        imported.import_length_bound(&proof, limits, &mut paid),
        Ok(true)
    );
    assert!(paid.used() > 0);
    assert!(!paid.exhausted());
    // Exact original reservation funds the bounded normalization too. A failed
    // optional admission neither publishes a partial stamp nor exhausts Work.
    for capacity in [0, paid.used() - 1, paid.used()] {
        let mut memory = original.clone();
        let mut budget = WorkBudget::new(capacity);
        let admitted = capacity == paid.used();
        assert_eq!(
            memory.import_length_bound(&proof, limits, &mut budget),
            Ok(admitted)
        );
        assert_eq!(&memory, if admitted { &imported } else { &original });
        assert_eq!(budget.used(), if admitted { capacity } else { 0 });
        assert!(!budget.exhausted());
    }
    // A previously complete unfactored stamp stays idempotent as well.
    let mut bound = proof.clone();
    bound.forget_identity();
    bound.forget_expression();
    let mut scope = super::distribute(proof.expression().unwrap(), &bound);
    scope
        .facts
        .insert(proof.expression().unwrap().clone(), bound);
    let mut legacy = original.clone();
    legacy.conditional = ConditionalFacts::from_branches(vec![Arc::new(Branch {
        condition: ExprId::constant(U256::ONE),
        then_facts: scope,
        else_facts: super::empty(),
    })]);
    let before = legacy.clone();
    let mut budget = WorkBudget::new(super::IMPORT_WORK);
    assert_eq!(
        legacy.import_length_bound(&proof, limits, &mut budget),
        Ok(true)
    );
    assert_eq!(legacy, before);
    // Guard growth fits the bare implication tree but not its eventual true
    // wrapper. Optional normalization must retain the original import here.
    let selector = ExprId::ite(
        ExprId::fresh().unwrap(),
        ExprId::fresh().unwrap(),
        ExprId::fresh().unwrap(),
        limits,
    )
    .unwrap();
    let nested = ExprId::ite(
        ExprId::fresh().unwrap(),
        ExprId::constant(U256::ZERO),
        ExprId::fresh().unwrap(),
        limits,
    )
    .unwrap();
    let expression = ExprId::ite(
        selector.clone(),
        nested,
        ExprId::constant(U256::ZERO),
        limits,
    )
    .unwrap();
    let shallow = crate::domain::symbolic::ExprLimits {
        max_depth: expression.depth() + 1,
        ..limits
    };
    let mut refused = original.clone();
    let mut budget = WorkBudget::new(super::IMPORT_WORK);
    assert_eq!(
        refused.import_length_bound(&v(1).with_expression(expression), shallow, &mut budget),
        Ok(true)
    );
    assert_eq!(
        refused.conditional.branches[0]
            .then_facts
            .conditional
            .branches[0]
            .condition,
        selector
    );
    refused.conditional.check(shallow).unwrap();
    let once = imported.clone();
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        let mut budget = WorkBudget::new(super::IMPORT_WORK);
        assert_eq!(
            imported.import_length_bound(&proof, limits, &mut budget),
            Err(QueryReason::Cancelled)
        );
        assert_eq!(budget.used(), 0);
    });
    assert_eq!(imported, once);
    let mut budget = WorkBudget::new(super::IMPORT_WORK);
    assert_eq!(
        imported.import_length_bound(&proof, limits, &mut budget),
        Ok(true)
    );
    assert_eq!(imported.conditional.branches.len(), 1);
}
