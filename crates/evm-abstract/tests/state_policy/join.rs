use super::{
    oracle,
    support::{ROOT, address, branches, complete, config, entry, run, starts_at, word, world},
};
use alloy_primitives::U256;
use evm_abstract::{
    Fork,
    analysis::{
        Acceleration, BoundedJoin, Config, ContextInsensitive, CorrelatedJoin, EnginePolicies,
        FeedbackAcceleration, FrontierReason, Limit, MergeResumeStatus, NoAcceleration,
        OutcomeKind, PolicyContext, ProjectedSummaries, SemanticPolicies, Status,
        StructuralPartition, WholeStateMetadata,
    },
    domain::{AbstractValue, Domain, NumericValue, ValueAlgebra},
    resource::WorkBudget,
    world::{Account, ByteArray, World},
};
use revm_bytecode::opcode;

#[test]
fn changing_whole_state_join_changes_real_cross_memory_precision() {
    let fixture = branches(true);
    let world = world(&fixture);
    let entry = entry(ByteArray::unknown());
    let correlated = run(
        &world,
        &entry,
        config(),
        &EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(
            SemanticPolicies::new(
                StructuralPartition,
                CorrelatedJoin,
                NoAcceleration,
                ProjectedSummaries,
            ),
        ),
    );
    let bounded = run(
        &world,
        &entry,
        config(),
        &EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(
            SemanticPolicies::new(
                StructuralPartition,
                BoundedJoin,
                NoAcceleration,
                ProjectedSummaries,
            ),
        ),
    );
    let witnesses = oracle::witnesses(&world, &[0, 1, 2], |_, witness| {
        assert_eq!(
            witness.output_word(32),
            U256::ZERO,
            "stack value equals its memory copy"
        );
    });
    for graph in [&correlated, &bounded] {
        complete(graph);
        for witness in &witnesses {
            witness.covers(graph);
        }
    }
    let returns: Vec<_> = correlated
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
        .collect();
    assert!(!returns.is_empty());
    assert!(
        returns.iter().all(
            |o| o.data.read_word(&word(32), Domain::default()).singleton() == Some(U256::ZERO)
        ),
        "whole-input choices must preserve the stack/memory equality"
    );
    assert!(
        bounded
            .outcomes()
            .iter()
            .any(|o| o.kind == OutcomeKind::Return
                && o.data
                    .read_word(&word(32), Domain::default())
                    .contains(U256::from(11 ^ 22))),
        "bounded admission must actually replace the correlated representation"
    );
}

fn counter_world() -> World {
    let mut world = World::new(Fork::Osaka, "state-policy counter feedback");
    world
        .insert(
            address(ROOT),
            Account::from_hex("5f5b600101600156", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world
}

#[test]
fn feedback_widens_a_real_counter_while_join_only_preserves_an_unfinished_obligation() {
    let world = counter_world();
    let entry = entry(ByteArray::empty());
    let mut config = config();
    config.analysis = Config {
        context_depth: 0,
        max_transfers: 12,
        ..Config::default()
    };
    let accelerated = run(
        &world,
        &entry,
        config.clone(),
        &EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(
            SemanticPolicies::new(
                StructuralPartition,
                CorrelatedJoin,
                FeedbackAcceleration,
                ProjectedSummaries,
            ),
        ),
    );
    let exact = run(
        &world,
        &entry,
        config,
        &EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(
            SemanticPolicies::new(
                StructuralPartition,
                CorrelatedJoin,
                NoAcceleration,
                ProjectedSummaries,
            ),
        ),
    );
    complete(&accelerated);
    let header = accelerated
        .states()
        .iter()
        .find(|s| starts_at(s, 1))
        .unwrap();
    let value = &header.entry.active().stack[0];
    let successor = Domain::default().apply(opcode::ADD, &[word(1), value.clone()]);
    let numeric = |v: &evm_abstract::domain::AbstractValue| {
        evm_abstract::domain::AbstractValue::from_numeric(v.numeric().clone())
    };
    assert_eq!(
        Domain::default()
            .join(&numeric(value), &numeric(&successor))
            .numeric(),
        value.numeric(),
        "Converged must include the actual next backedge input"
    );
    for n in [U256::ZERO, U256::ONE, U256::from(255), U256::MAX] {
        assert!(value.contains(n));
        assert!(successor.contains(n.wrapping_add(U256::ONE)));
    }
    assert_eq!(exact.status(), Status::Incomplete);
    assert!(
        exact
            .frontiers()
            .iter()
            .any(|f| matches!(f.reason, FrontierReason::Budget(Limit::Transfers)))
    );
    let pending = exact.states().iter().find(|s| starts_at(s, 1)).unwrap();
    assert!(pending.entry.active().stack[0].contains(U256::ZERO));
    assert!(pending.entry.active().stack[0].contains(U256::ONE));
    assert!(
        !pending.entry.active().stack[0].contains(U256::MAX),
        "a join-only schedule must not secretly apply the default widening"
    );
}

#[test]
fn public_admission_helpers_separate_covering_join_from_widening() {
    let world = counter_world();
    let entry = entry(ByteArray::empty());
    let graph = run(
        &world,
        &entry,
        config(),
        &EnginePolicies::new(ContextInsensitive, MergeResumeStatus).with_state(
            SemanticPolicies::new(
                StructuralPartition,
                CorrelatedJoin,
                FeedbackAcceleration,
                ProjectedSummaries,
            ),
        ),
    );
    complete(&graph);
    let mut first = graph
        .states()
        .iter()
        .find(|s| starts_at(s, 1))
        .unwrap()
        .entry
        .clone();
    // Intervals avoid the numeric algebra's deliberately retained small finite
    // sets: permission to widen is not a promise to discard two constants.
    first.call_stack.active_mut().stack = vec![AbstractValue::from_numeric(
        NumericValue::unsigned_range(U256::ZERO, U256::from(4)).unwrap(),
    )];
    first.relations.forget();
    let mut next = first.clone();
    next.call_stack.active_mut().stack = vec![AbstractValue::from_numeric(
        NumericValue::unsigned_range(U256::ZERO, U256::from(5)).unwrap(),
    )];
    let algebra = ValueAlgebra::new(Domain::default());
    let mut budget = WorkBudget::new(20_000_000);
    let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
    let mut exact_metadata = WholeStateMetadata::initialize(&first, &mut cx).unwrap();
    let mut wide_metadata = WholeStateMetadata::initialize(&first, &mut cx).unwrap();
    let exact = exact_metadata
        .admit_bounded(&first, &next, Acceleration::Join, &mut cx)
        .unwrap()
        .unwrap();
    let widened = wide_metadata
        .admit_bounded(&first, &next, Acceleration::Widen, &mut cx)
        .unwrap()
        .unwrap();
    for payload in [&exact, &widened] {
        assert!(payload.active().stack[0].contains(U256::ZERO));
        assert!(payload.active().stack[0].contains(U256::ONE));
        assert!(payload.active().stack[0].contains(U256::from(5)));
    }
    assert!(exact_metadata.has_bound() && wide_metadata.has_bound());
    // The signed lower bound remains nonnegative; an expanding positive value
    // witnesses widening without demanding that a lawful widening become Top.
    let later = U256::ONE << 128usize;
    assert!(!exact.active().stack[0].contains(later));
    assert!(widened.active().stack[0].contains(later));
}
