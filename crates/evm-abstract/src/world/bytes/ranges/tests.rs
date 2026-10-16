use super::expanded;
use crate::domain::{
    AbstractValue, Domain, NumericValue,
    relational::{RelationLimits, RelationState, ValueQuery},
    symbolic::ExprId,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn range(lo: u64, hi: u64) -> AbstractValue {
    AbstractValue::unsigned_range(U256::from(lo), U256::from(hi))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap())
}

#[test]
fn proved_length_does_not_construct_an_irrelevant_oversized_maximum() {
    let mut spec = Domain::default().spec();
    let limits = RelationLimits {
        max_nodes: 128,
        ..spec.relations()
    };
    spec = spec.with_relations(limits);
    let domain = Domain::from_spec(spec);
    let mut source = ExprId::fresh().unwrap();
    for _ in 0..28 {
        source = ExprId::operation(
            opcode::ADD,
            &[source, ExprId::constant(U256::from(1))],
            limits.expressions(),
        )
        .unwrap();
    }
    let size = domain.join(&word(32), &word(64)).with_expression(source);
    let offset = domain
        .join(&word(0), &word(32))
        .with_expression(ExprId::fresh().unwrap());
    let actual = expanded(&word(128), &offset, &size, 4096, true, domain).unwrap();
    assert_eq!(actual.singleton(), Some(U256::from(128)));
}

#[test]
fn repeated_same_positive_symbolic_extent_keeps_one_length_expression() {
    let domain = Domain::default();
    let size = domain
        .join(&word(32), &word(64))
        .with_expression(ExprId::fresh().unwrap());
    let first = expanded(&word(0), &word(0), &size, 4096, true, domain).unwrap();
    let mut current = first.clone();
    for _ in 0..40 {
        current = expanded(&current, &word(0), &size, 4096, true, domain).unwrap();
        assert_eq!(current.expression(), first.expression());
        assert_eq!(current.numeric(), first.numeric());
    }
}

#[test]
fn symbolic_expansion_agrees_with_checked_word_rounding_and_zero_size() {
    let domain = Domain::default();
    let policy = domain.spec().relations();
    // Overlap, old-dominates, and new-dominates policies use independent inputs.
    for (old, offset, size, old_values, offsets, sizes) in [
        (
            range(32, 96),
            range(0, 64),
            range(0, 64),
            vec![32, 64, 96],
            vec![0, 1, 32, 64],
            vec![0, 1, 31, 32, 33, 64],
        ),
        (
            range(128, 192),
            range(0, 16),
            range(0, 64),
            vec![128, 160, 192],
            vec![0, 16],
            vec![0, 1, 64],
        ),
        (
            range(32, 64),
            range(128, 160),
            range(1, 32),
            vec![32, 64],
            vec![128, 160],
            vec![1, 32],
        ),
    ] {
        let output = expanded(&old, &offset, &size, 4096, true, domain).unwrap();
        let expression = output.symbolic_expression().unwrap();
        for previous in &old_values {
            for at in &offsets {
                for count in &sizes {
                    let expected = if *count == 0 {
                        *previous
                    } else {
                        (*previous).max((at + count + 31) / 32 * 32)
                    };
                    let mut relations = RelationState::default();
                    for (input, number) in [(&old, *previous), (&offset, *at), (&size, *count)] {
                        relations
                            .observe(
                                input.expression().unwrap(),
                                &NumericValue::constant(U256::from(number)),
                                &policy,
                            )
                            .unwrap();
                    }
                    assert_eq!(
                        relations.unique_value(&expression, &policy),
                        ValueQuery::Unique(U256::from(expected))
                    );
                }
            }
        }
    }
    let old = range(32, 96);
    let unchanged = expanded(
        &old,
        &AbstractValue::constant(U256::MAX),
        &word(0),
        4096,
        true,
        domain,
    )
    .unwrap();
    assert_eq!(unchanged, old);
}

#[test]
fn numeric_shortcuts_retain_cancellation_and_allocation_operand_limits() {
    use crate::domain::symbolic::{ExprError, ExprLimits};
    use crate::world::{ByteArray, RangeError};
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory.expand(&word(0), &word(128), 4096, domain).unwrap();
    let before = memory.clone();
    let size = domain
        .join(&word(32), &word(64))
        .with_expression(ExprId::fresh().unwrap());
    let cancel = analysis_control::Cancellation::new();
    cancel.cancel();
    let result = analysis_control::with_cancellation(&cancel, || {
        memory.expand(&word(0), &size, 4096, domain)
    });
    assert_eq!(result, Err(RangeError::Expression(ExprError::Cancelled)));
    assert_eq!(memory, before);
    let small = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_nodes: 4,
        ..domain.spec().relations()
    }));
    let mut source = ExprId::fresh().unwrap();
    for _ in 0..3 {
        source = ExprId::operation(
            opcode::ADD,
            &[source, ExprId::constant(U256::from(1))],
            ExprLimits::default(),
        )
        .unwrap();
    }
    let excessive = size.with_expression(source);
    assert!(matches!(
        memory.expand(&word(0), &excessive, 4096, small),
        Err(RangeError::Expression(ExprError::Limit { .. }))
    ));
    assert_eq!(memory, before);
}

#[test]
fn numeric_shortcuts_check_synthesized_allocation_expressions() {
    use crate::domain::symbolic::ExprError;
    use crate::world::{ByteArray, RangeError};

    let domain = Domain::default();
    let size = domain.join(&word(32), &word(64));
    assert!(size.expression().is_none());
    let allocation = super::super::expr(&size).unwrap();
    assert_eq!(allocation.depth(), 3);
    let small = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_depth: 2,
        ..domain.spec().relations()
    }));
    let mut memory = ByteArray::memory();
    memory.expand(&word(0), &word(128), 4096, domain).unwrap();
    let before = memory.clone();
    let result = memory.expand(&word(0), &size, 4096, small);
    assert!(
        matches!(
            result,
            Err(RangeError::Expression(ExprError::Limit { depth: 3, .. }))
        ),
        "expected retained allocation limit, received {result:?}"
    );
    assert_eq!(memory, before);
}
