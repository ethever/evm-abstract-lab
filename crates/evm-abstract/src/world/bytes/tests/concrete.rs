//! Concrete effects must not repeatedly embed unrelated read history.

use super::super::*;
use crate::domain::{
    identity::Symbol,
    provenance::Origin,
    relational::{CheckResult, RelationLimits, RelationState, ValueQuery},
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

fn domain(policy: RelationLimits) -> Domain {
    Domain::from_spec(Domain::default().spec().with_relations(policy))
}

fn observed(scope: u64, origin: Origin) -> AbstractValue {
    let mut value = AbstractValue::top();
    value.set_origin(origin);
    value
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
        .with_numeric(constant(7).numeric().clone())
}

fn prove(relations: &RelationState, value: &AbstractValue, expected: U256, limits: RelationLimits) {
    assert_eq!(
        relations.local_value(&expr(value).unwrap(), &limits),
        ValueQuery::Exact(expected)
    );
    assert_eq!(
        relations.can_equal(&expr(value).unwrap(), expected ^ U256::ONE, &limits),
        CheckResult::ProvenImpossible
    );
}

#[test]
fn concrete_free_pointer_history_stays_bounded_at_original_budgets() {
    let limits = policy();
    let domain = domain(limits);
    let mut memory = ByteArray::memory();
    memory
        .write_word(&constant(64), &constant(128), 4096, domain)
        .unwrap();
    let mut historical = Vec::new();
    // On 03cdf, iteration7's offset write needs32067 nodes, beyond16384.
    for iteration in 0..48 {
        let pointer = memory.read_word_checked(&constant(64), domain).unwrap();
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
            memory.write_word(&offset, &value, 4096, domain).unwrap();
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
    let mut memory = ByteArray::memory();
    memory.write_word(&constant(0), &a, 256, domain).unwrap();
    let old = memory.read_word_checked(&constant(0), domain).unwrap();
    memory.write_word(&constant(32), &b, 256, domain).unwrap();
    memory.write_byte(&constant(128), &a, 256, domain).unwrap();
    memory
        .write_word(&constant(160), &constant(99), 256, domain)
        .unwrap();
    let saved = memory.clone();
    let form = memory.widening_form(domain, limits.expressions()).unwrap();
    for current in [&memory, &form] {
        let mut relations = RelationState::default();
        for (expression, value) in current.symbolic_facts() {
            relations
                .observe(expression, value.numeric(), &limits)
                .unwrap();
        }
        assert_eq!(relations.check(&limits), CheckResult::MayBeFeasible);
        for (offset, original, origin) in [(0, &a, Origin::Calldata), (32, &b, Origin::Returndata)]
        {
            assert_eq!(
                current.operands.get(original.expression().unwrap()),
                Some(original)
            );
            let read = current
                .read_word_checked(&constant(offset), domain)
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
        let byte = current.read_byte_checked(&constant(128), domain).unwrap();
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
                .read_word(&constant(0), domain)
                .same_identity(&current.read_word(&constant(32), domain))
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
    let offset = AbstractValue::unsigned_range(U256::ZERO, U256::from(64))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let value = AbstractValue::unsigned_range(U256::from(7), U256::from(8))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let mut memory = ByteArray::memory();
    memory.write_word(&offset, &value, 96, domain).unwrap();
    let read = memory.read_word_checked(&constant(0), domain).unwrap();
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
    let mut yes = ByteArray::memory();
    yes.write_word(&constant(0), &constant(7), 32, domain)
        .unwrap();
    let mut no = ByteArray::memory();
    no.write_word(&constant(0), &constant(11), 32, domain)
        .unwrap();
    let joined = yes
        .join_guarded(&no, domain, selector.clone(), limits.expressions())
        .unwrap();
    let read = joined.read_word_checked(&constant(0), domain).unwrap();
    assert!(read.singleton().is_none());
    assert!(matches!(
        joined.state_root().kind(),
        ByteStateKind::Ite { .. }
    ));
    for (nonzero, expected) in [(true, 7), (false, 11)] {
        let mut relations = RelationState::default();
        assert_eq!(
            relations.assume(&selector, nonzero, &limits),
            CheckResult::MayBeFeasible
        );
        prove(&relations, &read, U256::from(expected), limits);
    }
}

#[test]
fn overlaps_padding_and_overwritten_history_keep_their_own_values() {
    let limits = policy();
    let domain = domain(limits);
    let mut memory = ByteArray::memory();
    memory
        .write_word(
            &constant(0),
            &AbstractValue::constant(U256::MAX),
            96,
            domain,
        )
        .unwrap();
    let old = memory.read_word_checked(&constant(1), domain).unwrap();
    let old_hash = memory
        .slice(&constant(0), &constant(32), 96, domain)
        .unwrap()
        .keccak(domain, 32)
        .unwrap();
    let snapshot = memory.clone();
    memory
        .write_byte(&constant(31), &constant(42), 96, domain)
        .unwrap();
    let overlapping = memory.state_root().clone();
    memory
        .write_word(&constant(64), &constant(9), 96, domain)
        .unwrap();
    let read = memory.read_word_checked(&constant(0), domain).unwrap();
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
        .slice(&constant(0), &constant(32), 96, domain)
        .unwrap()
        .keccak(domain, 32)
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
        &AbstractValue::top().with_expression(short),
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
        &AbstractValue::top().with_expression(read),
        U256::from(255) << 248,
        limits,
    );
}

#[test]
fn normalization_keeps_retention_and_cancellation_checks_and_failed_writes_atomic() {
    let domain = domain(policy());
    let mut memory = ByteArray::memory();
    memory
        .write_word(&constant(0), &constant(7), 96, domain)
        .unwrap();
    memory
        .write_word(&constant(64), &constant(9), 96, domain)
        .unwrap();
    let original = memory.clone();
    let low = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_depth: 1,
        ..policy()
    }));
    assert!(matches!(
        memory.read_word_checked(&constant(0), low),
        Err(RangeError::Expression(ExprError::Limit { .. }))
    ));
    assert!(matches!(
        memory.write_word(&constant(0), &constant(11), 96, low),
        Err(RangeError::State(StateError::Limit { .. }))
    ));
    assert_eq!(memory, original);
    assert!(matches!(
        memory.write_word(
            &AbstractValue::constant(U256::MAX),
            &constant(11),
            96,
            domain
        ),
        Err(RangeError::OffsetTooLarge(_))
    ));
    assert_eq!(memory, original);
    memory
        .expand(
            &AbstractValue::constant(U256::MAX),
            &constant(0),
            96,
            domain,
        )
        .unwrap();
    assert_eq!(memory, original);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            memory.read_word_checked(&constant(0), domain),
            Err(RangeError::Expression(ExprError::Cancelled))
        );
        assert!(
            memory
                .write_byte(&constant(0), &constant(11), 96, domain)
                .is_err()
        );
        assert_eq!(memory, original);
    });
}
