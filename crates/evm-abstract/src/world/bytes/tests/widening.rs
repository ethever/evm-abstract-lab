//! Cycle metadata canonicalization retains external facts and actual content.

use super::super::*;

#[test]
fn widening_form_is_idempotent_and_keeps_external_word_facts_and_history() {
    let domain = Domain::default();
    let gas = ExprId::fresh().unwrap();
    let value = AbstractValue::unsigned_range(U256::ZERO, U256::from(u64::MAX))
        .unwrap()
        .with_expression(gas.clone());
    let mut memory = ByteArray::memory();
    memory
        .write_word(&constant(0), &value, 1024, domain)
        .unwrap();
    let old_read = memory.read_word(&constant(0), domain);
    let unused = ExprId::fresh().unwrap();
    memory
        .operands
        .insert(unused.clone(), AbstractValue::constant(U256::ONE));
    let historical = memory.clone();
    let widened = memory.widening_form(domain, limits(domain)).unwrap();
    assert_eq!(widened.root, historical.root);
    assert_eq!(widened.length, historical.length);
    assert_eq!(widened.memory, historical.memory);
    assert!(widened.bytes.is_empty());
    assert!(widened.allocations.is_empty());
    assert_eq!(widened.operands.get(&gas), Some(&value));
    assert!(!widened.operands.contains_key(&unused));
    let read = widened.read_word(&constant(0), domain);
    assert_eq!(read.numeric(), old_read.numeric());
    assert_eq!(read.expression(), old_read.expression());
    assert!(!read.contains(U256::from(u64::MAX) + U256::ONE));
    assert_eq!(
        widened,
        widened.widening_form(domain, limits(domain)).unwrap()
    );
    assert_eq!(memory, historical);
}

#[test]
fn widening_form_preserves_observed_input_and_large_copy_without_expansion() {
    let domain = Domain::default();
    let source = super::byte_sequence(constant(1024 * 1024));
    let mut memory = ByteArray::memory();
    memory
        .copy_from(
            &constant(0),
            &source,
            &constant(0),
            source.len(),
            1024 * 1024,
            domain,
        )
        .unwrap();
    let root = memory.root.clone();
    let widened = memory.widening_form(domain, limits(domain)).unwrap();
    assert_eq!(widened.root, root);
    assert_eq!(widened.len(), memory.len());
    assert!(widened.bytes.is_empty());
    assert!(widened.work_size() < 1000);
    assert_eq!(
        widened.byte_at(7, domain).singleton(),
        Some(U256::from(0xaa))
    );
    assert_eq!(widened.fresh_leaves(), memory.root.fresh_leaves());
}

#[test]
fn widening_form_preserves_original_on_failure_cancel_and_low_policy() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory
        .write_word(&constant(0), &constant(7), 64, domain)
        .unwrap();
    let original = memory.clone();
    assert!(
        memory
            .widening_form(
                domain,
                ExprLimits {
                    max_nodes: 1,
                    max_depth: 1
                }
            )
            .is_err()
    );
    assert_eq!(memory, original);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert!(matches!(
            memory.widening_form(domain, limits(domain)),
            Err(RangeError::Expression(ExprError::Cancelled)
                | RangeError::State(StateError::Cancelled))
        ));
    });
    assert_eq!(memory, original);
    let failure = RangeError::TooLarge {
        requested: U256::from(99),
        limit: 32,
    };
    memory.failure = Some(failure.clone());
    let failed = memory.clone();
    assert_eq!(memory.widening_form(domain, limits(domain)), Err(failure));
    assert_eq!(memory, failed);
}

#[test]
fn widening_form_refreshes_projected_length_from_the_authoritative_constant() {
    let domain = Domain::default();
    let limits = limits(domain);
    let left = ByteArray::memory();
    let mut right = left.clone();
    right
        .expand(&constant(0), &constant(32), 64, domain)
        .unwrap();
    let selector = ExprId::fresh().unwrap();
    let joined = left
        .join_guarded(&right, domain, selector.clone(), limits)
        .unwrap();
    let mut projected = joined.clone();
    projected
        .rename_fresh(
            &BTreeMap::from([(selector, ExprId::constant(U256::ZERO))]),
            limits,
        )
        .unwrap();
    assert!(projected.len().contains(U256::ZERO));
    assert!(projected.len().contains(U256::from(32)));
    let expression = projected.len().expression().cloned();
    let widened = projected.widening_form(domain, limits).unwrap();
    let direct = right.widening_form(domain, limits).unwrap();
    assert_eq!(widened.root, direct.root);
    assert_eq!(widened.len().numeric(), direct.len().numeric());
    assert_eq!(widened.len().singleton(), Some(U256::from(32)));
    assert_eq!(widened.len().expression().cloned(), expression);
    assert_eq!(widened.len().provenance(), projected.len().provenance());
    assert_eq!(widened, widened.widening_form(domain, limits).unwrap());
    assert_eq!(joined.len().singleton(), None);
}
