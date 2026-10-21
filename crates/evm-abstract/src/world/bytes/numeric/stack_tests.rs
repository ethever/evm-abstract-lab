//! Legal content depth must not become native call-stack depth.

use super::{Facts, Reader, numeric_domain};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use {
    crate::domain::provenance::Origin, crate::domain::relational::RelationLimits,
    crate::domain::symbolic::ByteStateId, crate::domain::symbolic::ByteView,
    crate::domain::symbolic::ExprId, crate::domain::symbolic::StateLimits,
    crate::world::test_support::TestAlgebra, crate::world::test_support::TestValue,
};

use crate::{
    domain::{ValueAlgebra, symbolic::ExprError},
    resource::WorkBudget,
    world::bytes::conditional::{self, ConditionalFacts, Context},
};

fn term(value: usize) -> ExprId {
    ExprId::constant(U256::from(value))
}

fn domain() -> TestAlgebra {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    crate::world::test_support::numeric_product(crate::domain::ValueAlgebra::new(
        crate::domain::ProductAlgebra::from_spec(domain.spec().with_relations(RelationLimits {
            max_nodes: 65_536,
            max_depth: 512,
            ..domain.spec().relations()
        })),
    ))
}

#[test]
fn deep_copy_sources_preserve_word_identity_bytes_and_padding_on_the_default_stack() {
    let depth = 256;
    let limits = StateLimits {
        max_nodes: 65_536,
        max_depth: 512,
    };
    let domain = domain();
    let key = ExprId::fresh().unwrap();
    let value = TestValue::unsigned_range(U256::from(3), U256::from(9))
        .unwrap()
        .with_origin(Origin::Storage)
        .with_identity(982_140, 1)
        .with_expression(key.clone());
    let facts = Facts::from([(key.clone(), value.clone())]);
    let mut state = ByteStateId::zero()
        .write_word(term(depth), key, limits)
        .unwrap();
    for _ in 0..depth {
        // A shifted source cannot canonicalize to the complete previous copy.
        // Every layer remains observable, within the ordinary admitted policy.
        state = ByteStateId::zero()
            .copy(
                term(0),
                ByteView::new(state, term(1), term(1024)),
                term(1024),
                limits,
            )
            .unwrap();
    }
    assert_eq!(state.depth(), depth + 2);
    assert!(state.nodes() < limits.max_nodes);
    let frozen = state.clone();
    let mut reader = Reader::new(&facts, domain.borrowed());
    let word = reader.word(&state, &term(0));
    assert_eq!(word, value);
    assert!(word.same_identity(&value));
    for index in [0, 30, 31] {
        let expected = domain.observe_byte(
            opcode::BYTE,
            &[TestValue::constant(U256::from(index)), value.clone()],
        );
        assert_eq!(reader.read(&state, &term(index)), expected);
    }
    for index in [32, 1023, 1024] {
        assert_eq!(
            reader.read(&state, &term(index)).singleton(),
            Some(U256::ZERO)
        );
    }
    assert_eq!(state, frozen);
    assert_eq!(reader.cache_stats(), (0, 0));
}

#[test]
fn deep_selected_snapshots_keep_their_branch_context_and_whole_word() {
    let limits = StateLimits {
        max_nodes: 65_536,
        max_depth: 512,
    };
    let domain = domain();
    let key = ExprId::fresh().unwrap();
    let value = TestValue::unsigned_range(U256::from(3), U256::from(9))
        .unwrap()
        .with_origin(Origin::Returndata)
        .with_identity(982_140, 2);
    let mut facts = Facts::from([(key.clone(), value.clone())]);
    let mut state = ByteStateId::zero()
        .write_word(term(0), key, limits)
        .unwrap();
    for _ in 0..256 {
        let guard = ExprId::fresh().unwrap();
        facts.insert(guard.clone(), TestValue::constant(U256::ONE));
        state = state
            .join_guarded(&ByteStateId::zero(), guard, limits)
            .unwrap();
    }
    assert!(state.depth() >= 256 && state.depth() <= limits.max_depth);
    let mut reader = Reader::new(&facts, domain.borrowed());
    assert_eq!(reader.word(&state, &term(0)), value);
    assert_eq!(
        reader.read(&state, &term(31)),
        domain.observe_byte(opcode::BYTE, &[TestValue::constant(U256::from(31)), value],)
    );
    assert_eq!(reader.cache_stats(), (0, 0));
}

#[test]
fn cancellation_after_a_deep_branch_and_copy_walk_stops_the_original_demand() {
    let domain = domain();
    let limits = StateLimits {
        max_nodes: 65_536,
        max_depth: 512,
    };
    let key = ExprId::fresh().unwrap();
    let mut facts = Facts::from([(
        key.clone(),
        TestValue::unsigned_range(U256::from(3), U256::from(9)).unwrap(),
    )]);
    let mut state = ByteStateId::zero()
        .write_word(term(64), key, limits)
        .unwrap();
    for _ in 0..64 {
        state = ByteStateId::zero()
            .copy(
                term(0),
                ByteView::new(state, term(1), term(1024)),
                term(1024),
                limits,
            )
            .unwrap();
    }
    for _ in 0..8 {
        let guard = ExprId::fresh().unwrap();
        facts.insert(guard.clone(), TestValue::constant(U256::ONE));
        state = state
            .join_guarded(&ByteStateId::zero(), guard, limits)
            .unwrap();
    }
    let frozen = facts.clone();
    let conditional = ConditionalFacts::default();
    let algebra = domain.borrowed();
    let preparation = conditional::preparation(
        state.work_size(),
        &TestValue::constant(U256::from(31)),
        facts
            .iter()
            .map(|(key, value)| key.work_size() + value.work_size())
            .sum(),
        facts.len(),
        &conditional,
        algebra,
    );
    let mut budget = WorkBudget::new(usize::MAX);
    let root = Context::budgeted(&facts, &conditional, algebra, &preparation, &mut budget)
        .unwrap()
        .unwrap();
    let cancellation = analysis_control::Cancellation::new();
    root.cancel_after_numeric_payments(1, cancellation.clone());
    let mut reader = Reader::with_context(root.clone(), algebra);
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(reader.read(&state, &term(31)), TestValue::top());
        assert_eq!(root.check(), Err(ExprError::Cancelled));
        assert!(!root.cache_refused());
        assert!(!root.work_exhausted());
        let paid = root.demand_payments();
        assert_eq!(paid.iter().filter(|(numeric, _)| *numeric).count(), 1);
        assert_eq!(reader.word(&state, &term(0)), TestValue::top());
        assert_eq!(root.demand_payments(), paid);
    });
    assert_eq!(facts, frozen);
    assert_eq!(reader.cache_stats(), (0, 0));
}
