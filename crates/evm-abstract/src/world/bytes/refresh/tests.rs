use super::{charge, equality_work, run};
use crate::{
    domain::{
        AbstractValue, Domain, NumericValue,
        facts::{BitConstraints, UnaryPredicate},
        identity::Symbol,
        provenance::Origin,
        relational::RelationLimits,
        symbolic::{ByteStateKind, ExprError, ExprId, ExprLimits},
    },
    resource::WorkBudget,
    test_instrumentation::Counter,
    world::{ByteArray, RangeError, bytes::write::NUMERIC_READS},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

#[derive(Clone, Copy, Default)]
pub(super) struct Stats {
    pub(super) epochs: usize,
    pub(super) shared: usize,
    pub(super) fresh: usize,
    warm: usize,
    cold_starts: usize,
    entries: usize,
    retained: usize,
    cancel_after: usize,
}

pub(super) static EPOCHS: Counter<Stats> = Counter::new(Stats {
    epochs: 0,
    shared: 0,
    fresh: 0,
    warm: 0,
    cold_starts: 0,
    entries: 0,
    retained: 0,
    cancel_after: 0,
});

pub(super) fn observed(reader: &super::numeric::Reader<'_>, before: (usize, usize), shared: bool) {
    let (entries, retained) = reader.cache_stats();
    let mut cancel = false;
    EPOCHS.update(|stats| {
        stats.shared += usize::from(shared);
        stats.warm += usize::from(shared && before.0 != 0);
        stats.cold_starts += usize::from(!shared && before == (0, 0));
        stats.entries = stats.entries.max(entries);
        stats.retained = stats.retained.max(retained);
        if stats.cancel_after != 0 {
            stats.cancel_after -= 1;
            cancel = stats.cancel_after == 0;
        }
    });
    if cancel {
        analysis_control::current_cancellation().cancel();
    }
}

fn word(n: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(n))
}

fn offset() -> AbstractValue {
    AbstractValue::unsigned_range(U256::ZERO, U256::from(31))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap())
}

fn source() -> AbstractValue {
    // A non-Top word with a forced bit in every byte must keep the existing
    // numeric word-byte union fallback, independent of the full-image shortcut.
    let mut value = Domain::default()
        .from_facts(&[UnaryPredicate::KnownBits(
            BitConstraints::new(U256::ZERO, U256::from_be_bytes([0x20; 32])).unwrap(),
        )])
        .unwrap();
    value.set_origin(Origin::Calldata);
    value
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(981))
        .with_identity(981, 7)
}

fn memory(domain: Domain) -> ByteArray {
    let mut memory = ByteArray::memory();
    memory
        .write_word(&word(0), &word(0x1234), 128, domain)
        .unwrap();
    memory
}

fn differential(staged: ByteArray, domain: Domain) -> ByteArray {
    // Share the EXACT staged root/source identities with the original oracle.
    let saved = staged.clone();
    let mut expected = staged.clone();
    expected.refresh(domain);
    let mut actual = staged;
    assert!(run(&mut actual, domain, &mut WorkBudget::new(1_000_000_000)).unwrap());
    assert_eq!(actual, expected);
    assert_eq!(actual.root, saved.root);
    assert_eq!(actual.operands, saved.operands);
    assert_eq!(actual.conditional, saved.conditional);
    assert_eq!(actual.default, saved.default);
    assert_eq!(actual.length, saved.length);
    assert_eq!(actual.allocations, saved.allocations);
    assert_eq!(
        actual.summary_projection(domain),
        expected.summary_projection(domain)
    );
    for index in [0, 31, 32, 63, 64, 127] {
        assert_eq!(
            actual.byte_at(index, domain),
            expected.byte_at(index, domain)
        );
    }
    assert_eq!(actual.byte_at(127, domain).singleton(), Some(U256::ZERO));
    actual
}

#[test]
fn original_refresh_matches_whole_staged_history_guards_and_saved_snapshots() {
    let domain = Domain::default();
    let mut before = memory(domain);
    let original = before.clone();
    let input = ByteArray::exact(&[0x80, 0x22, 0x11]);
    before
        .copy_from(&word(28), &input, &word(0), &word(8), 128, domain)
        .unwrap();
    assert_eq!(input.byte_at(3, domain).singleton(), Some(U256::ZERO));
    let guard = ExprId::fresh().unwrap();
    let guarded = before
        .join_guarded(&original, domain, guard, ExprLimits::default())
        .unwrap();
    let at = offset();
    let value = source();
    for prior in [before.clone(), guarded] {
        let saved = prior.clone();
        let staged = prior.staged_write(&at, &value, 32, 128, domain).unwrap();
        assert!(
            matches!(staged.root.kind(), ByteStateKind::WriteWord { previous, .. }
            if previous == saved.state_root())
        );
        let actual = differential(staged, domain);
        assert_eq!(prior, saved);
        assert_eq!(actual.is_memory(), saved.is_memory());
        assert!(actual.len().contains(U256::from(64)));
    }
    assert_eq!(
        original.byte_at(31, domain).singleton(),
        Some(U256::from(0x34))
    );
    assert_eq!(before.byte_at(31, domain).singleton(), Some(U256::ZERO));
    // Finite views keep their own zero padding after the same symbolic write.
    differential(
        ByteArray::exact(&[1, 2, 3])
            .staged_write(&at, &value, 32, 128, domain)
            .unwrap(),
        domain,
    );
}

#[test]
fn cacheable_non_top_words_reuse_one_reader_with_every_cell_read_and_new_epochs() {
    let _stats = EPOCHS.scope();
    let _reads = NUMERIC_READS.scope();
    let domain = Domain::default();
    let before = memory(domain);
    let at = offset();
    let value = source();
    assert!(!value.numeric().is_top());
    assert!(value.finite_constants().is_top());
    let staged = before.staged_write(&at, &value, 32, 128, domain).unwrap();
    let mut expected = staged.clone();
    NUMERIC_READS.set(0);
    expected.refresh(domain);
    assert_eq!(NUMERIC_READS.get(), 32);
    NUMERIC_READS.set(0);
    let mut actual = staged;
    assert!(run(&mut actual, domain, &mut WorkBudget::new(1_000_000_000)).unwrap());
    assert_eq!(NUMERIC_READS.get(), 32);
    assert_eq!(actual, expected);
    let stats = EPOCHS.get();
    assert_eq!(
        (stats.epochs, stats.cold_starts, stats.shared, stats.fresh),
        (1, 1, 31, 0)
    );
    assert_eq!(stats.warm, 31);
    assert_eq!(stats.entries, 1);
    assert!(stats.retained > 0 && stats.retained <= 8192);
    let saved = actual.clone();
    expected = actual
        .staged_write(&word(64), &word(7), 1, 128, domain)
        .unwrap();
    expected.refresh(domain);
    NUMERIC_READS.set(0);
    assert!(
        actual
            .write_byte_budgeted(
                &word(64),
                &word(7),
                128,
                domain,
                &mut WorkBudget::new(1_000_000_000)
            )
            .unwrap()
    );
    assert_eq!(NUMERIC_READS.get(), 33);
    assert_eq!(actual, expected);
    assert_eq!((EPOCHS.get().epochs, EPOCHS.get().cold_starts), (2, 2));
    assert_ne!(actual.root, saved.root);
    assert_eq!(saved.byte_at(64, domain).singleton(), Some(U256::ZERO));

    // Existing A1 still evaluates Reader once; the old oracle calls it 32 times.
    let top = value.with_numeric(NumericValue::top());
    let mut fresh = ByteArray::memory();
    NUMERIC_READS.set(0);
    assert!(
        fresh
            .write_word_budgeted(
                &word(0),
                &top,
                128,
                domain,
                &mut WorkBudget::new(1_000_000_000)
            )
            .unwrap()
    );
    assert_eq!(NUMERIC_READS.get(), 1);
    assert_eq!(EPOCHS.get().epochs, 2);
    let mut old = ByteArray::memory()
        .staged_write(&word(0), &top, 32, 128, domain)
        .unwrap();
    NUMERIC_READS.set(0);
    old.refresh(domain);
    assert_eq!(NUMERIC_READS.get(), 32);
    assert_eq!(fresh, old);
}

#[test]
fn full_facts_mismatch_and_changed_guard_policy_keep_independent_scopes() {
    let _stats = EPOCHS.scope();
    let domain = Domain::default();
    let mut staged = memory(domain)
        .staged_write(&offset(), &source(), 32, 128, domain)
        .unwrap();
    // A valid noncanonical constant-key AV loses its expression when index zero
    // is remembered. Numeric equality alone would incorrectly admit later cells.
    staged.operands.insert(
        ExprId::constant(U256::ZERO),
        word(0)
            .with_origin(Origin::Storage)
            .with_expression(ExprId::fresh().unwrap()),
    );
    let (_, zero_facts) = staged.prepare_byte_read(&word(0), domain).unwrap();
    let (_, one_facts) = staged.prepare_byte_read(&word(1), domain).unwrap();
    let key = ExprId::constant(U256::ZERO);
    assert_eq!(zero_facts[&key].numeric(), one_facts[&key].numeric());
    assert_ne!(zero_facts, one_facts);
    let actual = differential(staged.clone(), domain);
    assert_eq!(EPOCHS.get().fresh, 31);
    assert_eq!(EPOCHS.get().shared, 0);
    assert!(actual.operands[&key].expression().is_some());

    let changed = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_depth: domain.spec().relations().max_depth + 1,
        ..domain.spec().relations()
    }));
    let other = memory(domain);
    let guarded = staged
        .join_guarded(
            &other,
            domain,
            ExprId::fresh().unwrap(),
            ExprLimits::default(),
        )
        .unwrap();
    differential(guarded, changed);
    // The entire Context, Reader and cache are reconstructed for this root,
    // conditional environment and complete (including relation policy) Domain.
    assert_eq!((EPOCHS.get().epochs, EPOCHS.get().cold_starts), (2, 2));
}

#[test]
fn optional_declines_keep_old_work_and_actual_stops_abort_without_poisoning_later_epochs() {
    let _stats = EPOCHS.scope();
    let _reads = NUMERIC_READS.scope();
    let domain = Domain::default();
    let before = memory(domain);
    let saved = before.clone();
    let at = offset();
    let value = source();
    let staged = before.staged_write(&at, &value, 32, 128, domain).unwrap();
    let mut expected = staged.clone();
    expected.refresh(domain);
    let census = staged.operands.len() * 8 + 8;
    let total = census + super::EPOCH_WORK + (equality_work(&staged) + 8) * staged.bytes.len();
    for cut in [0, census - 1, census, total - 1] {
        let mut actual = before.clone();
        let mut budget = WorkBudget::new(cut + 7);
        assert!(budget.charge(7));
        NUMERIC_READS.set(0);
        assert_eq!(
            actual.write_word_budgeted(&at, &value, 128, domain, &mut budget),
            Ok(true)
        );
        assert!(!budget.exhausted());
        assert_eq!(budget.used(), 7 + if cut < census { 0 } else { census });
        assert_eq!(actual, expected);
        assert_eq!(saved, before);
        assert_eq!(NUMERIC_READS.get(), 32);
        assert_eq!(EPOCHS.get().epochs, 0);
    }
    // An ACTUAL original-payer refusal is sticky, even when the old read path
    // would otherwise fit. It cannot become an optional decline or fallback.
    let mut stopped_budget = WorkBudget::new(7);
    assert!(stopped_budget.charge(7));
    assert_eq!(charge(&mut stopped_budget, 1), Ok(false));
    let mut stopped = before.clone();
    NUMERIC_READS.set(0);
    assert_eq!(
        stopped.write_word_budgeted(&at, &value, 128, domain, &mut stopped_budget),
        Ok(false)
    );
    assert_eq!(stopped, before);
    assert!(stopped_budget.exhausted());
    assert_eq!(stopped_budget.used(), 7);
    assert_eq!(NUMERIC_READS.get(), 0);
    EPOCHS.set(Stats {
        cancel_after: 2,
        ..Stats::default()
    });
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut actual = before.clone();
        let mut budget = WorkBudget::new(total + 7);
        assert!(budget.charge(7));
        NUMERIC_READS.set(0);
        assert_eq!(
            actual.write_word_budgeted(&at, &value, 128, domain, &mut budget),
            Err(RangeError::Expression(ExprError::Cancelled))
        );
        assert_eq!(NUMERIC_READS.get(), 2);
        assert_eq!(actual, before);
        assert!(budget.exhausted());
        assert_eq!(budget.used(), total + 7);
    });
    EPOCHS.set(Stats::default());
    let mut actual = before.clone();
    let mut budget = WorkBudget::new(total + 7);
    assert!(budget.charge(7));
    assert!(
        actual
            .write_word_budgeted(&at, &value, 128, domain, &mut budget)
            .unwrap()
    );
    assert_eq!(budget.used(), total + 7);
    assert_eq!(actual, expected);
    assert!(
        actual
            .bytes
            .values()
            .all(|byte| !byte.symbolic_limit_reached())
    );
    assert_eq!((EPOCHS.get().epochs, EPOCHS.get().cold_starts), (1, 1));
    assert_eq!(saved, before);
}

#[test]
fn cache_capacity_refusal_and_checked_read_markers_keep_original_results() {
    let _stats = EPOCHS.scope();
    let domain = Domain::from_spec(Domain::default().spec().with_relations(RelationLimits {
        max_nodes: 1_000_000,
        ..Domain::default().spec().relations()
    }));
    let mut expression = ExprId::fresh().unwrap();
    for _ in 0..13 {
        expression = ExprId::operation(
            opcode::ADD,
            &[expression.clone(), expression],
            ExprLimits {
                max_nodes: 1_000_000,
                max_depth: 256,
            },
        )
        .unwrap();
    }
    assert!(expression.work_size() > 8192);
    let value = source().with_expression(expression);
    let staged = memory(domain)
        .staged_write(&offset(), &value, 32, 128, domain)
        .unwrap();
    differential(staged, domain);
    assert_eq!(
        (
            EPOCHS.get().entries,
            EPOCHS.get().retained,
            EPOCHS.get().warm
        ),
        (0, 0, 0)
    );
    assert_eq!(EPOCHS.get().shared, 31);

    // Staging fits but checked byte expressions do not: keep the existing
    // symbolic-limit compatibility marker rather than a successful Top sentinel.
    let normal = Domain::default();
    let limited = Domain::from_spec(normal.spec().with_relations(RelationLimits {
        max_nodes: 4,
        ..normal.spec().relations()
    }));
    let value = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    let before = ByteArray::memory();
    let mut expected = before
        .staged_write(&word(0), &value, 1, 128, limited)
        .unwrap();
    expected.refresh(limited);
    let mut actual = before;
    assert!(
        actual
            .write_byte_budgeted(
                &word(0),
                &value,
                128,
                limited,
                &mut WorkBudget::new(1_000_000_000)
            )
            .unwrap()
    );
    assert_eq!(actual, expected);
    assert!(
        actual
            .bytes
            .values()
            .all(AbstractValue::symbolic_limit_reached)
    );
}
