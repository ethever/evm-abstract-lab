//! Concrete effects must not repeatedly embed unrelated read history.

use super::super::*;
use crate::domain::{
    identity::Symbol,
    provenance::Origin,
    relational::{CheckResult, EvmValueQuery, RelationLimits, RelationState},
    symbolic::ExprKind,
};

fn policy() -> RelationLimits {
    // The captured Multicall request's retention and local reasoning limits.
    RelationLimits {
        max_nodes: 16_384,
        max_depth: 256,
        max_constraints: 2048,
        ..RelationLimits::default()
    }
}

fn domain(policy: RelationLimits) -> TestAlgebra {
    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
            .spec()
            .with_relations(policy),
    ))
}

fn observed(scope: u64, origin: Origin) -> TestValue {
    let mut value = TestValue::top();
    value.set_origin(origin);
    value
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
        .with_numeric(constant::<crate::domain::NumericValue>(7).numeric().clone())
}

fn prove(relations: &RelationState, value: &TestValue, expected: U256, limits: RelationLimits) {
    assert_eq!(
        relations.local_value(
            &expr(value).unwrap(),
            &limits,
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed()
        ),
        EvmValueQuery::Exact(expected)
    );
    assert_eq!(
        relations.can_equal(
            &expr(value).unwrap(),
            expected ^ U256::ONE,
            &limits,
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed()
        ),
        CheckResult::ProvenImpossible
    );
}

#[test]
fn concrete_free_pointer_history_stays_bounded_at_original_budgets() {
    let limits = policy();
    let domain = domain(limits);
    let mut memory = TestBytes::memory();
    memory
        .write_word(&constant(64), &constant(128), domain.borrowed())
        .unwrap();
    let mut historical = Vec::new();
    // On 03cdf, iteration7's offset write needs32067 nodes, beyond16384.
    for iteration in 0..48 {
        let pointer = memory
            .read_word_checked(&constant(64), domain.borrowed())
            .unwrap();
        let expected = U256::from(128 + iteration * 64);
        assert_eq!(pointer.singleton(), Some(expected));
        historical.push((pointer.clone(), expected));
        let next = domain.apply(opcode::ADD, &[pointer.clone(), constant(64)]);
        for (offset, value) in [
            (constant(64), next),
            (pointer.clone(), constant(0)),
            (constant(128 + iteration * 64 + 32), constant(96)),
            (constant(128 + iteration * 32), pointer),
        ] {
            memory
                .write_word(&offset, &value, domain.borrowed())
                .unwrap();
        }
        assert!(memory.state_root().nodes() <= 4 + (iteration + 1) * 12);
        assert!(memory.state_root().depth() <= limits.max_depth);
    }
    for (old, expected) in historical {
        assert_eq!(old.singleton(), Some(expected));
        prove(&RelationState::default(), &old, expected, limits);
    }
}

#[test]
fn disjoint_reads_keep_original_singleton_sources_and_local_facts() {
    let limits = policy();
    let domain = domain(limits);
    let a = observed(851, Origin::Calldata);
    let b = observed(852, Origin::Returndata);
    let mut memory = TestBytes::memory();
    memory
        .write_word(&constant(0), &a, domain.borrowed())
        .unwrap();
    let old = memory
        .read_word_checked(&constant(0), domain.borrowed())
        .unwrap();
    memory
        .write_word(&constant(32), &b, domain.borrowed())
        .unwrap();
    memory
        .write_byte(&constant(128), &a, domain.borrowed())
        .unwrap();
    memory
        .write_word(&constant(160), &constant(99), domain.borrowed())
        .unwrap();
    let saved = memory.clone();
    let form = memory
        .widening_form(domain.borrowed(), limits.expressions())
        .unwrap();
    for current in [&memory, &form] {
        let mut relations = RelationState::default();
        for (expression, value) in current.symbolic_facts() {
            relations
                .observe(expression, value.numeric(), &limits)
                .unwrap();
        }
        assert_eq!(
            relations.check(&limits, domain.borrowed()),
            CheckResult::MayBeFeasible
        );
        for (offset, original, origin) in [(0, &a, Origin::Calldata), (32, &b, Origin::Returndata)]
        {
            assert_eq!(
                current.operands.get(original.expression().unwrap()),
                Some(original)
            );
            let read = current
                .read_word_checked(&constant(offset), domain.borrowed())
                .unwrap();
            assert_eq!(read.numeric(), original.numeric());
            assert_eq!(read.expression(), original.expression());
            assert!(
                read.provenance()
                    .origins()
                    .sources()
                    .unwrap()
                    .contains(&origin)
            );
            prove(&relations, &read, U256::from(7), limits);
        }
        let byte = current
            .read_byte_checked(&constant(128), domain.borrowed())
            .unwrap();
        assert!(
            byte.provenance()
                .origins()
                .sources()
                .unwrap()
                .contains(&Origin::Calldata)
        );
        assert_eq!(
            byte.expression().unwrap().inputs(),
            a.expression().unwrap().inputs()
        );
        prove(&relations, &byte, U256::from(7), limits);
        assert!(
            !current
                .read_word(&constant(0), domain.borrowed())
                .same_identity(&current.read_word(&constant(32), domain.borrowed()))
        );
    }
    assert!(old.same_identity(&a));
    assert!(!old.same_identity(&b));
    assert_eq!(memory, saved);
}

#[test]
fn symbolic_effects_and_unconstrained_guards_keep_both_values() {
    let limits = policy();
    let domain = domain(limits);
    let offset = TestValue::unsigned_range(U256::ZERO, U256::from(64))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let value = TestValue::unsigned_range(U256::from(7), U256::from(8))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let mut memory = TestBytes::memory();
    memory
        .write_word(&offset, &value, domain.borrowed())
        .unwrap();
    let read = memory
        .read_word_checked(&constant(0), domain.borrowed())
        .unwrap();
    assert!(
        matches!(read.expression().unwrap().kind(), ExprKind::WordRead { view, .. } if &view.state == memory.state_root())
    );
    for (at, written, expected) in [(0, 7, 7), (0, 8, 8), (64, 7, 0)] {
        let mut relations = RelationState::default();
        relations
            .observe(
                offset.expression().unwrap(),
                constant(at).numeric(),
                &limits,
            )
            .unwrap();
        relations
            .observe(
                value.expression().unwrap(),
                constant(written).numeric(),
                &limits,
            )
            .unwrap();
        prove(&relations, &read, U256::from(expected), limits);
    }
    let selector = ExprId::fresh().unwrap();
    let mut yes = TestBytes::memory();
    yes.write_word(&constant(0), &constant(7), domain.borrowed())
        .unwrap();
    let mut no = TestBytes::memory();
    no.write_word(&constant(0), &constant(11), domain.borrowed())
        .unwrap();
    let joined = yes
        .join_guarded(
            &no,
            domain.borrowed(),
            selector.clone(),
            limits.expressions(),
        )
        .unwrap();
    let read = joined
        .read_word_checked(&constant(0), domain.borrowed())
        .unwrap();
    assert!(read.singleton().is_none());
    assert!(matches!(
        joined.state_root().kind(),
        ByteStateKind::Ite { .. }
    ));
    for (nonzero, expected) in [(true, 7), (false, 11)] {
        let mut relations = RelationState::default();
        assert_eq!(
            relations.assume(&selector, nonzero, &limits, domain.borrowed()),
            CheckResult::MayBeFeasible
        );
        prove(&relations, &read, U256::from(expected), limits);
    }
}

#[test]
fn overlaps_padding_and_overwritten_history_keep_their_own_values() {
    let limits = policy();
    let domain = domain(limits);
    let mut memory = TestBytes::memory();
    memory
        .write_word(
            &constant(0),
            &TestValue::constant(U256::MAX),
            domain.borrowed(),
        )
        .unwrap();
    let old = memory
        .read_word_checked(&constant(1), domain.borrowed())
        .unwrap();
    let old_hash = memory
        .slice(&constant(0), &constant(32), domain.borrowed())
        .unwrap()
        .keccak(domain.borrowed(), 32)
        .unwrap();
    let snapshot = memory.clone();
    memory
        .write_byte(&constant(31), &constant(42), domain.borrowed())
        .unwrap();
    let overlapping = memory.state_root().clone();
    memory
        .write_word(&constant(64), &constant(9), domain.borrowed())
        .unwrap();
    let read = memory
        .read_word_checked(&constant(0), domain.borrowed())
        .unwrap();
    assert!(
        matches!(read.expression().unwrap().kind(), ExprKind::WordRead { view, .. } if view.state == overlapping)
    );
    prove(
        &RelationState::default(),
        &read,
        (U256::MAX & !U256::from(255)) | U256::from(42),
        limits,
    );
    prove(&RelationState::default(), &old, U256::MAX << 8, limits);
    let hash = snapshot
        .slice(&constant(0), &constant(32), domain.borrowed())
        .unwrap()
        .keccak(domain.borrowed(), 32)
        .unwrap();
    assert_eq!(old_hash, hash);
    let short = ExprId::word_read(
        ByteView::new(
            snapshot.state_root().clone(),
            ExprId::constant(U256::ZERO),
            ExprId::constant(U256::from(31)),
        ),
        ExprId::constant(U256::ZERO),
        limits.expressions(),
    )
    .unwrap();
    prove(
        &RelationState::default(),
        &TestValue::top().with_expression(short),
        U256::MAX & !U256::from(255),
        limits,
    );
    // Foundation indices are mathematical even beyond host-addressable memory.
    let maximal = ByteStateId::zero()
        .write_word(
            ExprId::constant(U256::MAX),
            ExprId::constant(U256::MAX),
            limits.expressions().into(),
        )
        .unwrap()
        .write_word(
            ExprId::constant(U256::ZERO),
            ExprId::constant(U256::from(9)),
            limits.expressions().into(),
        )
        .unwrap();
    let read = ExprId::word_read(
        ByteView::new(
            maximal,
            ExprId::constant(U256::MAX),
            ExprId::constant(U256::from(32)),
        ),
        ExprId::constant(U256::ZERO),
        limits.expressions(),
    )
    .unwrap();
    prove(
        &RelationState::default(),
        &TestValue::top().with_expression(read),
        U256::from(255) << 248,
        limits,
    );
}

#[test]
fn normalization_keeps_retention_and_cancellation_checks_and_failed_writes_atomic() {
    let domain = domain(policy());
    let mut memory = TestBytes::memory();
    memory
        .write_word(&constant(0), &constant(7), domain.borrowed())
        .unwrap();
    memory
        .write_word(&constant(64), &constant(9), domain.borrowed())
        .unwrap();
    let original = memory.clone();
    let low = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        domain.spec().with_relations(RelationLimits {
            max_depth: 1,
            ..policy()
        }),
    ));
    assert!(matches!(
        memory.read_word_checked(&constant(0), low.borrowed()),
        Err(RangeError::Expression(ExprError::Limit { .. }))
    ));
    assert!(matches!(
        memory.write_word(&constant(0), &constant(11), low.borrowed()),
        Err(RangeError::State(StateError::Limit { .. }))
    ));
    assert_eq!(memory, original);
    assert!(matches!(
        memory.write_word(
            &TestValue::constant(U256::MAX),
            &constant(11),
            domain.borrowed()
        ),
        Err(RangeError::OffsetTooLarge(_))
    ));
    assert_eq!(memory, original);
    memory
        .expand(
            &TestValue::constant(U256::MAX),
            &constant(0),
            domain.borrowed(),
        )
        .unwrap();
    assert_eq!(memory, original);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            memory.read_word_checked(&constant(0), domain.borrowed()),
            Err(RangeError::Expression(ExprError::Cancelled))
        );
        assert!(
            memory
                .write_byte(&constant(0), &constant(11), domain.borrowed())
                .is_err()
        );
        assert_eq!(memory, original);
    });
}
