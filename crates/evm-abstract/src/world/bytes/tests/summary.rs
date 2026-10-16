//! Canonical byte payloads cover fresh inputs while retaining length/padding.

use super::super::*;
use crate::domain::relational::{CheckResult, RelationLimits, RelationState};

#[test]
fn arbitrary_inputs_project_equally_and_instantiate_independently() {
    let domain = Domain::default();
    let first = ByteArray::unknown_checked()
        .unwrap()
        .summary_projection(domain)
        .unwrap();
    let second = ByteArray::unknown_checked()
        .unwrap()
        .summary_projection(domain)
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.join(&second, domain, false), first);
    let live = first.instantiate(domain).unwrap();
    let sibling = first.instantiate(domain).unwrap();
    assert_eq!(live.len().expression().unwrap().nodes(), 1);
    assert!(live.state_root().nodes() < 8);
    assert_ne!(live.state_root(), sibling.state_root());
    assert_ne!(live.len().expression(), sibling.len().expression());
    assert_eq!(live.summary_projection(domain).unwrap(), first);
}

#[test]
fn byte_join_preserves_exact_word_facts_and_length_zero_padding() {
    let domain = Domain::default();
    let mut one = [0; 32];
    one[31] = 1;
    let mut two = [0; 32];
    two[31] = 2;
    let first = ByteArray::exact(&one).summary_projection(domain).unwrap();
    let second = ByteArray::exact(&two).summary_projection(domain).unwrap();
    let joined = first.join(&second, domain, false);
    assert!(joined.covers(&first, domain));
    assert!(joined.covers(&second, domain));
    let live = joined.instantiate(domain).unwrap();
    assert_eq!(live.byte_at(32, domain).singleton(), Some(U256::ZERO));
    let word = live.read_word(&constant(0), domain);
    assert!(word.contains(U256::from(1)) && word.contains(U256::from(2)));
    assert!(!word.contains(U256::from(3)));
    assert_eq!(live.summary_projection(domain).unwrap(), joined);
    assert_eq!(joined.join(&joined, domain, true), joined);
}

#[test]
fn memory_projection_keeps_exact_contents_and_msize_but_drops_path_allocations() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory
        .write_word(&constant(0), &constant(9), 1024, domain)
        .unwrap();
    memory
        .expand(&constant(65), &constant(1), 1024, domain)
        .unwrap();
    let summary = memory.summary_projection(domain).unwrap();
    let live = summary.instantiate(domain).unwrap();
    assert!(live.is_memory());
    assert_eq!(live.len().singleton(), Some(U256::from(96)));
    assert_eq!(
        live.read_word(&constant(0), domain).singleton(),
        Some(U256::from(9))
    );
    assert_eq!(live.byte_at(64, domain).singleton(), Some(U256::ZERO));
    assert_eq!(summary, live.summary_projection(domain).unwrap());
}

#[test]
fn summaries_cover_length_and_raw_root_even_when_numeric_cache_is_empty() {
    let domain = Domain::default();
    let first = ByteArray::exact(&[1, 2])
        .summary_projection(domain)
        .unwrap();
    let second = ByteArray::exact(&[1, 2, 3])
        .summary_projection(domain)
        .unwrap();
    assert!(!first.covers(&second, domain));
    let mut memory = ByteArray::memory();
    memory
        .write_byte(&constant(0), &constant(7), 64, domain)
        .unwrap();
    memory.bytes.clear();
    let live = memory
        .summary_projection(domain)
        .unwrap()
        .instantiate(domain)
        .unwrap();
    assert_eq!(live.byte_at(0, domain).singleton(), Some(U256::from(7)));
    assert_eq!(live.len().singleton(), Some(U256::from(32)));
}

#[test]
fn numerical_projection_compresses_history_under_a_smaller_replay_limit() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    for index in 1..=20 {
        memory
            .write_byte(&constant(index), &constant(9), 64, domain)
            .unwrap();
    }
    let bounds = AbstractValue::unsigned_range(U256::from(7), U256::from(8)).unwrap();
    memory
        .write_byte(&constant(0), &bounds, 64, domain)
        .unwrap();
    let data = memory
        .slice(&constant(0), &constant(1), 64, domain)
        .unwrap();
    let smaller = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_nodes: 48,
        ..RelationLimits::default()
    }));
    assert!(data.state_root().nodes() > smaller.spec().relations().max_nodes);
    assert!(data.read_byte_checked(&constant(0), smaller).is_err());
    let summary = data.summary_projection(smaller).unwrap();
    let replay = summary.instantiate(smaller).unwrap();
    assert!(replay.state_root().nodes() <= smaller.spec().relations().max_nodes);
    assert!(replay.byte_at(0, smaller).contains(U256::from(7)));
    assert!(replay.byte_at(0, smaller).contains(U256::from(8)));
    assert_eq!(replay.byte_at(1, smaller).singleton(), Some(U256::ZERO));
    assert!(summary.work_size() >= replay.work_size());
}

#[test]
fn constant_runs_use_small_roots_and_keep_variable_length_padding() {
    let domain = Domain::default();
    let length = AbstractValue::unsigned_range(U256::from(32), U256::from(64)).unwrap();
    let length_expression = expr(&length).unwrap();
    let cells = (0..64)
        .map(|index| (index, constant(usize::from(index == 31) * 9)))
        .collect::<BTreeMap<_, _>>();
    let mut input = ByteArray::unknown();
    input.length = length.with_expression(length_expression.clone());
    input.root = ByteStateId::observed(
        SourceId::fresh().unwrap(),
        length_expression.clone(),
        cells
            .iter()
            .map(|(index, value)| crate::domain::symbolic::StateBinding {
                index: U256::from(*index),
                value: ExprId::constant(value.singleton().unwrap()),
                numeric: value.numeric().clone(),
            })
            .collect(),
        limits(domain).into(),
    )
    .unwrap();
    input.bytes = cells;
    input
        .operands
        .insert(length_expression, input.length.clone());
    let summary = input.summary_projection(domain).unwrap();
    let replay = summary.instantiate(domain).unwrap();
    assert!(replay.state_root().nodes() < 128);
    assert_eq!(replay.byte_at(31, domain).singleton(), Some(U256::from(9)));
    assert_eq!(replay.byte_at(64, domain).singleton(), Some(U256::ZERO));
    assert_eq!(replay.summary_projection(domain).unwrap(), summary);
    assert!(summary.work_size() >= replay.work_size());
    for input in [
        ByteArray::memory(),
        ByteArray::exact(&[1, 0, 2]),
        ByteArray::unknown(),
    ] {
        let summary = input.summary_projection(domain).unwrap();
        assert!(summary.work_size() >= summary.instantiate(domain).unwrap().work_size());
    }
}

#[test]
fn mixed_byte_constraints_use_independent_leaves_and_keep_native_numeric_facts() {
    let domain = Domain::default();
    let bounded = domain.join(&constant(7), &constant(9));
    let length = AbstractValue::unsigned_range(U256::from(128), U256::from(129)).unwrap();
    let length_expression = expr(&length).unwrap();
    let cells = (0..128)
        .map(|index| {
            let value = match index {
                0..32 => bounded.clone(),
                32..64 => constant(0xaa),
                64..96 => constant(0xbb),
                _ => zero(),
            };
            (index, value)
        })
        .collect::<BTreeMap<_, _>>();
    let mut input = ByteArray::unknown();
    input.length = length.with_expression(length_expression.clone());
    input.root = ByteStateId::observed(
        SourceId::fresh().unwrap(),
        length_expression.clone(),
        cells
            .iter()
            .map(|(index, value)| crate::domain::symbolic::StateBinding {
                index: U256::from(*index),
                value: value
                    .singleton()
                    .map(ExprId::constant)
                    .unwrap_or_else(|| ExprId::fresh().unwrap()),
                numeric: value.numeric().clone(),
            })
            .collect(),
        crate::domain::symbolic::StateLimits {
            max_nodes: 10_000,
            max_depth: 256,
        },
    )
    .unwrap();
    input.bytes = cells;
    input
        .operands
        .insert(length_expression, input.length.clone());
    let summary = input.summary_projection(domain).unwrap();
    let replay = summary.instantiate(domain).unwrap();
    let sibling = summary.instantiate(domain).unwrap();
    assert!(replay.state_root().nodes() < domain.spec().relations().max_nodes);
    assert!(summary.work_size() >= replay.work_size());
    assert_eq!(
        replay.byte_at(33, domain).singleton(),
        Some(U256::from(0xaa))
    );
    assert_eq!(
        replay.byte_at(65, domain).singleton(),
        Some(U256::from(0xbb))
    );
    assert_eq!(replay.byte_at(129, domain).singleton(), Some(U256::ZERO));
    let policy = RelationLimits {
        max_nodes: 100_000,
        max_depth: 256,
        rlimit: 1_000_000,
        ..RelationLimits::default()
    };
    let equal = |data: &ByteArray, index, value| {
        ExprId::operation(
            opcode::EQ,
            &[
                data.read_byte_checked(&constant(index), domain)
                    .unwrap()
                    .expression()
                    .unwrap()
                    .clone(),
                ExprId::constant(U256::from(value)),
            ],
            policy.expressions(),
        )
        .unwrap()
    };
    for forbidden in [0, 6, 8, 10] {
        assert_eq!(
            RelationState::default().assume(&equal(&replay, 0, forbidden), true, &policy),
            CheckResult::Unsat
        );
    }
    let mut independent = RelationState::default();
    assert_eq!(
        independent.assume(&equal(&replay, 0, 7), true, &policy),
        CheckResult::Sat
    );
    assert_eq!(
        independent.assume(&equal(&replay, 1, 9), true, &policy),
        CheckResult::Sat
    );
    assert_eq!(
        independent.assume(&equal(&sibling, 0, 9), true, &policy),
        CheckResult::Sat
    );
    assert_ne!(replay.state_root(), sibling.state_root());
}

#[test]
fn numerical_projection_preserves_failed_construction_and_cancellation() {
    let mut failed = ByteArray::empty();
    failed.failure = Some(RangeError::UnknownSize);
    assert_eq!(
        failed.summary_projection(Domain::default()),
        Err(RangeError::UnknownSize)
    );
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            ByteArray::exact(&[1]).summary_projection(Domain::default()),
            Err(RangeError::Expression(ExprError::Cancelled))
        );
    });
}
