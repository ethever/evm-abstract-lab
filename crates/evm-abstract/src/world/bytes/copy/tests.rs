use super::super::*;

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
        .join_guarded(&branch(0x44), domain, selector.clone(), limits(domain))
        .unwrap();
    source.remember(selector, &constant(1), domain).unwrap();
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
