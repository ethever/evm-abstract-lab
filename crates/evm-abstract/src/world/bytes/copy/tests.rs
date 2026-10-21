use super::super::*;

#[test]
fn projection_requires_numeric_certificates_separate_from_prepared_literals() {
    let previous = ByteArray::memory();
    let source = ByteArray::exact(&[0x11]);
    let complete = ByteArray::memory();
    let values = [zero(), zero(), constant(1)].map(|value| {
        let term = ExprId::constant(*value.constants().unwrap().first().unwrap());
        value.with_expression(term)
    });
    for missing in 0..3 {
        let mut operands = values.clone();
        operands[missing] =
            AbstractValue::top().with_expression(operands[missing].expression().unwrap().clone());
        let projected = super::project(
            &previous,
            &source,
            &operands[0],
            &operands[1],
            &operands[2],
            &complete,
        )
        .unwrap();
        let byte = &projected[&0];
        assert_eq!(byte.singleton(), None);
        assert!(byte.contains(U256::ZERO));
        assert!(byte.contains(U256::from(0xff)));
    }
    let range_values = [0, 0, 1].map(|n| {
        let word = U256::from(n);
        AbstractValue::unsigned_range(word, word)
            .unwrap()
            .with_expression(ExprId::constant(word))
    });
    let bit_values = [0, 0, 1].map(|n| {
        let word = U256::from(n);
        let numeric =
            crate::domain::NumericValue::from_observation(&crate::domain::WordObservation::bits(
                crate::domain::known_bits::KnownBits::exact(word),
            ));
        AbstractValue::from_numeric(numeric).with_expression(ExprId::constant(word))
    });
    assert!(range_values[0].constants().is_none());
    assert!(bit_values[0].constants().is_none());
    for values in [values, range_values, bit_values] {
        let projected = super::project(
            &previous, &source, &values[0], &values[1], &values[2], &complete,
        )
        .unwrap();
        assert_eq!(projected[&0].singleton(), Some(U256::from(0x11)));
    }
}

#[test]
fn copy_quotes_pay_source_once_and_additive_occurrences_and_keep_history_fees() {
    let domain = Domain::default();
    let policy = limits(crate::domain::ValueAlgebra::new(&domain)).into();
    let c = |n| ExprId::constant(U256::from(n));
    let a = ByteStateId::zero();
    let b = ByteStateId::exact(Arc::from([0x22]));
    let pair = a
        .join_guarded(&b, ExprId::fresh().unwrap(), policy)
        .unwrap();
    let third = ByteStateId::exact(Arc::from([0x33]));
    // Reuse leaves in reverse order, then a distinct byte-0 alternative: all
    // five occurrences survive exact shared-branch factoring at 13 total nodes.
    let reversed = b
        .join_guarded(&a, ExprId::fresh().unwrap(), policy)
        .unwrap();
    let nested = reversed
        .join_guarded(&third, ExprId::fresh().unwrap(), policy)
        .unwrap();
    let shared = pair
        .join_guarded(&nested, ExprId::fresh().unwrap(), policy)
        .unwrap();
    let mut history = pair;
    let mut branch = a;
    for at in 0u64..3 {
        history = history.write_byte(c(at), c(at + 1), policy).unwrap();
        branch = branch.write_byte(c(at), c(at + 1), policy).unwrap();
    }
    let two = branch
        .join_guarded(&b, ExprId::fresh().unwrap(), policy)
        .unwrap();
    let source = ByteArray::unknown(); // No presentation candidates in any lane.
    let flat = ByteArray::memory();
    let destination = constant(64);
    let offset = zero();
    let size = constant(1);
    let accounts = 2usize;
    let quotes = |target: &ByteArray| {
        [
            target.copy_work(&source, &destination, &offset, &size, domain),
            target.copy_code_work(0, accounts, &destination, &offset, &size, domain),
            target.copy_return_data_work(&source, &destination, &size, domain),
        ]
    };
    let ranges = super::scalar_work(&flat.length)
        + super::scalar_work(&destination)
        + super::scalar_work(&offset)
        + super::scalar_work(&size)
        + 8;
    let return_ranges = (super::scalar_work(&size)
        + super::scalar_work(&source.length)
        + super::scalar_work(&flat.length)
        + super::scalar_work(&destination)
        + 8)
        * 4;
    let units = [
        (source.root.nodes() + 1) * ranges * 8,
        (accounts * 6 + 3 + 1) * ranges * 8,
        (source.root.nodes() + 1) * return_ranges * 8,
    ];
    let per_leaf = [
        (source.root.nodes() + ranges + 1) * 8,
        (accounts * 6 + 3 + ranges + 1) * 8,
        (source.root.nodes() + return_ranges + 1) * 8,
    ];
    let mut one = flat.clone();
    one.root = history;
    assert!(matches!(one.root.kind(), ByteStateKind::WriteByte { .. }));
    assert_eq!(one.root.copy_leaves(), 1); // Ite history is below an update.
    let history_fee = |nodes| nodes * super::height(nodes) * 4;
    let baseline = quotes(&flat);
    let nonselected = quotes(&one);
    for lane in 0..3 {
        // Both legacy and new non-Ite factors are one. Retained history still
        // contributes exactly its original independent fee, with no candidates.
        assert_eq!(
            nonselected[lane] - baseline[lane],
            history_fee(13) - history_fee(1)
        );
    }
    for (root, leaves) in [(two, 2), (shared, 5)] {
        assert_eq!(root.nodes(), 13);
        assert_eq!(root.copy_leaves(), leaves);
        let mut selected = one.clone();
        selected.root = root;
        let selected_quotes = quotes(&selected);
        for lane in 0..3 {
            // Identical metadata and retention counts isolate additive target
            // occurrence work. The complete source/range product is paid once.
            let legacy =
                nonselected[lane] - per_leaf[lane] + (selected.root.nodes() - 1) * units[lane];
            assert_eq!(
                selected_quotes[lane] - nonselected[lane],
                (leaves - 1) * per_leaf[lane]
            );
            assert_eq!(
                legacy - selected_quotes[lane],
                (13 - 1) * units[lane] - leaves * per_leaf[lane]
            );
        }
    }
}

#[test]
fn uncertified_cells_keep_coordinates_for_authoritative_scoped_summary_reads() {
    let domain = Domain::default();
    let selector = ExprId::fresh().unwrap();
    let branch = |byte| {
        let value = AbstractValue::top()
            .with_expression(ExprId::fresh().unwrap())
            .with_numeric(constant(byte).numeric().clone());
        let mut array = ByteArray::memory();
        array
            .write_byte(&constant(700), &value, 1024, domain)
            .unwrap();
        array
    };
    let mut source = branch(0x33)
        .join_guarded(
            &branch(0x44),
            domain,
            selector.clone(),
            limits(crate::domain::ValueAlgebra::new(&domain)),
        )
        .unwrap();
    source
        .remember(
            selector,
            &constant(1),
            crate::domain::ValueAlgebra::new(&domain),
        )
        .unwrap();
    assert!(source.conditional.len() > 0);
    // Presentation cannot certify these stale values, even with retained terms.
    source.bytes.insert(700, constant(0x99));
    let mut target = ByteArray::memory();
    target
        .write_byte(&constant(4000), &constant(0xaa), 8192, domain)
        .unwrap();
    let previous = target.root.clone();
    target.bytes.insert(4000, constant(0x88));
    target
        .bytes
        .extend((4001..4300).map(|index| (index, constant(0x88))));
    assert!(target.bytes.len() > COPY_FACT_LIMIT);
    target
        .copy_from(
            &constant(4096),
            &source,
            &zero(),
            &constant(1024),
            8192,
            domain,
        )
        .unwrap();
    assert!(target.bytes.len() <= COPY_FACT_LIMIT);
    assert!(
        matches!(target.root.kind(), ByteStateKind::Copy { previous: old, source: view, .. }
        if old == &previous && view.state == source.root)
    );
    for index in [4000, 4796] {
        assert_eq!(
            target.bytes[&index].numeric(),
            AbstractValue::unknown_byte().numeric()
        );
    }
    assert_eq!(
        target.default.numeric(),
        AbstractValue::unknown_byte().numeric()
    );
    // Length > 256 disables both bounded materialization and summary's dense
    // key fallback. These two retained coordinates must be reread from the root
    // using current unconditional AND conditional facts, not the display map.
    assert!(target.len().singleton().unwrap() > U256::from(COPY_FACT_LIMIT));
    let replay = target
        .summary_projection(domain)
        .unwrap()
        .instantiate(domain)
        .unwrap();
    assert_eq!(
        replay.byte_at(4000, domain).singleton(),
        Some(U256::from(0xaa))
    );
    assert_eq!(
        replay.byte_at(4796, domain).singleton(),
        Some(U256::from(0x33))
    );
}

#[test]
fn possibly_empty_copy_preserves_strict_suffix_and_captured_source_padding() {
    let domain = Domain::default();
    let mut prior = vec![0xaa; 32];
    prior.extend([0xbb; 32]);
    let mut source = ByteArray::exact(&[0x11]);
    source
        .expand(&zero(), &constant(512), 1024, domain)
        .unwrap();
    let length = domain.join(&zero(), &constant(1));
    let length = length.clone().with_expression(expr(&length).unwrap());
    for (offset, copied) in [(0, 0x11), (1, 0)] {
        let mut target = ByteArray::memory();
        target
            .copy_from(
                &constant(32),
                &ByteArray::exact(&prior),
                &zero(),
                &constant(64),
                1024,
                domain,
            )
            .unwrap();
        target
            .copy_from(
                &constant(32),
                &source,
                &constant(offset),
                &length,
                1024,
                domain,
            )
            .unwrap();
        assert!(
            matches!(target.root.kind(), ByteStateKind::Copy { length: retained, .. }
            if Some(retained) == length.expression())
        );
        assert_eq!(
            target.bytes[&32].numeric(),
            AbstractValue::unknown_byte().numeric()
        );
        let first = target.byte_at(32, domain);
        assert!(first.contains(U256::from(0xaa)) && first.contains(U256::from(copied)));
        assert!(!first.contains(U256::from(0xbb)));
        if offset == 1 {
            assert!(!first.contains(U256::from(0x11)));
        }
        let replay = target
            .summary_projection(domain)
            .unwrap()
            .instantiate(domain)
            .unwrap();
        for current in [&target, &replay] {
            for index in 33..64 {
                assert_eq!(
                    current.byte_at(index, domain).singleton(),
                    Some(U256::from(0xaa))
                );
            }
            assert_eq!(
                current.read_word(&constant(64), domain).singleton(),
                Some(U256::from_be_bytes([0xbb; 32]))
            );
            assert_eq!(current.len().singleton(), Some(U256::from(96)));
        }
        // Wrapper expansion never changes the captured Exact root's boundary.
        assert_eq!(source.byte_at(1, domain).singleton(), Some(U256::ZERO));
    }
}

#[test]
fn overlapping_memory_copies_capture_prestate_and_preserve_historical_reads() {
    let domain = Domain::default();
    for (destination, offset, expected) in [
        (1, 0, [0x11, 0x11, 0x22, 0x33]),
        (0, 1, [0x22, 0x33, 0x44, 0x44]),
    ] {
        let mut memory = ByteArray::memory();
        memory
            .copy_from(
                &zero(),
                &ByteArray::exact(&[0x11, 0x22, 0x33, 0x44]),
                &zero(),
                &constant(4),
                64,
                domain,
            )
            .unwrap();
        let mut snapshot = memory.clone();
        let historical = snapshot.clone();
        let history = snapshot.read_word_checked(&zero(), domain).unwrap();
        let captured = snapshot.root.clone();
        memory
            .copy_from(
                &constant(destination),
                &snapshot,
                &constant(offset),
                &constant(3),
                64,
                domain,
            )
            .unwrap();
        assert!(
            matches!(memory.root.kind(), ByteStateKind::Copy { source, .. } if source.state == captured)
        );
        snapshot
            .write_byte(&zero(), &constant(0xff), 64, domain)
            .unwrap();
        for (index, byte) in expected.into_iter().enumerate() {
            assert_eq!(
                memory.byte_at(index, domain).singleton(),
                Some(U256::from(byte))
            );
        }
        let retained = historical.read_word_checked(&zero(), domain).unwrap();
        assert_eq!(retained.expression(), history.expression());
        assert_eq!(retained.numeric(), history.numeric());
        assert_ne!(
            snapshot.byte_at(0, domain).singleton(),
            memory.byte_at(0, domain).singleton()
        );
    }
}

#[test]
fn mandatory_retention_and_cancel_refusals_leave_all_copy_state_unchanged() {
    let domain = Domain::default();
    let source = ByteArray::exact(&[0x11, 0x22]);
    let mut memory = ByteArray::memory();
    memory
        .write_byte(&zero(), &constant(0xaa), 64, domain)
        .unwrap();
    let before = memory.clone();
    let low = Domain::from_spec(domain.spec().with_relations(
        crate::domain::relational::RelationLimits {
            max_depth: 1,
            ..domain.spec().relations()
        },
    ));
    assert!(matches!(
        memory.copy_from(&constant(8), &source, &zero(), &constant(2), 64, low),
        Err(RangeError::State(StateError::Limit { .. }))
    ));
    assert_eq!(memory, before);
    let limited = constant(2).with_symbolic_limit();
    assert!(matches!(
        memory.copy_from(&constant(8), &source, &zero(), &limited, 64, domain),
        Err(RangeError::Expression(ExprError::Limit {
            nodes: usize::MAX,
            depth: usize::MAX
        }))
    ));
    assert_eq!(memory, before);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            memory.copy_from(&constant(8), &source, &zero(), &constant(2), 64, domain),
            Err(RangeError::Expression(ExprError::Cancelled))
        );
        assert_eq!(memory, before);
        memory
            .copy_from(
                &AbstractValue::constant(U256::MAX),
                &source,
                &zero(),
                &zero(),
                0,
                domain,
            )
            .unwrap();
        assert_eq!(memory, before);
    });
}

fn conditional_return_data_fixture(domain: Domain, entries: usize) -> ByteArray {
    let branch = |byte| {
        let mut array = ByteArray::exact(&vec![byte; entries]);
        let value = AbstractValue::top()
            .with_expression(ExprId::fresh().unwrap())
            .with_numeric(constant(0x77).numeric().clone());
        array
            .write_byte(&constant(entries / 2), &value, 0, domain)
            .unwrap();
        array
    };
    branch(0x11)
        .join_guarded(
            &branch(0x22),
            domain,
            ExprId::fresh().unwrap(),
            limits(crate::domain::ValueAlgebra::new(&domain)),
        )
        .unwrap()
}

#[test]
fn zero_requested_return_data_preserves_large_conditional_memory_and_quote() {
    let domain = Domain::default();
    let source = conditional_return_data_fixture(domain, 4096);
    assert!(source.conditional.len() > 0);
    let source_before = source.clone();
    let mut target = ByteArray::memory();
    target
        .copy_from(&zero(), &source, &zero(), &constant(4096), 0, domain)
        .unwrap();
    target
        .expand(
            &AbstractValue::constant(U256::from(1) << 192),
            &constant(64),
            0,
            domain,
        )
        .unwrap();
    assert!(target.conditional.len() > 0);
    assert!(target.len().singleton().unwrap() > U256::from(usize::MAX));
    let before = target.clone();
    let historical = before.read_word_checked(&zero(), domain).unwrap();
    let destination = AbstractValue::constant(U256::MAX);
    let fee = target.copy_return_data_work(&source, &destination, &zero(), domain);
    assert!(fee > 0);
    for smaller in [
        ByteArray::empty(),
        conditional_return_data_fixture(domain, 16),
    ] {
        assert_eq!(
            target.copy_return_data_work(&smaller, &destination, &zero(), domain),
            fee
        );
    }
    assert_eq!(
        ByteArray::memory().copy_return_data_work(
            &ByteArray::empty(),
            &destination,
            &zero(),
            domain
        ),
        fee
    );
    target
        .copy_return_data(&destination, &source, &zero(), 0, domain)
        .unwrap();
    assert_eq!(target, before);
    assert_eq!(target.root, before.root);
    assert_eq!(target.metadata_len(), before.metadata_len());
    assert_eq!(
        target.read_word_checked(&zero(), domain).unwrap(),
        historical
    );
    assert_eq!(source, source_before);
}

#[test]
fn zero_requested_return_data_keeps_normalization_errors_before_noop() {
    let domain = Domain::default();
    let mut target = ByteArray::memory();
    let poison = RangeError::Expression(ExprError::Limit {
        nodes: usize::MAX,
        depth: usize::MAX,
    });
    target.failure = Some(poison.clone());
    let mut source = ByteArray::unknown();
    source.failure = Some(poison);
    let before = target.clone();
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        // A numeric-only bounded destination really constructs an observed
        // term. Cancellation must still fail there, before the zero shortcut.
        assert_eq!(
            target.copy_return_data(&AbstractValue::unknown_byte(), &source, &zero(), 0, domain),
            Err(RangeError::State(StateError::Cancelled))
        );
        assert_eq!(target, before);
        // Literal normalization succeeds without inspecting either poisoned
        // array, overflowing target extents, or a zero request's limit marker.
        target
            .copy_return_data(
                &AbstractValue::constant(U256::MAX),
                &source,
                &zero().with_symbolic_limit(),
                0,
                domain,
            )
            .unwrap();
        assert_eq!(target, before);
    });
}

#[test]
fn empty_actual_return_data_keeps_expansion_and_atomic_typed_refusals() {
    let domain = Domain::default();
    let possible_length = domain.join(&zero(), &constant(1));
    let mut source = conditional_return_data_fixture(domain, 4096)
        .slice(&zero(), &possible_length, 0, domain)
        .unwrap();
    // Constrain this captured slice's own length, retaining its root/history.
    let length_term = source.len().expression().unwrap().clone();
    source.length = source.length.clone().with_numeric(zero().numeric().clone());
    source
        .remember(
            length_term,
            &zero(),
            crate::domain::ValueAlgebra::new(&domain),
        )
        .unwrap();
    assert!(source.conditional.len() > 0);
    assert!(source.root.nodes() > ByteArray::empty().root.nodes());
    let source_before = source.clone();
    let mut target = ByteArray::memory();
    let old_byte = AbstractValue::top()
        .with_expression(ExprId::fresh().unwrap())
        .with_numeric(constant(0xaa).numeric().clone());
    target.write_byte(&zero(), &old_byte, 0, domain).unwrap();
    let before = target.clone();
    let historical = before.read_byte_checked(&zero(), domain).unwrap();
    let destination = constant(32);
    let requested = constant(64);
    let fee = target.copy_return_data_work(&source, &destination, &requested, domain);
    assert_eq!(
        fee,
        target.copy_return_data_work(&ByteArray::empty(), &destination, &requested, domain)
    );
    assert!(fee > target.copy_return_data_work(&source, &destination, &zero(), domain));
    assert!(
        fee > ByteArray::memory().copy_return_data_work(&source, &destination, &requested, domain)
    );
    let mut expected = before.clone();
    expected
        .expand(&destination, &requested, 0, domain)
        .unwrap();
    target
        .copy_return_data(&destination, &source, &requested, 0, domain)
        .unwrap();
    assert_eq!(target, expected);
    assert_eq!(target.root, before.root);
    assert_eq!(target.bytes, before.bytes);
    assert_eq!(target.len().singleton(), Some(U256::from(96)));
    assert_eq!(target.allocations.len(), before.allocations.len() + 1);
    assert_eq!(
        target.byte_at(0, domain).singleton(),
        Some(U256::from(0xaa))
    );
    assert_eq!(target.byte_at(32, domain).singleton(), Some(U256::ZERO));
    assert_eq!(
        before.read_byte_checked(&zero(), domain).unwrap(),
        historical
    );
    assert_eq!(source, source_before);
    let expanded = target.clone();
    target
        .copy_return_data(&destination, &source, &requested, 0, domain)
        .unwrap();
    assert_eq!(target, expanded);
    let tight = Domain::from_spec(domain.spec().with_relations(
        crate::domain::relational::RelationLimits {
            max_depth: 1,
            ..crate::domain::relational::RelationLimits::default()
        },
    ));
    // Reusing the allocation must still validate the retained target root.
    assert!(target.root.depth() > 1);
    assert!(matches!(
        target.copy_return_data(&destination, &source, &requested, 0, tight),
        Err(RangeError::State(StateError::Limit { .. }))
    ));
    assert_eq!(target, expanded);
    assert_eq!(
        target.copy_return_data(
            &AbstractValue::constant(U256::MAX),
            &source,
            &requested,
            0,
            domain
        ),
        Err(RangeError::OffsetTooLarge(U256::MAX))
    );
    assert_eq!(target, expanded);
    assert_eq!(
        target.copy_return_data(
            &constant(128),
            &source,
            &requested.clone().with_symbolic_limit(),
            0,
            domain
        ),
        Err(RangeError::Expression(ExprError::Limit {
            nodes: usize::MAX,
            depth: usize::MAX
        }))
    );
    assert_eq!(target, expanded);
    let mut poisoned = expanded.clone();
    let poison = RangeError::State(StateError::Limit {
        nodes: 99,
        depth: 3,
    });
    poisoned.failure = Some(poison.clone());
    let poisoned_before = poisoned.clone();
    assert_eq!(
        poisoned.copy_return_data(&destination, &source, &requested, 0, domain),
        Err(poison)
    );
    assert_eq!(poisoned, poisoned_before);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            target.copy_return_data(&destination, &source, &requested, 0, domain),
            Err(RangeError::Expression(ExprError::Cancelled))
        );
        assert_eq!(target, expanded);
    });
}

#[test]
fn maybe_zero_and_nonzero_return_data_keep_complete_copy_and_fee() {
    let domain = Domain::default();
    let finite = domain.join(&zero(), &constant(1));
    let interval = AbstractValue::unsigned_range(U256::ZERO, U256::ONE).unwrap();
    let source = ByteArray::exact(&[0x11]);
    for (source, requested, exact) in [
        (source.clone(), finite.clone(), false),
        (source.clone(), interval.clone(), false),
        (source.clone(), constant(1), true),
        (
            source.slice(&zero(), &finite, 0, domain).unwrap(),
            constant(1),
            false,
        ),
        (
            source.slice(&zero(), &interval, 0, domain).unwrap(),
            constant(1),
            false,
        ),
        (ByteArray::unknown(), constant(1), false),
    ] {
        let mut target = ByteArray::memory();
        target
            .copy_from(
                &zero(),
                &ByteArray::exact(&[0xaa, 0xbb]),
                &zero(),
                &constant(2),
                0,
                domain,
            )
            .unwrap();
        let previous = target.root.clone();
        // The existing complete quotation must remain exact for every case
        // which scalar singleton checks do not prove to be empty.
        let ranges = super::scalar_work(&requested)
            .saturating_add(super::scalar_work(&source.length))
            .saturating_add(super::scalar_work(&target.length))
            .saturating_add(super::scalar_work(&zero()))
            .saturating_add(8)
            .saturating_mul(4);
        let branches = super::alternatives(&target.length)
            .saturating_mul(super::alternatives(&zero()))
            .saturating_mul(super::alternatives(&requested))
            .saturating_mul(super::alternatives(&source.length));
        let complete = super::work(
            &target,
            (&source).into(),
            ranges,
            branches,
            crate::domain::ValueAlgebra::new(&domain),
        )
        .saturating_add(
            target
                .metadata_len()
                .saturating_mul(12 + crate::domain::provenance::MAX_ORIGINS + 4),
        );
        assert_eq!(
            target.copy_return_data_work(&source, &zero(), &requested, domain),
            complete
        );
        assert!(complete > target.copy_return_data_work(&source, &zero(), &zero(), domain));
        target
            .copy_return_data(&zero(), &source, &requested, 0, domain)
            .unwrap();
        assert!(matches!(
            target.root.kind(),
            ByteStateKind::Copy { previous: old, .. } if old == &previous
        ));
        assert_eq!(
            target.byte_at(1, domain).singleton(),
            Some(U256::from(0xbb))
        );
        let first = target.byte_at(0, domain);
        if exact {
            assert_eq!(first.singleton(), Some(U256::from(0x11)));
        } else {
            assert!(first.contains(U256::from(0xaa)));
            assert!(first.contains(U256::from(0x11)));
        }
    }
}

#[test]
fn empty_import_quote_avoids_policy_comparisons_only_for_empty_targets() {
    let domain = Domain::default();
    let larger = Domain::from_spec(
        domain
            .spec()
            .with_relations(crate::domain::relational::RelationLimits {
                max_nodes: limits(crate::domain::ValueAlgebra::new(&domain))
                    .max_nodes
                    .saturating_mul(2),
                ..domain.spec().relations()
            }),
    );
    let mut source = ByteArray::exact(&[0x11]);
    for n in 0..8 {
        source
            .operands
            .insert(ExprId::constant(U256::from(n)), constant(n));
    }
    let empty = ByteArray::memory();
    let quote = |target: &ByteArray, source: &ByteArray, domain| {
        [
            target.copy_work(source, &zero(), &zero(), &constant(1), domain),
            target.copy_return_data_work(source, &zero(), &constant(1), domain),
        ]
    };
    assert_eq!(
        quote(&empty, &source, domain),
        quote(&empty, &source, larger)
    );
    // Range operands need no literal/provenance certificate for the import.
    let symbolic = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    assert_eq!(
        empty.copy_work(&source, &symbolic, &symbolic, &symbolic, domain),
        empty.copy_work(&source, &symbolic, &symbolic, &symbolic, larger)
    );
    let mut nonempty = empty.clone();
    nonempty
        .operands
        .insert(ExprId::fresh().unwrap(), constant(0));
    let facts = nonempty.operands.len() + source.operands.len() + 3;
    let old_comparison_delta = source.operands.len()
        * 2
        * (limits(crate::domain::ValueAlgebra::new(&larger)).max_nodes
            - limits(crate::domain::ValueAlgebra::new(&domain)).max_nodes)
        * (super::probes(facts) * 2 + 4);
    for (before, after) in quote(&nonempty, &source, domain)
        .into_iter()
        .zip(quote(&nonempty, &source, larger))
    {
        assert_eq!(after - before, old_comparison_delta);
    }
    // Equal cached counts quote equally even when values need literal cleanup
    // or keys carry larger (but currently valid) expression shapes.
    let mut other = source.clone();
    let complex = expr(&AbstractValue::unknown_byte()).unwrap();
    other.operands.remove(&ExprId::constant(U256::ZERO));
    other
        .operands
        .insert(complex.clone(), constant(7).with_expression(complex));
    assert_eq!(
        quote(&empty, &source, domain),
        quote(&empty, &other, domain)
    );
}

#[test]
fn empty_import_quote_pays_clone_and_height_based_retain_even_for_neutral_facts() {
    let domain = Domain::default();
    let target = ByteArray::memory();
    // Isolate import/clone costs: no logical ranges, source cells or branches.
    // These cached counts also cover a source whose every fact will be removed.
    let quote = |operands| {
        super::work(
            &target,
            super::SourceWork {
                bytes: 0,
                operands,
                allocations: 0,
                conditional: 0,
                conditional_work: 0,
                nodes: 1,
            },
            0,
            0,
            crate::domain::ValueAlgebra::new(&domain),
        )
    };
    let value_copy = 12 + crate::domain::provenance::MAX_ORIGINS + 4;
    // One caller source clone plus the additional ordered import clone, with
    // a cloned key handle as well as each value. Cleanup costs remain on top.
    assert!(quote(6) - quote(0) > 6 * (value_copy * 2 + 1));
    let step = quote(8) - quote(7);
    assert!(step > quote(7) - quote(6));
    assert!(step > quote(9) - quote(8));
}

#[test]
fn code_source_estimates_cover_empty_operand_joins_and_import() {
    let domain = Domain::default();
    let exact = ByteArray::exact(&[0x11, 0x22]);
    let unknown = ByteArray::unknown();
    let selected = exact.join(&unknown, domain);
    assert!(selected.failure.is_none());
    assert!(selected.operands.is_empty());
    assert_eq!(selected.conditional.len(), 0);
    assert!(selected.allocations.is_empty());
    for occupied in [false, true] {
        let mut target = ByteArray::memory();
        if occupied {
            target
                .operands
                .insert(ExprId::fresh().unwrap(), constant(1));
        }
        assert!(
            target.copy_code_work(2, 2, &zero(), &zero(), &constant(1), domain)
                >= target.copy_work(&selected, &zero(), &zero(), &constant(1), domain)
        );
        let mut copied = target.clone();
        copied
            .copy_from(&zero(), &selected, &zero(), &constant(1), 64, domain)
            .unwrap();
        assert_eq!(copied.operands, target.operands);
    }
}
