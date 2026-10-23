use super::*;
use crate::analysis::state::projection::{Policy, Projection};

#[test]
fn direct_relation_projection_keeps_constraints_outside_the_observation_language() {
    let algebra = ValueAlgebra::new(Algebra::new());
    let domain = algebra.borrowed();
    let (entry, input) = fixture(domain);
    let expression = input.active().stack[0].expression().unwrap();
    let limits = domain.spec().relations();
    let mut relations = input.relations.clone();
    relations
        .observe(
            expression,
            &ExcludingWord::unsigned_range(U256::from(10), U256::from(20)).unwrap(),
            &limits,
        )
        .unwrap();
    let mut projected = input.clone();
    let mut budget = WorkBudget::new(usize::MAX);
    let mut projection = Projection::new(&relations, domain, &limits, &mut budget, Policy::Summary);
    projection.payload(&mut projected).unwrap();
    assert_eq!(projection.finish(), Ok(true));
    for scalar in projected.active().values() {
        excludes(scalar, 13);
        assert_eq!(
            scalar.interval().unsigned_bounds(),
            (U256::from(10), U256::from(20)),
        );
        assert_eq!(scalar.expression(), Some(expression));
    }
    assert!(algebra.numeric_algebra().reductions.get() > 0);
    assert!(budget.used() > 0);
    // The reduced executable state can become an independent whole-state
    // template without exporting its retained N through WordObservation.
    let bound = form_normalized(&projected, &entry, domain).unwrap();
    let replayed = instantiate(&bound, domain).unwrap();
    excludes(&replayed.active().stack[0], 13);
    assert!(!replayed.active().stack[0].contains(U256::from(9)));
    assert!(!replayed.active().stack[0].contains(U256::from(21)));
    assert_eq!(
        input.active().stack[0].interval().unsigned_bounds().0,
        U256::ZERO
    );
}

#[test]
fn ordinary_bound_import_preserves_the_actual_private_carrier() {
    let algebra = ValueAlgebra::new(Algebra::new());
    let domain = algebra.borrowed();
    let (entry, mut input) = fixture(domain);
    let expression = input.active().call_value.expression().unwrap().clone();
    input
        .relations
        .observe(
            &expression,
            &ExcludingWord::unsigned_range(U256::from(10), U256::from(20)).unwrap(),
            &domain.spec().relations(),
        )
        .unwrap();
    let original = input.clone();
    let mut budget = WorkBudget::new(20_000_000);
    let (_, admitted) =
        loop_bound::EvmLoopBound::new_scheduled(&input, &input, &entry, domain, &mut budget, false)
            .unwrap();
    for scalar in admitted.active().values() {
        excludes(scalar, 13);
        assert_eq!(
            scalar.interval().unsigned_bounds(),
            (U256::from(10), U256::from(20))
        );
    }
    assert!(admitted.relations.is_empty());
    assert_eq!(input, original);
    assert!(budget.used() > 0);
}

#[test]
fn equal_custom_bounds_alone_never_create_an_alias_or_a_replay_identity() {
    let algebra = ValueAlgebra::new(Algebra::new());
    let domain = algebra.borrowed();
    let (entry, mut input) = fixture(domain);
    input.active_mut().stack[1] = value(13);
    assert_eq!(
        input.active().stack[0].numeric(),
        input.active().stack[1].numeric()
    );
    let bound = form_normalized(&input, &entry, domain).unwrap();
    assert!(!bound.active().stack_aliases.same_class(0, 1));
    let first = instantiate(&bound, domain).unwrap();
    let second = instantiate(&bound, domain).unwrap();
    assert_ne!(
        first.active().stack[0].expression(),
        first.active().stack[1].expression()
    );
    assert_ne!(
        first.active().stack[0].expression(),
        second.active().stack[0].expression()
    );
    for replayed in [&first, &second] {
        assert!(semantic_eq(
            &bound,
            &form_normalized(replayed, &entry, domain).unwrap()
        ));
        replayed
            .active()
            .stack
            .iter()
            .for_each(|scalar| excludes(scalar, 13));
    }
}

#[test]
fn fresh_renaming_preserves_the_actual_carrier_and_one_policy_view() {
    let algebra = ValueAlgebra::new(Algebra::new());
    let domain = algebra.borrowed();
    let (entry, input) = fixture(domain);
    let mut renamed = input.clone();
    let old = input.active().stack[0].expression().unwrap().clone();
    let replacement = ExprId::fresh().unwrap();
    renamed
        .rename_fresh(
            &BTreeMap::from([(old.clone(), replacement.clone())]),
            domain.spec().relations().expressions(),
            domain.without_relations(),
        )
        .unwrap();
    for scalar in renamed.active().values() {
        excludes(scalar, 13);
        assert_eq!(scalar.expression(), Some(&replacement));
    }
    assert_eq!(input.active().stack[0].expression(), Some(&old));
    assert!(semantic_eq(
        &form_normalized(&input, &entry, domain).unwrap(),
        &form_normalized(&renamed, &entry, domain).unwrap(),
    ));
}
