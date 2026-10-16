use super::{memory_condition, sequence_condition};
use crate::domain::{AbstractValue, Domain};
use alloy_primitives::U256;

fn value(value: U256) -> AbstractValue {
    AbstractValue::constant(value)
}

#[test]
fn sequence_guard_agrees_with_checked_math_including_empty_and_overflow() {
    let cases = [
        U256::ZERO,
        U256::from(1),
        U256::from(31),
        U256::from(32),
        U256::MAX - U256::from(1),
        U256::MAX,
    ];
    for offset in cases {
        for size in cases {
            for length in cases {
                let expected = offset.checked_add(size).is_some_and(|end| end <= length);
                let actual = sequence_condition(
                    &value(offset),
                    &value(size),
                    &value(length),
                    Domain::default(),
                );
                assert_eq!(actual.singleton(), Some(U256::from(u8::from(expected))));
            }
        }
    }
}

#[test]
fn memory_guard_distinguishes_zero_size_and_word_rounded_capacity() {
    let domain = Domain::default();
    for (offset, size, cap, expected) in [
        (U256::MAX, 0, 0, true),
        (U256::ZERO, 1, 31, false),
        (U256::ZERO, 1, 32, true),
        (U256::from(31), 1, 32, true),
        (U256::from(31), 2, 63, false),
        (U256::from(31), 2, 64, true),
        (U256::MAX, 1, usize::MAX, false),
    ] {
        let actual = memory_condition(&value(offset), &value(U256::from(size)), cap, domain);
        assert_eq!(actual.singleton(), Some(U256::from(u8::from(expected))));
    }
}

fn execution() -> super::Execution {
    use crate::{
        Fork,
        world::{Account, Entry, World},
    };
    use alloy_primitives::Address;
    let address = Address::from([0x11; 20]);
    let mut world = World::new(Fork::Osaka, "range partition unit test");
    world.insert(address, Account::empty()).unwrap();
    super::Execution {
        payload: super::super::initial(&world, &Entry::new(address), Domain::default()).unwrap(),
        executed_pcs: Vec::new(),
        instruction_progress: Vec::new(),
        instruction_values: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        completed_calls: Vec::new(),
    }
}

#[test]
fn symbolic_policy_partition_retains_both_domains_and_pre_effect_state() {
    use crate::{
        analysis::ExecutionConfig,
        domain::{relational::CheckResult, symbolic::ExprId},
    };
    let mut result = execution();
    let before = result.payload.active().memory.clone();
    let config = ExecutionConfig {
        max_memory_bytes: 64,
        ..ExecutionConfig::default()
    };
    let mut budget = super::super::WorkBudget::new(10_000_000);
    let mut context = super::TransferContext {
        config: &config,
        domain: Domain::default(),
        budget: &mut budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    };
    let source = ExprId::fresh().unwrap();
    let size = AbstractValue::top().with_expression(source.clone());
    assert!(super::memory(
        &mut result,
        &value(U256::ZERO),
        &size,
        &mut context,
        7
    ));
    assert_eq!(result.frontiers.len(), 1);
    let pending = result.frontiers[0].range.as_ref().unwrap();
    assert_eq!(pending.nonzero, Some(false));
    assert_eq!(pending.payload.active().memory, before);
    assert_eq!(result.payload.active().memory, before);
    for (length, legal) in [
        (0, true),
        (32, true),
        (64, true),
        (65, false),
        (4096, false),
    ] {
        let word = U256::from(length);
        assert_eq!(
            super::memory_condition(
                &value(U256::ZERO),
                &value(word),
                config.max_memory_bytes,
                Domain::default(),
            )
            .singleton(),
            Some(U256::from(u8::from(legal))),
            "known sizes retain the exact policy decision"
        );
        // The retained OR/AND guard keeps both partitions. A candidate is not
        // imported into F to search its compound Boolean alternatives.
        assert_eq!(
            result
                .payload
                .relations
                .can_equal(&source, word, &config.analysis.relations),
            CheckResult::MayBeFeasible
        );
        assert_eq!(
            pending
                .payload
                .relations
                .can_equal(&source, word, &config.analysis.relations),
            CheckResult::MayBeFeasible
        );
    }
}

#[test]
fn assumption_capacity_failure_preserves_the_whole_unmodified_input() {
    use crate::{
        analysis::ExecutionConfig,
        domain::{relational::CheckResult, symbolic::ExprId},
    };
    let mut result = execution();
    let mut config = ExecutionConfig {
        max_memory_bytes: 64,
        ..ExecutionConfig::default()
    };
    config.analysis.relations.max_constraints = 1;
    let other = ExprId::fresh().unwrap();
    assert_eq!(
        result
            .payload
            .relations
            .assume(&other, true, &config.analysis.relations),
        CheckResult::MayBeFeasible
    );
    let before = result.payload.clone();
    let source = ExprId::fresh().unwrap();
    let size = AbstractValue::top().with_expression(source);
    let mut budget = super::super::WorkBudget::new(10_000_000);
    let mut context = super::TransferContext {
        config: &config,
        domain: Domain::default(),
        budget: &mut budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    };
    assert!(!super::memory(
        &mut result,
        &value(U256::ZERO),
        &size,
        &mut context,
        7
    ));
    assert_eq!(result.payload, before);
    assert_eq!(result.frontiers.len(), 1);
    let pending = result.frontiers[0].range.as_ref().unwrap();
    assert_eq!(pending.nonzero, None);
    assert_eq!(*pending.payload, before);
    assert_eq!(
        result.frontiers[0].reason,
        crate::analysis::FrontierReason::Relations(
            crate::domain::relational::QueryReason::ConstraintLimit
        )
    );
}

#[test]
fn encoder_limit_keeps_both_guarded_memory_domains() {
    use crate::{
        analysis::{ExecutionConfig, FrontierReason},
        domain::{
            relational::{Constraint, QueryReason},
            symbolic::{ByteStateId, ByteView, ExprId, SourceId},
        },
    };
    let mut result = execution();
    let before = result.payload.active().memory.clone();
    let mut config = ExecutionConfig {
        max_memory_bytes: 64,
        ..ExecutionConfig::default()
    };
    config.analysis.relations.max_nodes = 64;
    let length = ExprId::constant(U256::from(64));
    let hash = ExprId::keccak(
        ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), length.clone()),
            ExprId::constant(U256::ZERO),
            length,
        ),
        config.analysis.relations.expressions(),
    )
    .unwrap();
    let size = AbstractValue::top().with_expression(hash);
    let condition = super::memory_condition(&value(U256::ZERO), &size, 64, Domain::default());
    let expression = condition.symbolic_expression().unwrap();
    assert!(expression.nodes() <= config.analysis.relations.max_nodes);
    let mut budget = super::super::WorkBudget::new(10_000_000);
    let mut context = super::TransferContext {
        config: &config,
        domain: Domain::default(),
        budget: &mut budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    };
    assert!(super::memory(
        &mut result,
        &value(U256::ZERO),
        &size,
        &mut context,
        7
    ));
    assert_eq!(result.payload.active().memory, before);
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].reason, FrontierReason::Memory);
    let excluded = result.frontiers[0].range.as_ref().unwrap();
    assert_eq!(excluded.nonzero, Some(false));
    assert_eq!(excluded.payload.active().memory, before);
    for (payload, nonzero) in [(&result.payload, true), (excluded.payload.as_ref(), false)] {
        assert!(payload.relations.constraints().any(|atom| atom
            == &Constraint::Truth {
                expression: expression.clone(),
                nonzero,
            }));
    }
    assert_eq!(
        result
            .diagnostics
            .iter()
            .filter(|(_, kind)| *kind
                == crate::analysis::DiagnosticKind::RelationalPrecisionLost(
                    QueryReason::ExpressionLimit
                ))
            .count(),
        2
    );
}
