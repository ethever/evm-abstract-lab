use super::super::*;
use crate::{
    domain::{identity::Symbol, provenance::Origin, relational::RelationLimits},
    resource::WorkBudget,
};

fn policy() -> RelationLimits {
    RelationLimits {
        max_nodes: 65_536,
        max_depth: 512,
        max_constraints: 2048,
        ..RelationLimits::default()
    }
}

fn domain() -> TestAlgebra {
    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
            .spec()
            .with_relations(policy()),
    ))
}

// Keep the complete pre-change wrapper around the independent recursive root
// oracle so equality covers all AV facts, presentation and captured history.
fn reference_copy_from(
    target: &mut TestBytes,
    destination: &TestValue,
    source: &TestBytes,
    offset: &TestValue,
    size: &TestValue,
    domain: TestAlgebra,
) -> Result<(), RangeError> {
    if size.singleton() == Some(U256::ZERO) {
        return Ok(());
    }
    if let Some(error) = target.failure.as_ref().or(source.failure.as_ref()) {
        return Err(error.clone());
    }
    let destination = &ranges::normalized(destination)?;
    let offset = &ranges::normalized(offset)?;
    let size = &ranges::normalized(size)?;
    let mut out = target.clone();
    out.expand(destination, size, domain.borrowed())?;
    let dst = expr(destination)?;
    let at = expr(offset)?;
    let len = expr(size)?;
    if out.operands.is_empty() {
        out.operands = operands::import_empty(
            &source.operands,
            domain.borrowed(),
            limits(domain.borrowed()),
        )?;
    } else {
        for (expression, value) in &source.operands {
            out.remember(expression.clone(), value, domain.borrowed())?;
        }
    }
    out.conditional
        .extend(&source.conditional, limits(domain.borrowed()))?;
    out.remember(dst.clone(), destination, domain.borrowed())?;
    out.remember(at.clone(), offset, domain.borrowed())?;
    out.remember(len.clone(), size, domain.borrowed())?;
    let view = ByteView::new(source.root.clone(), at, len.clone());
    out.root = out
        .root
        .copy_recanonicalizing(dst, view, len, limits(domain.borrowed()).into())?;
    out.bytes = super::project(target, source, destination, offset, size, &out)?;
    out.default = TestValue::unknown_byte();
    *target = out;
    Ok(())
}

fn reference_return(
    target: &mut TestBytes,
    destination: &TestValue,
    source: &TestBytes,
    requested: &TestValue,
    domain: TestAlgebra,
) -> Result<(), RangeError> {
    let destination = &ranges::normalized(destination)?;
    let requested = &ranges::normalized(requested)?;
    if requested.singleton() == Some(U256::ZERO) {
        return Ok(());
    }
    let mut out = target.clone();
    out.expand(destination, requested, domain.borrowed())?;
    let copied = ranges::minimum(requested, source.len(), domain.borrowed())?;
    reference_copy_from(&mut out, destination, source, &zero(), &copied, domain)?;
    *target = out;
    Ok(())
}

fn memory(scope: u64, number: usize, domain: TestAlgebra) -> TestBytes {
    let mut value = TestValue::top();
    value.set_origin(Origin::Calldata);
    let value = value
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
        .with_numeric(
            constant::<crate::domain::NumericValue>(number)
                .numeric()
                .clone(),
        );
    let mut out = TestBytes::memory();
    out.write_word(&zero(), &value, domain.borrowed()).unwrap();
    out
}

fn selected(leaves: usize, domain: TestAlgebra) -> TestBytes {
    let mut out = memory(800, 7, domain);
    for leaf in 1..leaves {
        let other = memory(800 + leaf as u64, leaf + 7, domain);
        out = out
            .join_guarded(
                &other,
                domain.borrowed(),
                ExprId::fresh().unwrap(),
                limits(domain.borrowed()),
            )
            .unwrap();
    }
    assert_eq!(out.root.copy_leaves(), leaves);
    out
}

fn compound(value: usize, domain: TestAlgebra) -> TestValue {
    constant(value).with_expression(
        ExprId::operation(
            opcode::ADD,
            &[ExprId::fresh().unwrap(), ExprId::constant(U256::ONE)],
            limits(domain.borrowed()),
        )
        .unwrap(),
    )
}

#[test]
fn return_copy_matches_old_full_values_for_nineteen_leaves_and_compound_ranges() {
    let domain = domain();
    let source = memory(999, 0x1122_3344, domain)
        .slice(&zero(), &constant(32), domain.borrowed())
        .unwrap();
    assert!(matches!(source.root.kind(), ByteStateKind::Slice { .. }));
    let saved_source = source.clone();
    for leaves in [1, 19] {
        for compound_ranges in [false, true] {
            let original = selected(leaves, domain);
            let history = original
                .read_word_checked(&zero(), domain.borrowed())
                .unwrap();
            let destination = if compound_ranges {
                compound(64, domain)
            } else {
                constant(64)
            };
            let requested = if compound_ranges {
                compound(32, domain)
            } else {
                constant(32)
            };
            let mut actual = original.clone();
            let mut expected = original.clone();
            let actual_result =
                actual.copy_return_data(&destination, &source, &requested, domain.borrowed());
            let expected_result =
                reference_return(&mut expected, &destination, &source, &requested, domain);
            assert_eq!(actual_result, expected_result);
            assert_eq!(actual, expected);
            assert_eq!(source, saved_source);
            assert_eq!(
                original
                    .read_word_checked(&zero(), domain.borrowed())
                    .unwrap(),
                history
            );
            for offset in [zero(), constant(64)] {
                assert_eq!(
                    actual.read_word_checked(&offset, domain.borrowed()),
                    expected.read_word_checked(&offset, domain.borrowed())
                );
            }
            // Repeating the update exercises complete old-Copy view equality.
            actual
                .copy_return_data(&destination, &source, &requested, domain.borrowed())
                .unwrap();
            reference_return(&mut expected, &destination, &source, &requested, domain).unwrap();
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn return_copy_preserves_real_limits_cancellation_and_unpaid_staging() {
    let domain = domain();
    let original = selected(2, domain);
    let source = memory(999, 0x1122, domain);
    let destination = constant(64);
    let requested = constant(32);
    let fee = original.copy_return_data_work(&source, &destination, &requested, domain.borrowed());
    let mut budget = WorkBudget::new(fee - 1);
    let mut unpaid = original.clone();
    let admitted = budget
        .charge(fee)
        .then(|| unpaid.copy_return_data(&destination, &source, &requested, domain.borrowed()));
    assert!(admitted.is_none());
    assert_eq!(budget.used(), 0);
    assert!(budget.exhausted());
    assert_eq!(unpaid, original);

    for restricted in [
        RelationLimits {
            max_nodes: original.root.nodes(),
            ..policy()
        },
        RelationLimits {
            max_depth: 1,
            ..policy()
        },
    ] {
        let restricted = crate::domain::ValueAlgebra::new(
            crate::domain::ProductAlgebra::from_spec(domain.spec().with_relations(restricted)),
        );
        let mut actual = original.clone();
        let mut expected = original.clone();
        let actual_error =
            actual.copy_return_data(&destination, &source, &requested, restricted.borrowed());
        let expected_error =
            reference_return(&mut expected, &destination, &source, &requested, restricted);
        assert!(actual_error.is_err());
        assert_eq!(actual_error, expected_error);
        assert_eq!(actual, original);
        assert_eq!(expected, original);
    }
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        let mut actual = original.clone();
        let mut expected = original.clone();
        let mut cancelled = WorkBudget::new(fee);
        assert!(!cancelled.charge(fee));
        assert_eq!(cancelled.used(), 0);
        assert_eq!(
            actual.copy_return_data(&destination, &source, &requested, domain.borrowed()),
            reference_return(&mut expected, &destination, &source, &requested, domain)
        );
        assert_eq!(actual, original);
        assert_eq!(expected, original);
    });
    let mut invalid = source.clone();
    invalid.failure = Some(RangeError::UnknownSize);
    let mut zero_request = original.clone();
    zero_request
        .copy_return_data(&destination, &invalid, &zero(), domain.borrowed())
        .unwrap();
    assert_eq!(zero_request, original);
}

#[test]
fn complex_source_growth_is_paid_once_plus_additive_leaf_fields() {
    let domain = domain();
    let target = selected(19, domain);
    let source = |nodes| super::SourceWork {
        bytes: 0,
        operands: 0,
        allocations: 0,
        conditional: 0,
        conditional_work: 0,
        nodes,
    };
    // Adjacent sizes keep projection's search height equal, isolating input
    // growth in the source once-envelope and the per-occurrence field bound.
    let small = super::work(&target, source(62), 516, 1, domain.borrowed());
    let large = super::work(&target, source(63), 516, 1, domain.borrowed());
    assert_eq!(large - small, 8 * (516 + 19));
    assert!(large - small < 8 * 19 * 516);

    let mut compound_source = memory(999, 7, domain);
    compound_source.length = compound(32, domain);
    let requested = compound(32, domain);
    let copied = ranges::minimum(&requested, compound_source.len(), domain.borrowed()).unwrap();
    assert_eq!(copied.singleton(), Some(U256::from(32)));
    assert!(expr(&copied).unwrap().as_constant().is_none());
    let destination = compound(0, domain);
    let mut actual = target.clone();
    let mut expected = target;
    actual
        .copy_return_data(
            &destination,
            &compound_source,
            &requested,
            domain.borrowed(),
        )
        .unwrap();
    reference_return(
        &mut expected,
        &destination,
        &compound_source,
        &requested,
        domain,
    )
    .unwrap();
    assert_eq!(actual, expected);
}
