use crate::domain::EvmNumericAlgebra as _;

use super::*;
#[cfg(test)]
use crate::analysis::testing::{Account, Entry, TestValue, World};
use crate::{
    Address, Fork, U256,
    analysis::{ExecutionConfig, FrontierReason, OutcomeKind, Status},
    bytecode::Program,
    domain::ProductAlgebra,
    world::{Code, Existence},
};
use alloy_primitives::B256;
use std::sync::Arc;

fn address(value: u16) -> Address {
    let mut bytes = [0u8; 20];
    bytes[18..].copy_from_slice(&value.to_be_bytes());
    Address::from(bytes)
}

fn word(value: u64) -> TestValue {
    TestValue::constant(U256::from(value))
}

fn catalog(code: &str) -> (Arc<CodeCatalog>, CodeId) {
    let program = Program::from_hex_with_fork(code, Fork::Osaka).unwrap();
    let (catalog, id) = CodeCatalog::new(Fork::Osaka).with_runtime(program).unwrap();
    (Arc::new(catalog), id)
}

use crate::analysis::{AnalysisEngine, AnalysisInput, EnginePolicies, control::Control, cooperation::NoExtension};
use std::{convert::Infallible, sync::LazyLock};
type Session = AnalysisSession<'static, ProductAlgebra, crate::analysis::RecentDistinctJumps,
    crate::analysis::FirstResumeStatus, crate::analysis::SemanticPolicies, NoExtension>;
static NUMERIC: LazyLock<crate::domain::ValueAlgebra<ProductAlgebra>> = LazyLock::new(|| crate::domain::ValueAlgebra::new(ProductAlgebra::default()));
static POLICIES: LazyLock<EnginePolicies> = LazyLock::new(EnginePolicies::default);
fn engine() -> AnalysisEngine<'static, ProductAlgebra, crate::analysis::RecentDistinctJumps,
    crate::analysis::FirstResumeStatus, crate::analysis::SemanticPolicies, NoExtension> {
    AnalysisEngine::new(&NUMERIC, &POLICIES, Box::leak(Box::new(NoExtension)))
}
fn from_world(world: World, entry: Entry, config: ExecutionConfig) -> Result<Session, SessionError<Infallible>> {
    AnalysisSession::new(engine(), AnalysisInput::new(world, entry)?, config)
}
fn start(catalog: Arc<CodeCatalog>, facts: StateFacts, id: CodeId, entry: Entry, config: ExecutionConfig)
    -> Result<Session, SessionError<Infallible>> {
    let mut world = match facts.snapshot().identity() {
        crate::world::SnapshotIdentity::Offline { label } => World::offline(catalog.fork(), label, "explicit catalog/facts fixture"),
        crate::world::SnapshotIdentity::Chain { chain_id, block_hash } => World::anchored(catalog.fork(), *chain_id, *block_hash, "explicit catalog/facts fixture"),
    };
    let mut owner = Account::unknown();
    owner.code = Code::Runtime(catalog.program(id).ok_or(SessionError::MissingCode(id))?.clone());
    world.insert(entry.address, owner)?;
    for (_, fact) in facts.observations() {
        if let Observation::Code { address, code } = fact {
            if *address == entry.address { continue; }
            let mut account = Account::unknown();
            account.code = match code { CodeObservation::Empty => Code::Empty,
                CodeObservation::Delegation(target) => Code::Delegation(*target),
                CodeObservation::Runtime(id) => Code::Runtime(catalog.program(*id).ok_or(SessionError::MissingCode(*id))?.clone()), };
            world.insert(*address, account)?;
        }
    }
    let input = AnalysisInput { world, entry, facts };
    AnalysisSession::new(engine(), input, config)
}

fn session(code: &str) -> Session {
    let (catalog, id) = catalog(code);
    start(
        catalog,
        StateFacts::new(SnapshotId::offline("layer tests")),
        id,
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap()
}

fn returns(report: &SessionAnalysis<crate::domain::NumericValue>) -> Vec<TestValue> {
    let values: Vec<_> = report
        .analysis()
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .map(|outcome| outcome.data.read_word(&word(0), NUMERIC.borrowed()))
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
        let report = session.run(&Control::default()).unwrap();
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
    let old = session.run(&Control::default()).unwrap();
    assert!(
        returns(&old)
            .iter()
            .all(|value| value.singleton().is_none())
    );
    assert!(old.reuse().entries > 0);
    assert!(Arc::ptr_eq(&old, &session.run(&Control::default()).unwrap()));
    let snapshot = session.facts().snapshot().clone();
    let effect = session
        .refine(
            &snapshot,
            [Observation::Storage {
                address: address(0x101),
                slot: U256::ZERO,
                value: U256::from(3),
            }], &Control::default())
        .unwrap();
    assert!(
        matches!(effect, RefinementEffect::RestartRequired { ref affected_states, .. } if !affected_states.is_empty())
    );
    let new = session.run(&Control::default()).unwrap();
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
        let old = session.run(&Control::default()).unwrap();
        let snapshot = session.facts().snapshot().clone();
        session
            .refine(
                &snapshot,
                [Observation::Storage {
                    address: address(0x101),
                    slot: U256::ZERO,
                    value: U256::from(3),
                }], &Control::default())
            .unwrap();
        let new = session.run(&Control::default()).unwrap();
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
                    .read(address(0x101), &word(0), ProductAlgebra::default())
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
                        .read(address(0x101), &word(0), ProductAlgebra::default())
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
    let mut left = start(
        Arc::clone(&catalog),
        StateFacts::new(SnapshotId::offline("left")),
        id,
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap();
    let mut right = start(
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
                }], &Control::default())
            .unwrap();
        let report = session.run(&Control::default()).unwrap();
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
    let report = session.run(&Control::default()).unwrap();
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
            , &Control::default())
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
            }], &Control::default())
        .unwrap();
    let report = session.run(&Control::default()).unwrap();
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
                .read_word(&word(0), ProductAlgebra::default())
                .singleton()
                .is_none())
    );
    let mut world = World::offline(Fork::Osaka, "bounded base", "test");
    let mut account = Account::unknown();
    account.code = Code::Runtime(program);
    account.balance = TestValue::unsigned_range(U256::from(3), U256::from(9)).unwrap();
    account.storage.insert(
        U256::ZERO,
        TestValue::unsigned_range(U256::from(5), U256::from(8)).unwrap(),
    );
    account.storage_unknown = false;
    world.insert(address(0x101), account.clone()).unwrap();
    let public = crate::analysis::testing::execute(
        world.clone(),
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
        crate::domain::DomainSpec::default(),
    )
    .unwrap();
    assert_eq!(public.world().account(address(0x101)), Some(&account));
    let mut session = from_world(
        world,
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap();
    let before = session.run(&Control::default()).unwrap();
    assert_eq!(
        before.analysis().world().account(address(0x101)),
        Some(&account)
    );
    let state = before.analysis().states().first().unwrap();
    assert_eq!(
        state.entry.store.read_balance(address(0x101)),
        account.balance
    );
    let value = state
        .execution_evidence()
        .unwrap()
        .instruction_values()
        .unwrap()
        .iter()
        .find(|value| state.executed_pcs[value.instruction_index()] == 0)
        .expect("SELFBALANCE must produce an instruction result");
    assert_eq!(value.numeric(), account.balance.numeric());
    let returned = returns(&before);
    assert!(
        returned.iter().all(|v| !v.contains(U256::from(2))
            && v.contains(U256::from(3))
            && v.contains(U256::from(9))
            && !v.contains(U256::from(10))),
        "returned byte view must retain the Store and SELFBALANCE bounds: {returned:?}"
    );
    let snapshot = session.facts().snapshot().clone();
    let revision = session.facts().revision();
    for observation in [
        Observation::Balance {
            address: address(0x101),
            value: U256::from(2),
        },
        Observation::Balance {
            address: address(0x101),
            value: U256::from(10),
        },
        Observation::Storage {
            address: address(0x101),
            slot: U256::ZERO,
            value: U256::ZERO,
        },
        Observation::Storage {
            address: address(0x101),
            slot: U256::ZERO,
            value: U256::from(9),
        },
        Observation::Storage {
            address: address(0x101),
            slot: U256::from(1),
            value: U256::from(1),
        },
    ] {
        assert!(session.refine(&snapshot, [observation], &Control::default()).is_err());
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
            ], &Control::default())
        .unwrap();
    let refined = session.run(&Control::default()).unwrap();
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
fn world_refinement_preserves_value_layers_and_rejects_both_bound_violations_atomically() {
    use crate::{

        domain::{NumericValue, identity::Symbol, provenance::Origin},
    };
    let owner = address(0x101);
    let neighbor = address(0x202);
    let bounded = |lower, upper, symbol, origin| {
        let mut value = TestValue::unsigned_range(U256::from(lower), U256::from(upper))
            .unwrap()
            .with_symbol(symbol, Some(1029));
        value.set_origin(origin);
        value
    };
    let mut account = Account::unknown();
    account.code = Code::Runtime(Program::from_hex_with_fork("00", Fork::Osaka).unwrap());
    account.balance = bounded(3u64, 9u64, Symbol::CallValue, Origin::Balance);
    account.nonce = bounded(11u64, 17u64, Symbol::Number, Origin::Nonce);
    account.storage.insert(
        U256::ZERO,
        bounded(
            5u64,
            8u64,
            Symbol::CalldataWord(U256::ZERO),
            Origin::Storage,
        ),
    );
    let mut world = World::offline(Fork::Osaka, "layered bounds and sources", "input world");
    world.insert(owner, account.clone()).unwrap();
    world.insert(neighbor, Account::unknown()).unwrap();
    let mut session =
        from_world(world.clone(), Entry::new(owner), ExecutionConfig::default())
            .unwrap();
    assert_eq!(session.facts().balance(owner), Observed::Unobserved);
    assert_eq!(session.facts().nonce(owner), Observed::Unobserved);
    assert_eq!(
        session.facts().storage(owner, U256::ZERO),
        Observed::Unobserved
    );
    let before = session.run(&Control::default()).unwrap();
    assert_eq!(before.analysis().world().accounts(), world.accounts());
    let snapshot = session.facts().snapshot().clone();
    let revision = session.facts().revision();
    for observation in [
        Observation::Balance {
            address: owner,
            value: U256::from(2),
        },
        Observation::Balance {
            address: owner,
            value: U256::from(10),
        },
        Observation::Nonce {
            address: owner,
            value: U256::from(10),
        },
        Observation::Nonce {
            address: owner,
            value: U256::from(18),
        },
        Observation::Storage {
            address: owner,
            slot: U256::ZERO,
            value: U256::from(4),
        },
        Observation::Storage {
            address: owner,
            slot: U256::ZERO,
            value: U256::from(9),
        },
    ] {
        let key = observation.key();
        assert!(matches!(
            session.refine(&snapshot, [observation], &Control::default()),
            Err(SessionError::Facts(FactError::Conflict { key: rejected })) if rejected == key
        ));
        assert_eq!(session.facts().revision(), revision);
        assert!(Arc::ptr_eq(&before, &session.run(&Control::default()).unwrap()));
    }
    let observations = [
        Observation::Balance {
            address: owner,
            value: U256::from(7),
        },
        Observation::Nonce {
            address: owner,
            value: U256::from(13),
        },
        Observation::Storage {
            address: owner,
            slot: U256::ZERO,
            value: U256::from(6),
        },
    ];
    let mut invalid_batch = observations.clone();
    invalid_batch[2] = Observation::Storage {
        address: owner,
        slot: U256::ZERO,
        value: U256::from(9),
    };
    assert!(session.refine(&snapshot, invalid_batch, &Control::default()).is_err());
    assert_eq!(session.facts().revision(), revision);
    assert!(Arc::ptr_eq(&before, &session.run(&Control::default()).unwrap()));
    session.refine(&snapshot, observations, &Control::default()).unwrap();
    let after = session.run(&Control::default()).unwrap();
    let projected = after.analysis().world();
    let mut expected = account.clone();
    expected.balance = expected
        .balance
        .with_numeric(NumericValue::constant(U256::from(7)));
    expected.nonce = expected
        .nonce
        .with_numeric(NumericValue::constant(U256::from(13)));
    expected.storage.insert(
        U256::ZERO,
        account.storage[&U256::ZERO]
            .clone()
            .with_numeric(NumericValue::constant(U256::from(6))),
    );
    // Whole-value equality checks provenance, scoped identity and expression
    // as well as numeric facts. Unobserved neighbors must not acquire zeroes.
    assert_eq!(projected.account(owner), Some(&expected));
    assert_eq!(projected.account(neighbor), world.account(neighbor));
    assert_eq!(projected.identity(), world.identity());
    assert_eq!(projected.provenance(), world.provenance());
    assert_eq!(
        projected.storage_source_token(),
        world.storage_source_token()
    );
    assert_eq!(session.base().accounts(), world.accounts());
    assert_eq!(before.analysis().world().accounts(), world.accounts());
    assert_eq!(before.revision(), revision);
    assert_eq!(after.snapshot(), &snapshot);
}

#[test]
fn completed_report_cache_observes_current_and_captured_cancellation() {
    use crate::analysis::{control::Control, progress::Observer};
    let mut current = session("00");
    let report = current.run(&Control::default()).unwrap();
    let control = Control::new(Observer::default());
    control.cancellation().cancel();
    assert!(current.run(&control).is_err());
    assert!(current.run(&control).is_err());
    assert_eq!(report.analysis().status(), Status::Converged);
    let captured = Control::new(Observer::default());
    let mut session = session("00");
    session.run(&captured).unwrap();
    captured.cancellation().cancel();
    assert!(session.run(&Control::default()).is_err());
}

#[test]
fn offline_demands_inject_initial_values_after_runtime_write_and_preserve_rollback() {
    let mut session = session("60095f55600154505f5ffd");
    let pending = session.run_with_demands(&Control::default()).unwrap();
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
            }], &Control::default())
        .unwrap();
    let refined = session.run_with_demands(&Control::default()).unwrap();
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
                .read(address(0x101), &word(0), ProductAlgebra::default())
                .singleton()
                .is_none()
        );
        assert_eq!(
            outcome
                .store
                .read(address(0x101), &word(1), ProductAlgebra::default())
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
    let original = session.run(&Control::default()).unwrap();
    let snapshot = session.facts().snapshot().clone();
    let first = session
        .refine(
            &snapshot,
            [Observation::Existence {
                address: address(0x101),
                exists: true,
            }], &Control::default())
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
            }], &Control::default())
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
    let mut session = start(
        Arc::new(catalog),
        facts,
        caller,
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap();
    let report = session.run(&Control::default()).unwrap();
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
        start(catalog, facts, id, entry, ExecutionConfig::default()).unwrap();
    let report = session.run(&Control::default()).unwrap();
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
                .read(address(0x101), &word(0), ProductAlgebra::default())
                .singleton(),
            Some(U256::from(3))
        );
    }
}
