use super::*;
use crate::{
    domain::{
        DomainSpec, NumericValue, Profile, provenance::Origin, relational::RelationLimits,
        symbolic::StateError,
    },
    world::bytes::{limits, summary::ByteArraySummary, summary::Content},
};
use std::{num::NonZeroUsize, sync::Arc};

fn policy(profile: Profile, capacity: usize) -> Domain {
    Domain::from_spec(DomainSpec::new(
        profile,
        NonZeroUsize::new(capacity).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(256).unwrap(),
    ))
}

fn finite(values: impl IntoIterator<Item = u64>) -> AbstractValue {
    let domain = policy(Profile::Product, 64);
    let value = values
        .into_iter()
        .map(|value| AbstractValue::constant(U256::from(value)))
        .reduce(|left, right| domain.join(&left, &right))
        .unwrap();
    scalar(&value, domain)
}

fn cells(length: AbstractValue, values: BTreeMap<usize, AbstractValue>) -> ByteArraySummary {
    ByteArraySummary {
        length,
        memory: false,
        content: Content::Cells(values),
    }
}

// Both passes share the very same fresh source, length and binding identities.
// Restore from ORIGINAL cells, never from the optimized presentation under test.
fn compare(
    mut fresh: ByteArray,
    original: BTreeMap<usize, AbstractValue>,
    bare_cells: bool,
    domain: Domain,
    expected_reads: usize,
) -> ByteArray {
    let mut reference = fresh.clone();
    fresh.bytes = original.clone();
    reference.bytes = original.clone();
    let mut reads = 0;
    refresh_with(
        &mut fresh,
        bare_cells,
        domain,
        |root, index, facts, domain| {
            reads += 1;
            numeric::read(root, index, facts, domain)
        },
    )
    .unwrap();
    for (index, previous) in &original {
        let value = numeric::read(
            &reference.root,
            &ExprId::constant(U256::from(*index)),
            &reference.operands,
            domain,
        );
        let value = restore_presentation(value, Some(previous));
        reference.bytes.insert(*index, scalar(&value, domain));
    }
    assert_eq!(reads, expected_reads);
    // Includes complete AV metadata, root/bindings, length, operands, defaults,
    // memory mode and the remaining executable-array metadata.
    assert_eq!(fresh, reference);
    fresh
}

fn check(summary: &ByteArraySummary, domain: Domain, expected_reads: usize) -> ByteArray {
    let fresh = summary.instantiate(domain).unwrap();
    let (original, bare) = match &summary.content {
        Content::Cells(cells) => (
            cells.clone(),
            !cells.values().any(|cell| {
                cell.singleton()
                    .is_some_and(|value| value <= U256::from(255))
            }),
        ),
        _ => (fresh.bytes.clone(), false),
    };
    let refreshed = compare(fresh.clone(), original, bare, domain, expected_reads);
    assert_eq!(refreshed, fresh);
    refreshed
}

#[test]
fn definite_bounds_reuse_full_carriers_before_restoration_and_current_policy() {
    let dense = finite(2..=31)
        .with_origin(Origin::Balance)
        .with_identity(10, 1)
        .with_expression(ExprId::fresh().unwrap())
        .with_symbolic_limit();
    let values = BTreeMap::from([
        (0, dense),
        (2, finite([1, 5, 9])), // Holey, nonzero, known bits and mod-4 congruence.
        (5, finite([0, 1, 2, 3, 8, 9, 10, 11])),
    ]);
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        for capacity in [64, 8] {
            let domain = policy(profile, capacity);
            for memory in [false, true] {
                let mut summary = cells(finite([4]), values.clone());
                summary.memory = memory;
                let out = check(&summary, domain, 0);
                assert_eq!(out.bytes[&5].singleton(), Some(U256::ZERO));
                assert_eq!(out.default, AbstractValue::unknown_byte());
                assert_eq!(out.memory, memory);
                assert_eq!(
                    out.byte_at(1, domain).numeric(),
                    AbstractValue::unknown_byte().numeric()
                );
                let ByteStateKind::Input { bindings, .. } = out.root.kind() else {
                    panic!("bare input")
                };
                if profile == Profile::Product {
                    assert!(bindings[0].numeric.constants().is_none());
                    assert_eq!(bindings[1].numeric, *values[&2].numeric());
                }
                for value in out.bytes.values() {
                    assert_eq!(value, &AbstractValue::from_numeric(value.numeric().clone()));
                }
            }
        }
    }
}

#[test]
fn straddling_and_unknown_lengths_use_reader_while_definite_cells_skip_it() {
    let values = BTreeMap::from([
        (0, finite([2, 3])),
        (2, finite([1, 5, 9])),
        (4, finite(4..=7)),
    ]);
    for length in [
        finite([2, 4]),
        AbstractValue::unsigned_range(U256::from(2), U256::from(4)).unwrap(),
    ] {
        check(
            &cells(length, values.clone()),
            policy(Profile::Product, 64),
            1,
        );
    }
    check(
        &cells(AbstractValue::top(), values.clone()),
        policy(Profile::Product, 64),
        3,
    );
    check(&cells(finite([0]), values), policy(Profile::Product, 64), 0);
    // Compact Q joined with zero is structurally different from joining old F.
    let straddle = cells(finite([0, 1]), BTreeMap::from([(0, finite([2, 3]))]));
    let out = check(&straddle, policy(Profile::Product, 64), 1);
    assert!(out.bytes[&0].constants().is_none());
    assert!(out.bytes[&0].contains(U256::from(1)));
}

#[test]
fn current_binding_and_instantiated_length_environment_are_authoritative() {
    let domain = policy(Profile::Product, 8);
    let original = BTreeMap::from([(0, finite(0..=31))]);
    let summary = cells(finite([1, 2]), original.clone());
    let mut fresh = summary.instantiate(domain).unwrap();
    let ByteStateKind::Input {
        source,
        length,
        bindings,
    } = fresh.root.kind()
    else {
        panic!("bare input")
    };
    let mut bindings = bindings.clone();
    // Old finite is redundant, but its other components are not current facts.
    let carrier = NumericValue::unsigned_range(U256::from(4), U256::from(7)).unwrap();
    bindings[0].numeric = carrier.clone();
    fresh.root = ByteStateId::observed(
        source.clone(),
        length.clone(),
        bindings,
        limits(domain).into(),
    )
    .unwrap();
    let refreshed = compare(fresh.clone(), original.clone(), true, domain, 0);
    let expected = scalar(
        &restore_presentation(AbstractValue::from_numeric(carrier), original.get(&0)),
        domain,
    );
    assert_eq!(refreshed.bytes[&0], expected);
    fresh
        .operands
        .insert(fresh.length.expression().unwrap().clone(), finite([0]));
    let out = compare(fresh, original, true, domain, 0);
    assert_eq!(out.bytes[&0].singleton(), Some(U256::ZERO));
}

#[test]
fn chunks_exact_zero_and_nonbyte_content_keep_the_original_reader_paths() {
    let domain = policy(Profile::Product, 64);
    let mixed = BTreeMap::from([(0, finite([7])), (1, finite(2..=7)), (2, finite([0]))]);
    for length in [finite([0]), finite([3]), finite([1, 3])] {
        check(&cells(length, mixed.clone()), domain, mixed.len());
    }
    // Preserve constructor history even if the actual shape is a bare Input.
    let original = BTreeMap::from([(0, finite([2, 3]))]);
    let fresh = cells(finite([1]), original.clone())
        .instantiate(domain)
        .unwrap();
    compare(fresh, original, false, domain, 1);
    for value in [AbstractValue::top(), finite([256, 257]), finite([300])] {
        check(&cells(finite([1]), BTreeMap::from([(0, value)])), domain, 1);
    }
    for content in [Content::Zero, Content::Exact(Arc::from([7u8, 0, 9]))] {
        let summary = ByteArraySummary {
            length: finite([0]),
            memory: false,
            content,
        };
        let reads = if matches!(summary.content, Content::Exact(_)) {
            2
        } else {
            0
        };
        let out = check(&summary, domain, reads);
        assert_eq!(out.default, scalar(&zero(), domain));
        if matches!(summary.content, Content::Exact(_)) {
            assert_eq!(out.bytes[&0].singleton(), Some(U256::from(7)));
        }
    }
}

#[test]
fn missing_bindings_nonfresh_bindings_and_unsupported_roots_fall_back() {
    let domain = policy(Profile::Product, 64);
    let original = BTreeMap::from([(0, finite([2, 3]))]);
    let fresh = cells(finite([2]), original.clone())
        .instantiate(domain)
        .unwrap();
    let ByteStateKind::Input {
        source,
        length,
        bindings,
    } = fresh.root.kind()
    else {
        panic!("bare input")
    };
    let mut nonfresh = bindings.clone();
    nonfresh[0].value = ExprId::constant(U256::from(2));
    let mut missing = bindings.clone();
    missing[0].index = U256::from(1);
    for root in [
        ByteStateId::zero(),
        ByteStateId::exact(Arc::from([9u8])),
        ByteStateId::input(source.clone(), length.clone()),
        ByteStateId::observed(
            source.clone(),
            length.clone(),
            nonfresh,
            limits(domain).into(),
        )
        .unwrap(),
        ByteStateId::observed(
            source.clone(),
            length.clone(),
            missing,
            limits(domain).into(),
        )
        .unwrap(),
        fresh
            .root
            .slice(
                ExprId::constant(U256::from(1)),
                ExprId::constant(U256::from(1)),
                limits(domain).into(),
            )
            .unwrap(),
    ] {
        let mut malformed = fresh.clone();
        malformed.root = root;
        compare(malformed, original.clone(), true, domain, 1);
    }
}

#[test]
fn original_retention_and_cancellation_errors_remain_checked() {
    let domain = policy(Profile::Product, 64);
    let summary = cells(finite([1]), BTreeMap::from([(0, finite(2..=31))]));
    let frozen = summary.clone();
    let tiny = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_nodes: 1,
        ..domain.spec().relations()
    }));
    assert!(matches!(
        summary.instantiate(tiny),
        Err(RangeError::State(StateError::Limit { .. }))
    ));
    let mut fresh = summary.instantiate(domain).unwrap();
    let saved = fresh.clone();
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            summary.instantiate(domain),
            Err(RangeError::State(StateError::Cancelled))
        );
        let mut reads = 0;
        let result = refresh_with(&mut fresh, true, domain, |root, index, facts, domain| {
            reads += 1;
            numeric::read(root, index, facts, domain)
        });
        assert_eq!(result, Err(RangeError::Expression(ExprError::Cancelled)));
        assert_eq!(reads, 0);
    });
    assert_eq!(fresh, saved);
    assert_eq!(summary, frozen);
}
