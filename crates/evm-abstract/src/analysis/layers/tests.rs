use super::*;
use crate::{
    Address, Fork, U256,
    analysis::{ExecutionConfig, FrontierReason, OutcomeKind, Status},
    bytecode::Program,
    domain::{AbstractValue, Domain},
    world::{Code, Entry, Existence},
};
use alloy_primitives::B256;
use std::sync::Arc;

fn address(value: u16) -> Address {
    let mut bytes = [0u8; 20];
    bytes[18..].copy_from_slice(&value.to_be_bytes());
    Address::from(bytes)
}

fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn catalog(code: &str) -> (Arc<CodeCatalog>, CodeId) {
    let program = Program::from_hex_with_fork(code, Fork::Osaka).unwrap();
    let (catalog, id) = CodeCatalog::new(Fork::Osaka).with_runtime(program).unwrap();
    (Arc::new(catalog), id)
}

fn session(code: &str) -> AnalysisSession {
    let (catalog, id) = catalog(code);
    AnalysisSession::new(
        catalog,
        StateFacts::new(SnapshotId::offline("layer tests")),
        id,
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap()
}

fn returns(report: &SessionAnalysis) -> Vec<AbstractValue> {
    let values: Vec<_> = report
        .analysis()
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .map(|outcome| outcome.data.read_word(&word(0), Domain::default()))
        .collect();
    assert!(!values.is_empty(), "{:?}", report.analysis().frontiers());
    values
}

#[test]
fn catalog_is_immutable_and_compiles_stack_and_initial_input_requirements() {
    let empty = CodeCatalog::new(Fork::Osaka);
    let program = Program::from_hex_with_fork("60026003016008565b5f5400", Fork::Osaka).unwrap();
    let (catalog, id) = empty.with_runtime(program.clone()).unwrap();
    assert!(empty.get(id).is_none());
    let compiled = catalog.get(id).unwrap();
    assert!(compiled.blocks()[0].stack_only);
    assert!(!compiled.blocks()[1].stack_only);
    assert_eq!(compiled.blocks()[0].required_stack, 0);
    assert_eq!(compiled.blocks()[0].produced_stack, 1);
    assert!(compiled.requirements().contains(&FactKind::Storage));
    let (again, same) = catalog.with_runtime(program.clone()).unwrap();
    assert_eq!(id, same);
    assert!(Arc::ptr_eq(compiled, again.get(same).unwrap()));
    let (other_mode, init) = catalog.with_program(program, CodeMode::Initcode).unwrap();
    assert_ne!(id, init);
    assert_eq!(id.hash(), init.hash());
    assert!(other_mode.get(init).is_some());
    let old_fork = Program::from_hex_with_fork("00", Fork::Cancun).unwrap();
    assert!(matches!(
        catalog.with_runtime(old_fork),
        Err(CatalogError::MixedFork { .. })
    ));
}

#[test]
fn code_only_has_unknown_account_slot_balance_input_and_external_code() {
    for code in [
        "5f545f5260205ff3",
        "475f5260205ff3",
        "365f5260205ff3",
        "6102223b5f5260205ff3",
    ] {
        let mut session = session(code);
        let report = session.analyze().unwrap();
        assert_eq!(
            report.analysis().status(),
            Status::Converged,
            "{:?}",
            report.analysis().frontiers()
        );
        assert!(
            returns(&report)
                .iter()
                .all(|value| value.singleton().is_none())
        );
        let account = report.analysis().world().account(address(0x101)).unwrap();
        assert_eq!(account.existence, Existence::Unknown);
        assert!(account.balance.singleton().is_none());
        assert!(account.nonce.singleton().is_none());
        assert!(account.storage_unknown);
        assert!(report.analysis().world().account(address(0x222)).is_none());
        assert!(
            report
                .analysis()
                .entry()
                .environment
                .calldata
                .len()
                .singleton()
                .is_none()
        );
        assert_eq!(
            session.facts().balance(address(0x101)),
            Observed::Unobserved
        );
    }
}

#[test]
fn immutable_snapshot_identity_rejects_same_label_other_input_and_other_chain_block() {
    let offline = SnapshotId::offline("same label");
    let other = SnapshotId::offline("same label");
    assert_ne!(offline, other);
    assert_eq!(offline, offline.clone());
    let facts = StateFacts::new(offline.clone());
    let revision = facts.revision();
    assert!(matches!(
        facts.refine(&other, []),
        Err(FactError::Snapshot { .. })
    ));
    assert_eq!(facts.revision(), revision);
    let chain = SnapshotId::chain(U256::MAX, B256::repeat_byte(7));
    assert_eq!(chain, SnapshotId::chain(U256::MAX, B256::repeat_byte(7)));
    for wrong in [
        SnapshotId::chain(U256::from(1), B256::repeat_byte(7)),
        SnapshotId::chain(U256::MAX, B256::repeat_byte(8)),
    ] {
        assert!(matches!(
            StateFacts::new(chain.clone()).refine(&wrong, []),
            Err(FactError::Snapshot { .. })
        ));
    }
}

#[test]
fn missing_zero_empty_and_absent_are_distinct_and_conflicts_are_atomic() {
    let snapshot = SnapshotId::offline("observations");
    let original = StateFacts::new(snapshot.clone());
    let owner = address(0x202);
    let zero = Observation::Storage {
        address: owner,
        slot: U256::ZERO,
        value: U256::ZERO,
    };
    let known = original
        .refine(&snapshot, [zero.clone()])
        .unwrap()
        .into_facts();
    assert_eq!(original.storage(owner, U256::ZERO), Observed::Unobserved);
    assert_eq!(
        known.storage(owner, U256::ZERO),
        Observed::Known(U256::ZERO)
    );
    assert_eq!(known.storage(owner, U256::from(1)), Observed::Unobserved);
    assert_eq!(known.existence(owner), Observed::Unobserved);
    assert_eq!(
        known.refine(&snapshot, [zero]).unwrap().facts().revision(),
        known.revision()
    );
    let absent = known
        .refine(
            &snapshot,
            [Observation::Existence {
                address: owner,
                exists: false,
            }],
        )
        .unwrap()
        .into_facts();
    assert_eq!(absent.code(owner), Observed::Known(CodeObservation::Empty));
    assert_eq!(absent.balance(owner), Observed::Known(U256::ZERO));
    assert_eq!(absent.nonce(owner), Observed::Known(U256::ZERO));
    assert_eq!(
        absent.storage(owner, U256::from(999)),
        Observed::Known(U256::ZERO)
    );
    let present_empty = known
        .refine(
            &snapshot,
            [Observation::Code {
                address: owner,
                code: CodeObservation::Empty,
            }],
        )
        .unwrap()
        .into_facts();
    assert_eq!(present_empty.existence(owner), Observed::Unobserved);
    assert_eq!(present_empty.balance(owner), Observed::Unobserved);
    let (_, runtime) = catalog("00");
    for contradiction in [
        Observation::Balance {
            address: owner,
            value: U256::from(1),
        },
        Observation::Nonce {
            address: owner,
            value: U256::from(1),
        },
        Observation::Storage {
            address: owner,
            slot: U256::from(1),
            value: U256::from(1),
        },
        Observation::Code {
            address: owner,
            code: CodeObservation::Runtime(runtime),
        },
        Observation::Code {
            address: owner,
            code: CodeObservation::Delegation(address(0x303)),
        },
        Observation::Existence {
            address: owner,
            exists: true,
        },
    ] {
        assert!(matches!(
            absent.refine(&snapshot, [contradiction.clone()]),
            Err(FactError::Conflict { .. })
        ));
        let positive = original
            .refine(&snapshot, [contradiction])
            .unwrap()
            .into_facts();
        assert!(matches!(
            positive.refine(
                &snapshot,
                [Observation::Existence {
                    address: owner,
                    exists: false
                }]
            ),
            Err(FactError::Conflict { .. })
        ));
    }
    let revision = known.revision();
    assert!(
        known
            .refine(
                &snapshot,
                [
                    Observation::Balance {
                        address: owner,
                        value: U256::from(8)
                    },
                    Observation::Storage {
                        address: owner,
                        slot: U256::ZERO,
                        value: U256::from(1)
                    }
                ]
            )
            .is_err()
    );
    assert_eq!(known.balance(owner), Observed::Unobserved);
    assert_eq!(known.revision(), revision);
}

#[test]
fn complete_storage_keeps_explicit_values_and_rejects_later_nonzero_default() {
    let snapshot = SnapshotId::offline("complete map");
    let owner = address(0x202);
    let facts = StateFacts::new(snapshot.clone())
        .refine(
            &snapshot,
            [
                Observation::StorageComplete { address: owner },
                Observation::Storage {
                    address: owner,
                    slot: U256::from(1),
                    value: U256::from(9),
                },
            ],
        )
        .unwrap()
        .into_facts();
    assert_eq!(
        facts.storage(owner, U256::from(1)),
        Observed::Known(U256::from(9))
    );
    assert_eq!(
        facts.storage(owner, U256::from(2)),
        Observed::Known(U256::ZERO)
    );
    assert!(
        facts
            .refine(
                &snapshot,
                [Observation::Storage {
                    address: owner,
                    slot: U256::from(2),
                    value: U256::from(7)
                }]
            )
            .is_err()
    );
}

#[test]
fn refinement_reuses_pure_computation_on_new_store_and_retains_old_report() {
    let mut session = session("60026003016008565b5f545f5260205ff3");
    let old = session.analyze().unwrap();
    assert!(
        returns(&old)
            .iter()
            .all(|value| value.singleton().is_none())
    );
    assert!(old.reuse().entries > 0);
    assert!(Arc::ptr_eq(&old, &session.analyze().unwrap()));
    let snapshot = session.facts().snapshot().clone();
    let effect = session
        .refine(
            &snapshot,
            [Observation::Storage {
                address: address(0x101),
                slot: U256::ZERO,
                value: U256::from(3),
            }],
        )
        .unwrap();
    assert!(
        matches!(effect, RefinementEffect::RestartRequired { ref affected_states, .. } if !affected_states.is_empty())
    );
    let new = session.analyze().unwrap();
    assert_ne!(old.revision(), new.revision());
    assert!(new.reuse().hits > old.reuse().hits);
    assert!(
        returns(&new)
            .iter()
            .all(|value| value.singleton() == Some(U256::from(3)))
    );
    assert!(
        returns(&old)
            .iter()
            .all(|value| value.singleton().is_none())
    );
    assert_eq!(
        old.analysis()
            .world()
            .account(address(0x101))
            .unwrap()
            .storage
            .get(&U256::ZERO),
        None
    );
    assert_eq!(
        new.analysis()
            .world()
            .account(address(0x101))
            .unwrap()
            .storage[&U256::ZERO]
            .singleton(),
        Some(U256::from(3))
    );
}

#[test]
fn injection_after_runtime_write_and_revert_never_patches_runtime_or_saved_roots() {
    for code in ["60095f555f545f5260205ff3", "60095f555f5ffd"] {
        let mut session = session(code);
        let old = session.analyze().unwrap();
        let snapshot = session.facts().snapshot().clone();
        session
            .refine(
                &snapshot,
                [Observation::Storage {
                    address: address(0x101),
                    slot: U256::ZERO,
                    value: U256::from(3),
                }],
            )
            .unwrap();
        let new = session.analyze().unwrap();
        for report in [&old, &new] {
            let exit = report
                .analysis()
                .states()
                .iter()
                .filter_map(|state| state.exit.as_ref())
                .next()
                .unwrap();
            assert_eq!(
                exit.store
                    .read(address(0x101), &word(0), Domain::default())
                    .singleton(),
                Some(U256::from(9))
            );
        }
        let failed = |report: &SessionAnalysis| {
            report
                .analysis()
                .outcomes()
                .iter()
                .filter(|outcome| {
                    matches!(outcome.kind, OutcomeKind::Failure | OutcomeKind::Revert)
                })
                .map(|outcome| {
                    outcome
                        .store
                        .read(address(0x101), &word(0), Domain::default())
                })
                .collect::<Vec<_>>()
        };
        assert!(failed(&old).iter().all(|value| value.singleton().is_none()));
        assert!(
            failed(&new)
                .iter()
                .all(|value| value.singleton() == Some(U256::from(3)))
        );
        if code.ends_with("f3") {
            assert!(
                returns(&old)
                    .iter()
                    .chain(returns(&new).iter())
                    .all(|value| value.singleton() == Some(U256::from(9)))
            );
        }
    }
}

#[test]
fn shared_catalog_sessions_do_not_share_facts_reports_or_memos() {
    let (catalog, id) = catalog("60026003016008565b5f545f5260205ff3");
    let mut left = AnalysisSession::new(
        Arc::clone(&catalog),
        StateFacts::new(SnapshotId::offline("left")),
        id,
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap();
    let mut right = AnalysisSession::new(
        catalog,
        StateFacts::new(SnapshotId::offline("right")),
        id,
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap();
    for (session, value) in [(&mut left, 11u64), (&mut right, 22u64)] {
        let snapshot = session.facts().snapshot().clone();
        session
            .refine(
                &snapshot,
                [Observation::Storage {
                    address: address(0x101),
                    slot: U256::ZERO,
                    value: U256::from(value),
                }],
            )
            .unwrap();
        let report = session.analyze().unwrap();
        assert!(
            returns(&report)
                .iter()
                .all(|v| v.singleton() == Some(U256::from(value)))
        );
        assert_eq!(report.reuse().hits, 0);
    }
    assert!(Arc::ptr_eq(left.catalog(), right.catalog()));
    assert_ne!(left.facts().revision(), right.facts().revision());
    assert_eq!(
        left.facts().storage(address(0x101), U256::ZERO),
        Observed::Known(U256::from(11))
    );
}

#[test]
fn dependency_index_includes_negative_reads_completeness_calls_and_return_edges() {
    use crate::analysis::{MachineEdge, MachineEdgeKind};
    let owner = address(0x202);
    let mut index = FactDependencies::default();
    index.add(
        1,
        FactDependency::Storage {
            address: Some(owner),
            slot: Some(U256::from(7)),
        },
    );
    index.add(3, FactDependency::Code(Some(owner)));
    index.finish(&[
        MachineEdge {
            from: 1,
            to: 2,
            kind: MachineEdgeKind::Call,
        },
        MachineEdge {
            from: 2,
            to: 4,
            kind: MachineEdgeKind::Revert,
        },
    ]);
    assert_eq!(
        index.affected_states(&[FactKey::StorageComplete(owner)]),
        [1, 2, 4].into_iter().collect()
    );
    assert_eq!(
        index.affected_states(&[FactKey::Existence(owner)]),
        [1, 2, 3, 4].into_iter().collect()
    );
    assert!(
        index
            .affected_states(&[FactKey::Storage {
                address: owner,
                slot: U256::from(8)
            }])
            .is_empty()
    );
    index.add(5, FactDependency::AllInitialState);
    assert!(
        index
            .affected_states(&[FactKey::Balance(address(0x999))])
            .contains(&5)
    );
    let mut session = session("5f545f5260205ff3");
    let report = session.analyze().unwrap();
    assert!(report.dependencies().reads().any(|(read, _)| *read
        == FactDependency::Storage {
            address: Some(address(0x101)),
            slot: None
        }));
    assert!(
        report
            .dependencies()
            .reads()
            .any(|(read, _)| *read == FactDependency::AllInitialState)
    );
    assert!(report.demands().is_empty()); // Ordinary code-only reads propagate unknowns.
}

#[test]
fn code_binding_and_rejected_refinement_do_not_establish_unrelated_state() {
    let mut session = session("00");
    let snapshot = session.facts().snapshot().clone();
    let (_, other) = catalog("600100");
    let before = session.facts().revision();
    assert!(
        session
            .refine(
                &snapshot,
                [Observation::Code {
                    address: address(0x101),
                    code: CodeObservation::Runtime(other),
                }]
            )
            .is_err()
    );
    assert_eq!(session.facts().revision(), before);
    let external = address(0x202);
    session
        .refine(
            &snapshot,
            [Observation::Existence {
                address: external,
                exists: false,
            }],
        )
        .unwrap();
    let report = session.analyze().unwrap();
    let account = report.analysis().world().account(external).unwrap();
    assert_eq!(account.existence, Existence::Absent);
    assert_eq!(account.code, Code::Empty);
    assert!(!account.storage_unknown);
    assert_eq!(account.balance.singleton(), Some(U256::ZERO));
    assert!(
        !report
            .analysis()
            .frontiers()
            .iter()
            .any(|f| matches!(f.reason, FrontierReason::MissingCode(_)))
    );
}

#[test]
fn ordinary_routes_share_layered_execution_and_preserve_abstract_world_bounds() {
    use crate::world::{Account, World};
    let program = Program::from_hex_with_fork("475f5260205ff3", Fork::Osaka).unwrap();
    let single = crate::analysis::analyze(program.clone(), Default::default()).unwrap();
    assert!(
        single
            .execution()
            .outcomes()
            .iter()
            .filter(|o| o.kind == OutcomeKind::Return)
            .all(|o| o
                .data
                .read_word(&word(0), Domain::default())
                .singleton()
                .is_none())
    );
    let mut world = World::offline(Fork::Osaka, "bounded base", "test");
    let mut account = Account::unknown();
    account.code = Code::Runtime(program);
    account.balance = AbstractValue::unsigned_range(U256::from(3), U256::from(9)).unwrap();
    account.storage.insert(
        U256::ZERO,
        AbstractValue::unsigned_range(U256::from(5), U256::from(8)).unwrap(),
    );
    account.storage_unknown = false;
    world.insert(address(0x101), account.clone()).unwrap();
    let public = crate::analysis::analyze_world(
        world.clone(),
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap();
    assert_eq!(public.world().account(address(0x101)), Some(&account));
    let mut session = AnalysisSession::from_world(
        world,
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap();
    let before = session.analyze().unwrap();
    assert!(returns(&before).iter().all(|v| !v.contains(U256::from(2))
        && v.contains(U256::from(3))
        && v.contains(U256::from(9))
        && !v.contains(U256::from(10))));
    let snapshot = session.facts().snapshot().clone();
    let revision = session.facts().revision();
    for observation in [
        Observation::Balance {
            address: address(0x101),
            value: U256::from(2),
        },
        Observation::Storage {
            address: address(0x101),
            slot: U256::ZERO,
            value: U256::ZERO,
        },
        Observation::Storage {
            address: address(0x101),
            slot: U256::from(1),
            value: U256::from(1),
        },
    ] {
        assert!(session.refine(&snapshot, [observation]).is_err());
        assert_eq!(session.facts().revision(), revision);
    }
    session
        .refine(
            &snapshot,
            [
                Observation::Balance {
                    address: address(0x101),
                    value: U256::from(7),
                },
                Observation::Storage {
                    address: address(0x101),
                    slot: U256::ZERO,
                    value: U256::from(6),
                },
            ],
        )
        .unwrap();
    let refined = session.analyze().unwrap();
    assert!(
        returns(&refined)
            .iter()
            .all(|v| v.singleton() == Some(U256::from(7)))
    );
    assert_eq!(session.base().account(address(0x101)), Some(&account));
    assert_eq!(
        before.analysis().world().account(address(0x101)),
        Some(&account)
    );
}

#[test]
fn completed_report_cache_observes_current_and_captured_cancellation() {
    use crate::analysis::{control::Control, progress::Observer};
    let mut current = session("00");
    let report = current.analyze().unwrap();
    let control = Control::new(Observer::default());
    control.cancellation().cancel();
    assert!(control.scope(|| current.analyze()).is_err());
    assert!(current.analyze_with_control(&control).is_err());
    assert_eq!(report.analysis().status(), Status::Converged);
    let captured = Control::new(Observer::default());
    let mut session = captured.scope(|| session("00"));
    session.analyze().unwrap();
    captured.cancellation().cancel();
    assert!(session.analyze().is_err());
}

#[test]
fn offline_demands_inject_initial_values_after_runtime_write_and_preserve_rollback() {
    let mut session = session("60095f55600154505f5ffd");
    let pending = session.analyze_with_demands().unwrap();
    assert!(pending.demand_driven());
    assert_eq!(pending.analysis().status(), Status::Incomplete);
    assert_eq!(
        pending.demands(),
        [FactDemand::Storage {
            address: address(0x101),
            slot: U256::from(1)
        }]
        .into_iter()
        .collect()
    );
    let snapshot = session.facts().snapshot().clone();
    session
        .refine(
            &snapshot,
            [Observation::Storage {
                address: address(0x101),
                slot: U256::from(1),
                value: U256::from(3),
            }],
        )
        .unwrap();
    let refined = session.analyze_with_demands().unwrap();
    assert!(refined.demands().is_empty());
    assert_eq!(
        refined.analysis().status(),
        Status::Converged,
        "{:?}",
        refined.analysis().frontiers()
    );
    let reverted: Vec<_> = refined
        .analysis()
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Revert)
        .collect();
    assert!(!reverted.is_empty());
    for outcome in reverted {
        assert!(
            outcome
                .store
                .read(address(0x101), &word(0), Domain::default())
                .singleton()
                .is_none()
        );
        assert_eq!(
            outcome
                .store
                .read(address(0x101), &word(1), Domain::default())
                .singleton(),
            Some(U256::from(3))
        );
    }
    assert_eq!(pending.demands().len(), 1);
    assert_ne!(pending.revision(), refined.revision());
}

#[test]
fn consecutive_injections_keep_the_executed_revision_and_cumulative_invalidation() {
    let mut session = session("60026003016008565b5f545f5260205ff3");
    let original = session.analyze().unwrap();
    let snapshot = session.facts().snapshot().clone();
    let first = session
        .refine(
            &snapshot,
            [Observation::Existence {
                address: address(0x101),
                exists: true,
            }],
        )
        .unwrap();
    assert!(
        matches!(first, RefinementEffect::RestartRequired { ref affected_states, .. } if affected_states.contains(&0))
    );
    let second = session
        .refine(
            &snapshot,
            [Observation::Storage {
                address: address(0x202),
                slot: U256::ZERO,
                value: U256::ZERO,
            }],
        )
        .unwrap();
    let RefinementEffect::RestartRequired {
        executed_revision,
        affected_states,
        ..
    } = second
    else {
        panic!("executed session must invalidate");
    };
    assert_eq!(executed_revision, original.revision());
    assert!(affected_states.contains(&0));
    assert_ne!(session.facts().revision(), executed_revision);
}

#[test]
fn catalog_code_binding_keeps_delegatecall_storage_owner_independent() {
    let (catalog, caller) = catalog("60205f5f5f6102026207a120f41560155760205ff35b5f5ffd");
    let callee = Program::from_hex_with_fork("5f545f5260205ff3", Fork::Osaka).unwrap();
    let (catalog, callee) = catalog.with_runtime(callee).unwrap();
    let snapshot = SnapshotId::offline("delegated owner");
    let facts = StateFacts::new(snapshot.clone())
        .refine(
            &snapshot,
            [
                Observation::Code {
                    address: address(0x202),
                    code: CodeObservation::Runtime(callee),
                },
                Observation::Storage {
                    address: address(0x101),
                    slot: U256::ZERO,
                    value: U256::from(7),
                },
                Observation::Storage {
                    address: address(0x202),
                    slot: U256::ZERO,
                    value: U256::from(99),
                },
            ],
        )
        .unwrap()
        .into_facts();
    let mut session = AnalysisSession::new(
        Arc::new(catalog),
        facts,
        caller,
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap();
    let report = session.analyze().unwrap();
    assert_eq!(
        report.analysis().status(),
        Status::Converged,
        "{:?}",
        report.analysis().frontiers()
    );
    assert!(
        report
            .analysis()
            .states()
            .iter()
            .any(|state| state.active().code_address == address(0x202)
                && state.active().address == address(0x101))
    );
    assert!(
        returns(&report)
            .iter()
            .all(|value| value.singleton() == Some(U256::from(7)))
    );
    for owner in [address(0x101), address(0x202)] {
        assert_eq!(session.facts().balance(owner), Observed::Unobserved);
        assert_eq!(session.facts().nonce(owner), Observed::Unobserved);
        assert_eq!(session.facts().existence(owner), Observed::Unobserved);
    }
}

#[test]
fn exact_initial_fact_does_not_bypass_static_write_failure() {
    let (catalog, id) = catalog("60095f5500");
    let snapshot = SnapshotId::offline("static owner");
    let facts = StateFacts::new(snapshot.clone())
        .refine(
            &snapshot,
            [Observation::Storage {
                address: address(0x101),
                slot: U256::ZERO,
                value: U256::from(3),
            }],
        )
        .unwrap()
        .into_facts();
    let mut entry = Entry::new(address(0x101));
    entry.environment.is_static = true;
    let mut session =
        AnalysisSession::new(catalog, facts, id, entry, ExecutionConfig::default()).unwrap();
    let report = session.analyze().unwrap();
    assert_eq!(report.analysis().status(), Status::Converged);
    assert!(
        report
            .analysis()
            .outcomes()
            .iter()
            .all(|outcome| outcome.kind == OutcomeKind::Failure)
    );
    for outcome in report.analysis().outcomes() {
        assert_eq!(
            outcome
                .store
                .read(address(0x101), &word(0), Domain::default())
                .singleton(),
            Some(U256::from(3))
        );
    }
}
