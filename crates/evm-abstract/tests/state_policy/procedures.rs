use super::{
    oracle,
    support::{
        Assembly, BALANCE, Fixture, ROOT, UNKNOWN, address, alternate_run, callback_implementation,
        callback_parent, complete, config, default_run, entry, insert, recursive_counter,
        recursive_returning, recursive_rollback, run, word, world,
    },
};
use alloy_primitives::U256;
use evm_abstract::{
    analysis::{
        Acceleration, CorrelatedJoin, EnginePolicies, ExecutionConfig, FeedbackAcceleration,
        FirstResumeStatus, MachineError, OutcomeKind, PolicyContext, PolicySite, ProcedureOutput,
        ProcedureSummaryKind, ProcedureSummaryPolicy, ProjectedSummaries, RecentDistinctJumps,
        RelativeState, SemanticPolicies, StructuralPartition,
    },
    domain::{Domain, NumericValue, ValueAlgebra},
    resource::WorkBudget,
    world::{Account, ByteArray, Existence, World},
};
use revm_bytecode::opcode;

pub(super) fn recursive_config() -> ExecutionConfig {
    ExecutionConfig {
        max_call_depth: 3,
        max_work: 100_000_000,
        ..config()
    }
}

#[test]
fn both_complete_strategies_cover_recursive_body_output_and_feedback_growth() {
    let fixture = recursive_counter(true);
    let world = world(&fixture);
    let entry = entry(ByteArray::unknown());
    let witnesses = oracle::witnesses(&world, &[0, 1, 2, 6, 17], |n, witness| {
        assert_eq!(witness.kind, OutcomeKind::Return);
        assert_eq!(witness.output_word(0), U256::from(n));
        assert_eq!(witness.output_word(32), U256::from(n));
        assert_eq!(witness.output_word(64), U256::from(77));
        assert_eq!(witness.storage[&(address(ROOT), U256::ZERO)], U256::from(n));
        if n >= 6 {
            assert!(
                witness
                    .steps
                    .iter()
                    .filter(|(code, pc)| {
                        *code == address(ROOT) && *pc == fixture.labels["recursive_call"]
                    })
                    .count()
                    >= 6,
                "the witness must exceed the unfolding bound"
            );
        }
    });
    for use_summaries in [false, true] {
        let config = ExecutionConfig {
            use_summaries,
            ..recursive_config()
        };
        let graphs = [
            default_run(&world, &entry, config.clone()),
            alternate_run(&world, &entry, config),
        ];
        for graph in &graphs {
            complete(graph);
            assert!(!graph.procedure_summaries().is_empty());
            assert!(
                graph
                    .procedure_summaries()
                    .iter()
                    .all(|r| r.kind == ProcedureSummaryKind::KnownRecursion)
            );
            let procedures: Vec<_> = graph
                .procedure_summaries()
                .iter()
                .flat_map(|r| &r.procedures)
                .collect();
            assert!(
                procedures.iter().any(|p| {
                    let counter = p.input.read(address(ROOT), &word(0), Domain::default());
                    counter.contains(U256::from(6))
                        && counter.contains(U256::from(17))
                        && !p.covered_blocks.is_empty()
                }),
                "recursive feedback must enlarge an actually executed input invariant"
            );
            assert!(
                procedures.iter().flat_map(|p| &p.outputs).any(|o| {
                    o.kind == OutcomeKind::Return
                        && o.data
                            .read_word(&word(0), Domain::default())
                            .contains(U256::from(17))
                        && o.store
                            .read(address(ROOT), &word(0), Domain::default())
                            .contains(U256::from(17))
                }),
                "output growth must reach retained recursive callers"
            );
            for witness in &witnesses {
                witness.covers(graph);
            }
        }
    }
}

#[test]
fn recursive_revert_restores_parent_persistent_and_transient_memory_witnesses() {
    let fixture = recursive_rollback();
    let world = world(&fixture);
    let entry = entry(ByteArray::unknown());
    // The initial caller and recursive body have distinct CALL sites. The
    // repeated structural edge is established at depth four in this fixture.
    let config = ExecutionConfig {
        max_call_depth: 4,
        ..recursive_config()
    };
    let graphs = [
        default_run(&world, &entry, config.clone()),
        alternate_run(&world, &entry, config),
    ];

    let witnesses = oracle::witnesses(&world, &[0, 1, 4, 6], |_, witness| {
        assert_eq!(witness.kind, OutcomeKind::Return);
        assert_eq!(witness.output_word(0), U256::from(5));
        assert_eq!(witness.output_word(32), U256::from(7));
        assert_eq!(witness.storage[&(address(ROOT), U256::ZERO)], U256::from(5));
    });
    for graph in &graphs {
        complete(graph);
        assert!(
            graph
                .procedure_summaries()
                .iter()
                .any(|r| r.kind == ProcedureSummaryKind::KnownRecursion)
        );
        for witness in &witnesses {
            witness.covers(graph);
        }
    }
    let returns: Vec<_> = graphs[0]
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
        .collect();
    assert!(!returns.is_empty());
    for outcome in returns {
        assert_eq!(
            outcome
                .store
                .read(address(ROOT), &word(0), Domain::default())
                .singleton(),
            Some(U256::from(5))
        );
        assert_eq!(
            outcome
                .data
                .read_word(&word(32), Domain::default())
                .singleton(),
            Some(U256::from(7))
        );
    }
}

fn unknown_world(fixture: &Fixture) -> World {
    let mut world = world(fixture);
    let mut unknown = Account::unknown();
    unknown.existence = Existence::Present;
    unknown.balance = word(BALANCE);
    unknown.nonce = word(0);
    world.insert(address(UNKNOWN), unknown).unwrap();
    world
}

fn concrete_unknown(world: &World, implementation: &Fixture) -> World {
    let mut concrete = World::new(world.fork(), "concrete unknown-code witness");
    for (owner, account) in world.accounts() {
        if *owner != address(UNKNOWN) {
            concrete.insert(*owner, account.clone()).unwrap();
        }
    }
    insert(&mut concrete, UNKNOWN, implementation, &[]);
    concrete
}

#[test]
fn both_strategies_cover_unknown_callbacks_and_each_rollback_scope() {
    for outer_revert in [false, true] {
        let fixture = callback_parent(outer_revert);
        let world = unknown_world(&fixture);
        let entry = entry(ByteArray::empty());
        let graphs = [
            default_run(&world, &entry, recursive_config()),
            alternate_run(&world, &entry, recursive_config()),
        ];
        let witnesses: Vec<_> = [
            (0, false, 1, 77, 0),  // callback reverts; U handles its failure
            (1, false, 1, 11, 11), // callback and U commit
            (1, true, 0, 77, 0),   // U rolls back its successful callback
        ]
        .into_iter()
        .map(
            |(selector, unknown_reverts, expected_status, expected_slot, expected_transient)| {
                let concrete =
                    concrete_unknown(&world, &callback_implementation(selector, unknown_reverts));
                let witness = oracle::execute(&concrete, &entry);
                assert_eq!(
                    witness.kind,
                    if outer_revert {
                        OutcomeKind::Revert
                    } else {
                        OutcomeKind::Return
                    }
                );
                assert_eq!(witness.output_word(0), U256::from(expected_status));
                assert_eq!(witness.output_word(32), U256::from(7));
                assert_eq!(witness.output_word(64), U256::from(expected_slot));
                assert_eq!(witness.output_word(96), U256::from(expected_transient));
                let callback_pc = fixture.labels[if selector == 0 {
                    "callback_fail"
                } else {
                    "callback_success"
                }];
                assert!(witness.steps.contains(&(address(ROOT), callback_pc)));
                assert_eq!(
                    witness.storage[&(address(ROOT), U256::ZERO)],
                    U256::from(if outer_revert { 0 } else { 7 })
                );
                assert_eq!(
                    witness.storage[&(address(ROOT), U256::ONE)],
                    U256::from(if outer_revert { 77 } else { expected_slot })
                );
                witness
            },
        )
        .collect();
        for graph in &graphs {
            complete(graph);
            assert!(
                graph.procedure_summaries().iter().any(|record| {
                    record.kind == ProcedureSummaryKind::UnknownEnvironment
                        && record.procedures.iter().any(|p| {
                            p.frame.code_address == address(ROOT) && !p.covered_blocks.is_empty()
                        })
                }),
                "unknown execution must analyze the actual known callback bytecode"
            );
            for witness in &witnesses {
                witness.covers(graph);
            }
        }
    }
}

/// Non-Clone policy borrowing an application option; all coarsening is paid by
/// the public helper and affects even the first published procedure output.
struct BorrowedOutputs<'a> {
    coarse: &'a bool,
}

impl ProcedureSummaryPolicy<Domain> for BorrowedOutputs<'_> {
    fn admit_relative(
        &self,
        current: &RelativeState<NumericValue>,
        incoming: &RelativeState<NumericValue>,
        site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, Domain>,
    ) -> Result<RelativeState<NumericValue>, MachineError> {
        ProjectedSummaries.admit_relative(current, incoming, site, acceleration, cx)
    }

    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<NumericValue>>,
        incoming: &ProcedureOutput<NumericValue>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, Domain>,
    ) -> Result<ProcedureOutput<NumericValue>, MachineError> {
        let output = ProjectedSummaries.admit_output(current, incoming, acceleration, cx)?;
        if *self.coarse {
            output.coarsen(cx)
        } else {
            Ok(output)
        }
    }
}

#[test]
fn borrowed_summary_policy_changes_published_return_bytes_and_still_covers_revm() {
    let fixture = recursive_returning();
    let world = world(&fixture);
    let entry = entry(ByteArray::unknown());
    let precise = default_run(&world, &entry, recursive_config());
    let coarse = true;
    let policies = EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus).with_state(
        SemanticPolicies::new(
            StructuralPartition,
            CorrelatedJoin,
            FeedbackAcceleration,
            BorrowedOutputs { coarse: &coarse },
        ),
    );
    let changed = run(&world, &entry, recursive_config(), &policies);
    let witnesses = oracle::witnesses(&world, &[0, 1, 4, 9], |_, witness| {
        assert_eq!(witness.output_word(0), U256::from(42));
    });
    for graph in [&precise, &changed] {
        complete(graph);
        assert!(!graph.procedure_summaries().is_empty());
        for witness in &witnesses {
            witness.covers(graph);
        }
    }
    let precise_outputs: Vec<_> = precise
        .procedure_summaries()
        .iter()
        .flat_map(|r| &r.procedures)
        .flat_map(|p| &p.outputs)
        .filter(|o| o.kind == OutcomeKind::Return)
        .collect();
    assert!(!precise_outputs.is_empty());
    assert!(precise_outputs.iter().all(
        |o| o.data.read_word(&word(0), Domain::default()).singleton() == Some(U256::from(42))
    ));
    assert!(
        changed
            .procedure_summaries()
            .iter()
            .flat_map(|r| &r.procedures)
            .flat_map(|p| &p.outputs)
            .any(|o| o.kind == OutcomeKind::Return
                && o.data
                    .read_word(&word(0), Domain::default())
                    .contains(U256::from(43))),
        "the selected output policy must change retained content, including first output"
    );
    assert!(
        changed
            .outcomes()
            .iter()
            .any(|o| o.kind == OutcomeKind::Return
                && o.data
                    .read_word(&word(0), Domain::default())
                    .contains(U256::from(43))),
        "coarser outputs must propagate to actual outer callers"
    );
}

#[test]
fn public_relative_coarsening_removes_a_real_stack_alias() {
    let mut a = Assembly::default();
    a.input().op(opcode::DUP1).op(opcode::STOP);
    let world = world(&a.finish());
    let entry = entry(ByteArray::unknown());
    let graph = default_run(&world, &entry, config());
    complete(&graph);
    let source = graph
        .states()
        .iter()
        .find_map(|state| state.exit.as_ref())
        .unwrap();
    assert_eq!(source.active().stack.len(), 2);
    let algebra = ValueAlgebra::new(Domain::default());
    let mut budget = WorkBudget::new(20_000_000);
    let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
    let projected = RelativeState::project(source, &mut cx).unwrap();
    let coarse = projected.coarsen(&mut cx).unwrap();
    assert!(!projected.semantic_eq(&coarse, &mut cx).unwrap());
    let precise_instance = projected.instantiate(&mut cx).unwrap();
    let coarse_instance = coarse.instantiate(&mut cx).unwrap();
    let compare = |state: &evm_abstract::analysis::MachinePayload| {
        Domain::default().apply(opcode::XOR, &state.active().stack)
    };
    assert_eq!(compare(&precise_instance).singleton(), Some(U256::ZERO));
    assert!(compare(&coarse_instance).contains(U256::ONE));
    for instance in [&precise_instance, &coarse_instance] {
        for value in &instance.active().stack {
            for concrete in [U256::ZERO, U256::ONE, U256::MAX] {
                assert!(value.contains(concrete));
            }
        }
    }
}
