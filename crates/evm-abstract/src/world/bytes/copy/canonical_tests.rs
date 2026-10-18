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

fn domain() -> Domain {
    Domain::from_spec(Domain::default().spec().with_relations(policy()))
}

// Keep the complete pre-change wrapper around the independent recursive root
// oracle so equality covers all AV facts, presentation and captured history.
fn reference_copy_from(
    target: &mut ByteArray,
    destination: &AbstractValue,
    source: &ByteArray,
    offset: &AbstractValue,
    size: &AbstractValue,
    domain: Domain,
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
    out.expand(destination, size, 4096, domain)?;
    let dst = expr(destination)?;
    let at = expr(offset)?;
    let len = expr(size)?;
    if out.operands.is_empty() {
        out.operands = operands::import_empty(&source.operands, domain, limits(domain))?;
    } else {
        for (expression, value) in &source.operands {
            out.remember(expression.clone(), value, domain)?;
        }
    }
    out.conditional
        .extend(&source.conditional, limits(domain))?;
    out.remember(dst.clone(), destination, domain)?;
    out.remember(at.clone(), offset, domain)?;
    out.remember(len.clone(), size, domain)?;
    let view = ByteView::new(source.root.clone(), at, len.clone());
    out.root = out
        .root
        .copy_recanonicalizing(dst, view, len, limits(domain).into())?;
    out.bytes = super::project(target, source, destination, offset, size, &out)?;
    out.default = AbstractValue::unknown_byte();
    *target = out;
    Ok(())
}

fn reference_return(
    target: &mut ByteArray,
    destination: &AbstractValue,
    source: &ByteArray,
    requested: &AbstractValue,
    domain: Domain,
) -> Result<(), RangeError> {
    let destination = &ranges::normalized(destination)?;
    let requested = &ranges::normalized(requested)?;
    if requested.singleton() == Some(U256::ZERO) {
        return Ok(());
    }
    let mut out = target.clone();
    out.expand(destination, requested, 4096, domain)?;
    let copied = ranges::minimum(requested, source.len(), domain)?;
    reference_copy_from(&mut out, destination, source, &zero(), &copied, domain)?;
    *target = out;
    Ok(())
}

fn memory(scope: u64, number: usize, domain: Domain) -> ByteArray {
    let mut value = AbstractValue::top();
    value.set_origin(Origin::Calldata);
    let value = value
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
        .with_numeric(constant(number).numeric().clone());
    let mut out = ByteArray::memory();
    out.write_word(&zero(), &value, 4096, domain).unwrap();
    out
}

fn selected(leaves: usize, domain: Domain) -> ByteArray {
    let mut out = memory(800, 7, domain);
    for leaf in 1..leaves {
        let other = memory(800 + leaf as u64, leaf + 7, domain);
        out = out
            .join_guarded(&other, domain, ExprId::fresh().unwrap(), limits(domain))
            .unwrap();
    }
    assert_eq!(out.root.copy_leaves(), leaves);
    out
}

fn compound(value: usize, domain: Domain) -> AbstractValue {
    constant(value).with_expression(
        ExprId::operation(
            opcode::ADD,
            &[ExprId::fresh().unwrap(), ExprId::constant(U256::ONE)],
            limits(domain),
        )
        .unwrap(),
    )
}

#[test]
fn return_copy_matches_old_full_values_for_nineteen_leaves_and_compound_ranges() {
    let domain = domain();
    let source = memory(999, 0x1122_3344, domain)
        .slice(&zero(), &constant(32), 4096, domain)
        .unwrap();
    assert!(matches!(source.root.kind(), ByteStateKind::Slice { .. }));
    let saved_source = source.clone();
    for leaves in [1, 19] {
        for compound_ranges in [false, true] {
            let original = selected(leaves, domain);
            let history = original.read_word_checked(&zero(), domain).unwrap();
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
                actual.copy_return_data(&destination, &source, &requested, 4096, domain);
            let expected_result =
                reference_return(&mut expected, &destination, &source, &requested, domain);
            assert_eq!(actual_result, expected_result);
            assert_eq!(actual, expected);
            assert_eq!(source, saved_source);
            assert_eq!(
                original.read_word_checked(&zero(), domain).unwrap(),
                history
            );
            for offset in [zero(), constant(64)] {
                assert_eq!(
                    actual.read_word_checked(&offset, domain),
                    expected.read_word_checked(&offset, domain)
                );
            }
            // Repeating the update exercises complete old-Copy view equality.
            actual
                .copy_return_data(&destination, &source, &requested, 4096, domain)
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
    let fee = original.copy_return_data_work(&source, &destination, &requested, domain);
    let mut budget = WorkBudget::new(fee - 1);
    let mut unpaid = original.clone();
    let admitted = budget
        .charge(fee)
        .then(|| unpaid.copy_return_data(&destination, &source, &requested, 4096, domain));
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
        let restricted = Domain::from_spec(domain.spec().with_relations(restricted));
        let mut actual = original.clone();
        let mut expected = original.clone();
        let actual_error =
            actual.copy_return_data(&destination, &source, &requested, 4096, restricted);
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
            actual.copy_return_data(&destination, &source, &requested, 4096, domain),
            reference_return(&mut expected, &destination, &source, &requested, domain)
        );
        assert_eq!(actual, original);
        assert_eq!(expected, original);
    });
    let mut invalid = source.clone();
    invalid.failure = Some(RangeError::UnknownSize);
    let mut zero_request = original.clone();
    zero_request
        .copy_return_data(&destination, &invalid, &zero(), 4096, domain)
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
    let small = super::work(&target, source(62), 516, 1, domain);
    let large = super::work(&target, source(63), 516, 1, domain);
    assert_eq!(large - small, 8 * (516 + 19));
    assert!(large - small < 8 * 19 * 516);

    let mut compound_source = memory(999, 7, domain);
    compound_source.length = compound(32, domain);
    let requested = compound(32, domain);
    let copied = ranges::minimum(&requested, compound_source.len(), domain).unwrap();
    assert_eq!(copied.singleton(), Some(U256::from(32)));
    assert!(expr(&copied).unwrap().as_constant().is_none());
    let destination = compound(0, domain);
    let mut actual = target.clone();
    let mut expected = target;
    actual
        .copy_return_data(&destination, &compound_source, &requested, 4096, domain)
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
