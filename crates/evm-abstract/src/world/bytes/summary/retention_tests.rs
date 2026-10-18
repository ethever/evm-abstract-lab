//! Exercise real observed-root admission and the summary regeneration boundary.

use super::*;
use crate::domain::{
    DomainSpec, NumericValue, Profile, relational::RelationLimits, symbolic::StateLimits,
};
use std::{collections::BTreeSet, num::NonZeroUsize};

fn domain(profile: Profile) -> Domain {
    Domain::from_spec(
        DomainSpec::new(
            profile,
            NonZeroUsize::new(256).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        )
        .with_relations(RelationLimits {
            max_nodes: 16_384,
            max_depth: 256,
            ..RelationLimits::default()
        }),
    )
}

fn finite(domain: Domain, values: impl IntoIterator<Item = u64>) -> AbstractValue {
    values
        .into_iter()
        .map(|value| AbstractValue::constant(U256::from(value)))
        .reduce(|left, right| domain.join(&left, &right))
        .map(|value| scalar(&value, domain))
        .unwrap()
}

fn producer_shape(domain: Domain) -> ByteArraySummary {
    let dense = finite(domain, 0..=255);
    let half = finite(domain, 0..=127);
    let holes = finite(domain, [0, 1, 2, 3, 8, 9, 10, 11]);
    ByteArraySummary {
        length: AbstractValue::from_numeric(NumericValue::constant(U256::from(64))),
        memory: false,
        content: Content::Cells(
            (0..64)
                .map(|index| {
                    let value = match index {
                        62 => half.clone(),
                        63 => holes.clone(),
                        _ => dense.clone(),
                    };
                    (index, value)
                })
                .collect(),
        ),
    }
}

fn old_root(summary: &ByteArraySummary, policy: StateLimits) -> Result<ByteStateId, StateError> {
    let Content::Cells(cells) = &summary.content else {
        panic!("cell fixture")
    };
    ByteStateId::observed(
        SourceId::fresh()?,
        ExprId::constant(summary.length.singleton().unwrap()),
        cells
            .iter()
            .map(|(index, value)| StateBinding {
                index: U256::from(*index),
                value: ExprId::fresh().unwrap(),
                numeric: value.numeric().clone(),
            })
            .collect(),
        policy,
    )
}

fn bindings(root: &ByteStateId) -> &[StateBinding] {
    let ByteStateKind::Input { bindings, .. } = root.kind() else {
        panic!("observed fixture")
    };
    bindings
}

fn finite_cardinality(values: &[StateBinding]) -> usize {
    values
        .iter()
        .filter_map(|binding| binding.numeric.finite_constants().cardinality())
        .sum()
}

#[test]
fn actual_shallow_observed_constructor_admits_compact_64_binding_carrier_only() {
    let domain = domain(Profile::Product);
    let summary = producer_shape(domain);
    let frozen = summary.clone();
    let original_work = summary.work_size();
    let policy: StateLimits = limits(domain).into();
    assert_eq!(policy.max_nodes, 16_384);
    assert_eq!(
        old_root(&summary, policy),
        Err(StateError::Limit {
            nodes: 16_843,
            depth: 2
        })
    );
    let old = old_root(
        &summary,
        StateLimits {
            max_nodes: 16_843,
            ..policy
        },
    )
    .unwrap();
    assert_eq!(finite_cardinality(bindings(&old)), 16_008);
    let replay = summary.instantiate(domain).unwrap();
    assert_eq!(replay.root.nodes(), 906);
    assert_eq!(replay.root.depth(), 2);
    let retained = bindings(&replay.root);
    assert_eq!(retained.len(), 64);
    assert_eq!(finite_cardinality(retained), 8);
    assert_eq!(
        retained
            .iter()
            .filter(|b| b.numeric.constants().is_none())
            .count(),
        63
    );
    let Content::Cells(cells) = &summary.content else {
        panic!("cell fixture")
    };
    for (before, after) in bindings(&old).iter().zip(retained) {
        assert_eq!(before.index, after.index);
        assert_eq!(before.numeric.known_bits(), after.numeric.known_bits());
        assert_eq!(before.numeric.interval(), after.numeric.interval());
        assert_eq!(before.numeric.congruence(), after.numeric.congruence());
        for candidate in (0u64..=256).map(U256::from).chain([U256::MAX]) {
            assert_eq!(
                before.numeric.contains(candidate),
                after.numeric.contains(candidate)
            );
        }
    }
    assert_eq!(retained[63].numeric, *cells[&63].numeric());
    assert_eq!(summary, frozen);
    assert_eq!(summary.work_size(), original_work);
    assert_eq!(replay.bytes, *cells);
    assert!(original_work >= replay.work_size());
}

#[test]
fn actual_84_binding_dense_carrier_shrinks_without_changing_the_policy() {
    let domain = domain(Profile::Product);
    let dense = finite(domain, 0..=255);
    let summary = ByteArraySummary {
        length: AbstractValue::from_numeric(NumericValue::constant(U256::from(84))),
        memory: false,
        content: Content::Cells((0..84).map(|index| (index, dense.clone())).collect()),
    };
    assert_eq!(
        old_root(&summary, limits(domain).into()),
        Err(StateError::Limit {
            nodes: 22_599,
            depth: 2
        })
    );
    let replay = summary.instantiate(domain).unwrap();
    assert_eq!(replay.root.nodes(), 1_179);
    assert_eq!(replay.root.depth(), 2);
    assert_eq!(finite_cardinality(bindings(&replay.root)), 0);
    assert!(summary.work_size() >= replay.work_size());
}

#[test]
fn future_byte_and_word_reads_include_values_and_keep_exact_known_controls() {
    let domain = domain(Profile::Product);
    let summary = producer_shape(domain);
    let replay = summary.instantiate(domain).unwrap();
    let Content::Cells(cells) = &summary.content else {
        panic!("cell fixture")
    };
    for index in [0, 61, 62, 63] {
        let read = replay
            .read_byte_checked(&AbstractValue::constant(U256::from(index)), domain)
            .unwrap();
        for candidate in (0u64..=256).map(U256::from).chain([U256::MAX]) {
            assert_eq!(read.contains(candidate), cells[&index].contains(candidate));
        }
    }
    assert_eq!(replay.byte_at(64, domain).singleton(), Some(U256::ZERO));
    let word = replay
        .read_word_checked(&AbstractValue::constant(U256::from(32)), domain)
        .unwrap();
    for (upper, lower) in [(0u8, 0u8), (127, 11), (93, 3), (1, 8)] {
        let mut bytes = [0xa5; 32];
        bytes[30] = upper;
        bytes[31] = lower;
        assert!(word.contains(U256::from_be_bytes(bytes)));
    }
    assert!(!word.contains(U256::from(128u64 << 8)));
    assert!(!word.contains(U256::from(4)));
    // A finite-sensitive word with only one uncertain byte must remain exact.
    let controlled = ByteArraySummary {
        length: AbstractValue::constant(U256::from(32)),
        memory: false,
        content: Content::Cells(
            (0..32)
                .map(|index| {
                    (
                        index,
                        if index == 31 {
                            finite(domain, 7..=9)
                        } else {
                            AbstractValue::constant(U256::ZERO)
                        },
                    )
                })
                .collect(),
        ),
    }
    .instantiate(domain)
    .unwrap();
    let word = controlled
        .read_word_checked(&AbstractValue::constant(U256::ZERO), domain)
        .unwrap();
    assert_eq!(
        word.constants(),
        Some(&BTreeSet::from([
            U256::from(7),
            U256::from(8),
            U256::from(9)
        ]))
    );
}

#[test]
fn repeated_projection_join_and_instantiation_keep_canonical_cells_and_work_stable() {
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let domain = domain(profile);
        let mut inputs = vec![ByteArraySummary {
            length: finite(domain, [1, 3]),
            memory: false,
            content: Content::Cells(BTreeMap::from([
                (0, finite(domain, 0..=255)),
                (1, finite(domain, 0..=127)),
                (2, finite(domain, [0, 1, 2, 3, 8, 9, 10, 11])),
            ])),
        }];
        if profile == Profile::Product {
            inputs.push(producer_shape(domain));
        }
        for mut summary in inputs {
            let Content::Cells(cells) = &summary.content else {
                panic!("cell fixture")
            };
            let mut old = summary.instantiate(domain).unwrap();
            old.root = ByteStateId::observed(
                SourceId::fresh().unwrap(),
                old.length.symbolic_expression().unwrap(),
                cells
                    .iter()
                    .map(|(index, value)| StateBinding {
                        index: U256::from(*index),
                        value: ExprId::fresh().unwrap(),
                        numeric: value.numeric().clone(),
                    })
                    .collect(),
                StateLimits {
                    max_nodes: 16_843,
                    max_depth: 256,
                },
            )
            .unwrap();
            // Start from the existing uncompressed round-trip canonical form.
            // This includes uncertain-length zero padding before compaction.
            let canonical = old.summary_projection(domain).unwrap();
            assert_eq!(canonical, summary);
            let frozen = summary.clone();
            let work = summary.work_size();
            for _ in 0..4 {
                let replay = summary.instantiate(domain).unwrap();
                let Content::Cells(cells) = &summary.content else {
                    panic!("cell fixture")
                };
                assert_eq!(replay.bytes, *cells);
                let projected = replay.summary_projection(domain).unwrap();
                assert_eq!(projected, frozen);
                assert_eq!(projected.work_size(), work);
                assert!(projected.covers(&frozen, domain));
                assert!(frozen.covers(&projected, domain));
                assert_eq!(frozen.join(&projected, domain, false), frozen);
                summary = projected;
            }
        }
    }
}

#[test]
fn instantiations_and_saved_versions_keep_independent_sources_and_old_reads() {
    let domain = domain(Profile::Product);
    let summary = producer_shape(domain);
    let mut current = summary.instantiate(domain).unwrap();
    let saved = current.clone();
    let sibling = summary.instantiate(domain).unwrap();
    assert_ne!(current.root, sibling.root);
    let current_binding = &bindings(&current.root)[0];
    let sibling_binding = &bindings(&sibling.root)[0];
    assert_ne!(current_binding.value, sibling_binding.value);
    let old_read = saved
        .read_byte_checked(&AbstractValue::constant(U256::ZERO), domain)
        .unwrap();
    current
        .write_byte(
            &AbstractValue::constant(U256::ZERO),
            &AbstractValue::constant(U256::from(17)),
            64,
            domain,
        )
        .unwrap();
    assert_eq!(current.byte_at(0, domain).singleton(), Some(U256::from(17)));
    assert!(saved.byte_at(0, domain).contains(U256::from(200)));
    assert!(sibling.byte_at(0, domain).contains(U256::from(200)));
    assert_eq!(
        saved
            .read_byte_checked(&AbstractValue::constant(U256::ZERO), domain)
            .unwrap(),
        old_read
    );
    assert_eq!(saved.summary_projection(domain).unwrap(), summary);
    let Content::Cells(cells) = current.summary_projection(domain).unwrap().content else {
        panic!("mixed current content")
    };
    assert_eq!(cells[&0].singleton(), Some(U256::from(17)));
}

#[test]
fn retention_failures_and_cancellation_never_install_partial_arrays_or_summaries() {
    let domain = domain(Profile::Product);
    let summary = producer_shape(domain);
    let frozen = summary.clone();
    let tiny = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_nodes: 905,
        ..domain.spec().relations()
    }));
    assert_eq!(
        summary.instantiate(tiny),
        Err(RangeError::State(StateError::Limit {
            nodes: 906,
            depth: 2
        }))
    );
    assert_eq!(summary, frozen);
    let replay = summary.instantiate(domain).unwrap();
    let saved = replay.clone();
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            summary.instantiate(domain),
            Err(RangeError::State(StateError::Cancelled))
        );
        assert_eq!(
            replay.summary_projection(domain),
            Err(RangeError::Expression(ExprError::Cancelled))
        );
    });
    assert_eq!(summary, frozen);
    assert_eq!(replay, saved);
}

#[test]
fn presentation_restoration_preserves_current_value_metadata_and_never_copies_old_bounds() {
    use crate::domain::{facts::Symbol, provenance::Origin};

    let domain = domain(Profile::Product);
    let previous = finite(domain, 0..=255)
        .with_origin(Origin::Balance)
        .with_identity(10, 1)
        .with_expression(ExprId::fresh().unwrap());
    let numeric = NumericValue::unsigned_range(U256::from(4), U256::from(31)).unwrap();
    let current = AbstractValue::constant(U256::ZERO)
        .with_numeric(numeric.clone())
        .with_origin(Origin::Calldata)
        .with_identity(11, 2)
        .with_symbol(Symbol::new(1), Some(11))
        .with_symbolic_limit();
    let restored = restore_presentation(current.clone(), Some(&previous), domain);
    assert_eq!(
        restored.numeric(),
        &numeric.restore_redundant_finite_from(previous.numeric())
    );
    assert_eq!(restored.provenance(), current.provenance());
    assert_eq!(restored.identity(), current.identity());
    assert_eq!(restored.expression(), current.expression());
    assert_eq!(
        restored.symbolic_limit_reached(),
        current.symbolic_limit_reached()
    );
    assert_eq!(restore_presentation(current.clone(), None, domain), current);
    assert_eq!(
        restore_presentation(restored.clone(), Some(&previous), domain),
        restored
    );
}

#[test]
fn projection_to_a_smaller_capacity_cannot_resurrect_a_discarded_finite_component() {
    let domain = domain(Profile::Product);
    let summary = ByteArraySummary {
        length: AbstractValue::constant(U256::from(1)),
        memory: false,
        content: Content::Cells(BTreeMap::from([(0, finite(domain, 0..=255))])),
    };
    let compact = summary.instantiate(domain).unwrap();
    let mut original = compact.clone();
    original.root = old_root(&summary, limits(domain).into()).unwrap();
    for smaller in [
        Domain::default(),
        Domain::new(NonZeroUsize::new(8).unwrap()),
    ] {
        assert_eq!(smaller.capacity(), 8);
        let projected = compact.summary_projection(smaller).unwrap();
        assert_eq!(projected, original.summary_projection(smaller).unwrap());
        let Content::Cells(cells) = projected.content else {
            panic!("unknown-byte projection")
        };
        if smaller.spec().profile() == Profile::Product {
            assert!(cells.is_empty());
        } else {
            assert!(cells[&0].numeric().is_top());
        }
    }
}

#[test]
fn projection_checks_current_root_before_using_a_stale_or_missing_presentation_cell() {
    let domain = domain(Profile::Product);
    let previous = finite(domain, 0..=255);
    for current in [
        AbstractValue::unsigned_range(U256::from(4), U256::from(31)).unwrap(),
        AbstractValue::unsigned_range(U256::ZERO, U256::from(511)).unwrap(),
        finite(domain, [1, 3, 5]),
    ] {
        let summary = ByteArraySummary {
            length: AbstractValue::constant(U256::from(1)),
            memory: false,
            content: Content::Cells(BTreeMap::from([(0, previous.clone())])),
        };
        let mut replay = summary.instantiate(domain).unwrap();
        replay.root = ByteStateId::observed(
            SourceId::fresh().unwrap(),
            ExprId::constant(U256::from(1)),
            vec![StateBinding {
                index: U256::ZERO,
                value: ExprId::fresh().unwrap(),
                numeric: current.numeric().clone(),
            }],
            limits(domain).into(),
        )
        .unwrap();
        let projected = replay.summary_projection(domain).unwrap();
        let Content::Cells(cells) = &projected.content else {
            panic!("cell fixture")
        };
        let expected = current
            .numeric()
            .restore_redundant_finite_from(previous.numeric());
        assert_eq!(cells[&0].numeric(), &expected);
        for candidate in (0u64..=512).map(U256::from).chain([U256::MAX]) {
            assert_eq!(cells[&0].contains(candidate), current.contains(candidate));
        }
        assert_eq!(
            projected
                .instantiate(domain)
                .unwrap()
                .summary_projection(domain)
                .unwrap(),
            projected
        );
        replay.bytes.clear();
        let uncached = replay.summary_projection(domain).unwrap();
        let Content::Cells(cells) = &uncached.content else {
            panic!("cell fixture")
        };
        assert_eq!(cells[&0].numeric(), current.numeric());
    }
}
