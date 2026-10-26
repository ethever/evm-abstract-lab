//! Installed tests cross Session, the native engine and real finite HTTP RPC.

mod publication;
mod transport;

use super::{RpcBaselineRelation, RpcHybridAnalysis, RpcPrecisionOutcome};
use crate::{
    U256,
    analysis::{
        AnalysisEngine, AnalysisInput, EnginePolicies, ExecutionConfig, FrontierReason,
        HybridConfig, HybridInterpretation, HybridLocalScope, HybridNativeBinding, MachineEdgeKind,
        ProcessSource, ProjectionError, Status, analyze_rpc_hybrid,
        control::Control,
        cooperation::NoExtension,
        layers::{AnalysisSession, Observation, SessionError, SnapshotId},
        transfer::StorageReadPolicy,
    },
    domain::{NumericValue, ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
    world::{
        AddressInput, EvmEntry, EvmEnvironmentValues, RootInputBinding,
        rpc::{RpcFailureKind, RpcInput},
    },
};
use std::{collections::BTreeMap, thread};
use transport::{Fixture, Server, address};

fn entry() -> EvmEntry<NumericValue> {
    EvmEntry {
        address: address(0x101),
        environment: EvmEnvironmentValues {
            to: address(0x101).into(),
            caller: AddressInput::unknown_caller(),
            ..Default::default()
        },
    }
}

fn fixture(code: &str) -> Fixture {
    Fixture {
        code: BTreeMap::from([(address(0x101), code.to_owned())]),
        ..Default::default()
    }
}

// Match the declared CLI/Web request maximum (cli/defaults.rs and
// protocol/request.rs). The generic engine's 20M control remains separate.
fn product_config() -> ExecutionConfig {
    ExecutionConfig {
        max_work: 2_000_000_000,
        ..ExecutionConfig::default()
    }
}

fn run(
    input: &RpcInput,
    config: ExecutionConfig,
    hybrid: HybridConfig,
    control: &Control,
) -> RpcHybridAnalysis<NumericValue> {
    try_run(input, config, hybrid, control).unwrap()
}

fn try_run(
    input: &RpcInput,
    config: ExecutionConfig,
    hybrid: HybridConfig,
    control: &Control,
) -> Result<
    RpcHybridAnalysis<NumericValue>,
    crate::analysis::RpcAnalysisError<std::convert::Infallible>,
> {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    analyze_rpc_hybrid(
        AnalysisEngine::new(&algebra, &policies, &mut extension),
        input,
        entry(),
        config,
        hybrid,
        control,
    )
}

fn assert_accounting(report: &RpcHybridAnalysis<NumericValue>, maximum: usize) {
    let c = report.coverage();
    let original = report.baseline().baseline();
    let binding = original.root_binding();
    assert_eq!(binding, original.entry().binding());
    assert_eq!(report.latest().baseline().root_binding(), binding);
    assert_eq!(
        serde_json::to_value(original.entry()).unwrap()["binding"],
        serde_json::to_value(binding).unwrap()
    );
    assert!(report.work() <= maximum);
    assert_eq!(report.work(), c.work);
    assert_eq!(report.work(), report.baseline().work() + c.precision_work);
    assert!(c.precision_work <= c.max_precision_work);
    assert_eq!(report.baseline().baseline().revision(), c.baseline_revision);
    assert_eq!(report.baseline().baseline().snapshot(), &c.snapshot);
    let mut revision = c.baseline_revision;
    for change in &c.restrictions {
        assert_eq!(change.previous, revision);
        assert_ne!(change.current, change.previous);
        assert!(change.added_observations > 0);
        revision = change.current;
    }
    assert_eq!(revision, c.current_revision);
    assert!(c.acquisition.states_created >= report.baseline().baseline().states().len());
    if let Some(detail) = report.detail() {
        assert_eq!(detail.snapshot(), &c.snapshot);
        assert_eq!(Some(detail.revision()), c.detail_revision);
        assert!(detail.work() <= report.work());
        let native_binding = report.detail_report().unwrap().coverage().detail_binding;
        match native_binding {
            HybridNativeBinding::OriginalOwner => {
                assert_eq!(detail.root_binding(), binding);
                assert!(
                    report
                        .detail_report()
                        .unwrap()
                        .coverage()
                        .detail_regions
                        .iter()
                        .all(|region| matches!(region.source, ProcessSource::Runtime { .. }))
                );
            }
            HybridNativeBinding::SymbolicOwnerNamespace { namespace } => {
                assert_eq!(
                    binding,
                    RootInputBinding::Namespace {
                        coordinate: namespace
                    }
                );
                assert_eq!(
                    detail.root_binding(),
                    RootInputBinding::Physical { address: namespace }
                );
            }
        }
    }
    for receipt in report.local_detail() {
        assert_eq!(receipt.snapshot, &c.snapshot);
        assert_eq!(Some(receipt.revision), c.detail_revision);
        assert_eq!(
            receipt.binding,
            report.detail_report().unwrap().coverage().detail_binding
        );
        assert_eq!(
            receipt.scope,
            match receipt.binding {
                HybridNativeBinding::OriginalOwner => HybridLocalScope::RetainedInputPrefix,
                HybridNativeBinding::SymbolicOwnerNamespace { .. } =>
                    HybridLocalScope::NativeOwnerNamespacePrefix,
            }
        );
    }
}

fn declared_source(report: &crate::analysis::EvmWorldAnalysis<NumericValue>) -> ProcessSource {
    // Independent presentation work pays only the core's declared-input getter.
    // It cannot admit/restart a kernel, alter the Session ledger or certify closure.
    let mut provenance = WorkBudget::new(1024);
    let source = report.declared_root_source(&mut provenance).unwrap();
    assert!(provenance.used() > 0);
    source
}

#[test]
fn authentic_root_and_unknown_inputs_survive_storage_to_exact_jump_refinement() {
    thread::scope(|scope| {
        // Slot zero supplies the sole concrete JUMP destination, PC4.
        let mut f = fixture("5f54565b5b00");
        f.storage
            .insert((address(0x101), U256::ZERO), U256::from(4));
        let server = Server::new(scope, f);
        let report = run(
            &server.input(),
            product_config(),
            HybridConfig::default(),
            &Control::default(),
        );
        assert_eq!(
            report.status(),
            Status::Converged,
            "{:?}",
            report.coverage()
        );
        assert_eq!(
            report.coverage().precision,
            RpcPrecisionOutcome::Current,
            "{:?}",
            report.latest().coverage()
        );
        assert_eq!(
            report.coverage().baseline_relation,
            RpcBaselineRelation::InitialFactRestriction
        );
        assert_ne!(
            report.coverage().baseline_revision,
            report.coverage().current_revision
        );
        assert!(
            report
                .baseline()
                .baseline()
                .world()
                .account(address(0x101))
                .unwrap()
                .storage
                .is_empty()
        );
        let detail = report.detail().unwrap();
        assert_eq!(detail.revision(), report.coverage().current_revision);
        assert_eq!(
            detail.world().account(address(0x101)).unwrap().storage[&U256::ZERO].singleton(),
            Some(U256::from(4))
        );
        assert!(
            detail
                .world()
                .account(address(0x101))
                .unwrap()
                .storage_unknown
        );
        assert!(matches!(
            detail.entry().environment.caller,
            AddressInput::Symbolic(_)
        ));
        assert!(detail.entry().environment.value.singleton().is_none());
        assert!(
            detail
                .entry()
                .environment
                .calldata
                .len()
                .singleton()
                .is_none()
        );
        let targets: Vec<_> = detail
            .edges()
            .iter()
            .filter(|e| e.kind == MachineEdgeKind::Intraprocedural(crate::analysis::EdgeKind::Jump))
            .map(|e| {
                detail.world().runtime(address(0x101)).unwrap().blocks()
                    [detail.states()[e.to].active().basic_block_index]
                    .start_pc
            })
            .collect();
        assert_eq!(targets, [4]);
        assert_eq!(server.count("eth_getCode", address(0x101)), 1);
        assert_eq!(server.count("eth_getStorageAt", address(0x101)), 1);
        assert_eq!(report.coverage().acquisition.fetched_storage.len(), 1);
        assert!(report.coverage().acquisition.rounds >= 3);
        for request in server.requests() {
            if matches!(
                request["method"].as_str(),
                Some(
                    "eth_getCode"
                        | "eth_getBalance"
                        | "eth_getTransactionCount"
                        | "eth_getStorageAt"
                )
            ) {
                let block = request["params"].as_array().unwrap().last().unwrap();
                assert_eq!(
                    block["blockHash"],
                    serde_json::json!(alloy_primitives::B256::repeat_byte(0x11))
                );
                assert_eq!(block["requireCanonical"], true);
            }
        }
        assert_accounting(&report, product_config().max_work);
    });
}

#[test]
fn delegated_root_then_two_storage_demands_restart_under_one_ledger() {
    thread::scope(|scope| {
        let delegated = format!("ef0100{}", alloy_primitives::hex::encode(address(0x200)));
        let mut f = fixture(&delegated);
        // First read chooses a second slot, then returns that slot's word.
        f.code
            .insert(address(0x200), "5f54545f5260205ff3".to_owned());
        f.storage
            .insert((address(0x101), U256::ZERO), U256::from(1));
        f.storage
            .insert((address(0x101), U256::from(1)), U256::from(9));
        let server = Server::new(scope, f);
        let report = run(
            &server.input(),
            product_config(),
            HybridConfig::default(),
            &Control::default(),
        );
        assert_eq!(
            report.status(),
            Status::Converged,
            "{:?}",
            report.coverage()
        );
        assert_eq!(
            report.coverage().precision,
            RpcPrecisionOutcome::Current,
            "{:?}",
            report.latest().coverage()
        );
        assert_eq!(
            report.coverage().acquisition.fetched_accounts,
            [address(0x200)]
        );
        assert_eq!(report.coverage().acquisition.fetched_storage.len(), 2);
        assert_eq!(report.coverage().restrictions.len(), 3);
        assert!(report.coverage().acquisition.rounds >= 5);
        assert_eq!(server.count("eth_getCode", address(0x200)), 1);
        assert_eq!(server.count("eth_getStorageAt", address(0x101)), 2);
        assert!(
            report
                .baseline()
                .baseline()
                .world()
                .account(address(0x200))
                .is_none()
        );
        assert_eq!(
            report.latest().coverage().detail_regions[0].code_address,
            address(0x200)
        );
        assert_eq!(
            report.latest().coverage().detail_regions[0].source,
            declared_source(report.detail().unwrap())
        );
        assert!(
            report
                .local_detail()
                .any(|site| site.frame.code_address == address(0x200)
                    && site.frame.address == address(0x101))
        );
        assert!(
            report.coverage().acquisition.states_created > report.detail().unwrap().states().len()
        );
        assert!(report.latest().coverage().work.prior > report.baseline().work());
        assert_eq!(
            declared_source(report.baseline().baseline()),
            ProcessSource::AnyRuntime {
                fork: crate::Fork::Osaka
            }
        );
        assert_eq!(
            declared_source(report.detail().unwrap()),
            ProcessSource::Runtime {
                source: address(0x200),
                hash: report
                    .detail()
                    .unwrap()
                    .world()
                    .runtime(address(0x200))
                    .unwrap()
                    .bytes_digest(),
                fork: crate::Fork::Osaka,
            }
        );
        assert_accounting(&report, product_config().max_work);
    });
}

#[test]
fn late_code_failure_preserves_original_global_certificate_and_actual_error() {
    thread::scope(|scope| {
        let mut f = fixture("5f5f5f5f6102006207a120fa5000");
        f.code.insert(address(0x200), "00".to_owned());
        f.fail_code = Some(address(0x200));
        let server = Server::new(scope, f);
        let report = run(
            &server.input(),
            product_config(),
            HybridConfig::default(),
            &Control::default(),
        );
        assert_eq!(
            report.status(),
            Status::Converged,
            "{:?}",
            report.coverage()
        );
        assert_eq!(
            report.coverage().precision,
            RpcPrecisionOutcome::AcquisitionFailed
        );
        assert_eq!(report.failures().len(), 1);
        assert_eq!(report.failures()[0].failure().kind, RpcFailureKind::Remote);
        assert_eq!(
            report.coverage().acquisition.failed_accounts,
            [address(0x200)]
        );
        assert_eq!(server.count("eth_getCode", address(0x200)), 1);
        assert!(
            report
                .detail()
                .unwrap()
                .frontiers()
                .iter()
                .any(|f| matches!(f.reason, FrontierReason::MissingCode(a) if a == address(0x200)))
        );
        assert!(report.baseline().baseline().frontiers().is_empty());
        assert!(report.coverage().restrictions.is_empty());
        assert_accounting(&report, product_config().max_work);
    });
}

#[test]
fn late_identity_change_rejects_entire_storage_batch_and_revision() {
    for chain in [true, false] {
        thread::scope(|scope| {
            let mut f = fixture("5f545000");
            f.wrong_chain_on_storage = chain;
            f.wrong_hash_on_storage = !chain;
            let server = Server::new(scope, f);
            let report = run(
                &server.input(),
                product_config(),
                HybridConfig::default(),
                &Control::default(),
            );
            assert_eq!(
                report.status(),
                Status::Converged,
                "{:?}",
                report.coverage()
            );
            assert_eq!(
                report.coverage().precision,
                RpcPrecisionOutcome::AcquisitionFailed
            );
            assert_eq!(report.failures().len(), 1);
            assert!(report.coverage().restrictions.is_empty());
            assert_eq!(
                report.coverage().baseline_revision,
                report.coverage().current_revision
            );
            assert!(report.coverage().acquisition.fetched_storage.is_empty());
            assert_eq!(report.coverage().acquisition.failed_storage.len(), 1);
            assert!(
                report
                    .detail()
                    .unwrap()
                    .world()
                    .account(address(0x101))
                    .unwrap()
                    .storage
                    .is_empty()
            );
            assert_accounting(&report, product_config().max_work);
        });
    }
}

#[test]
fn optional_work_refusal_keeps_baseline_and_larger_cap_reaches_real_precision() {
    thread::scope(|scope| {
        let mut f = fixture("5f545000");
        f.storage
            .insert((address(0x101), U256::ZERO), U256::from(7));
        let server = Server::new(scope, f);
        let partial = run(
            &server.input(),
            product_config(),
            HybridConfig {
                max_detail_work: 1,
                ..Default::default()
            },
            &Control::default(),
        );
        assert_eq!(partial.status(), Status::Converged);
        assert_eq!(partial.coverage().precision, RpcPrecisionOutcome::Work);
        assert!(partial.detail().is_none());
        assert_eq!(server.count("eth_getStorageAt", address(0x101)), 0);
        assert_accounting(&partial, product_config().max_work);
        let complete = run(
            &server.input(),
            product_config(),
            HybridConfig::default(),
            &Control::default(),
        );
        assert_eq!(
            complete.coverage().precision,
            RpcPrecisionOutcome::Current,
            "{:?}",
            complete.coverage()
        );
        assert_eq!(server.count("eth_getStorageAt", address(0x101)), 1);
        assert!(complete.work() > partial.work());
        assert_eq!(
            complete
                .detail()
                .unwrap()
                .world()
                .account(address(0x101))
                .unwrap()
                .storage[&U256::ZERO]
                .singleton(),
            Some(U256::from(7))
        );
        assert_accounting(&complete, product_config().max_work);
    });
}

#[test]
fn late_cancellation_of_rpc_and_original_payer_returns_typed_error_without_publication() {
    thread::scope(|scope| {
        let control = Control::new(Default::default());
        let mut f = fixture("5f545000");
        f.cancel_storage = Some(control.cancellation().clone());
        let server = Server::new(scope, f);
        let result = try_run(
            &server.input(),
            product_config(),
            HybridConfig::default(),
            &control,
        );
        assert!(matches!(
            result,
            Err(crate::analysis::RpcAnalysisError::Cancelled(_))
        ));
        // The callback cancels the same token captured by the retained payer,
        // after the original baseline and first native demand already ran.
        assert!(control.cancellation().is_cancelled());
        assert_eq!(server.count("eth_getStorageAt", address(0x101)), 1);
        assert_eq!(server.count("eth_getCode", address(0x101)), 1);
        let requests = server.requests();
        assert_eq!(requests.last().unwrap()["method"], "eth_getStorageAt");
    });
}

#[test]
fn session_hybrid_rejects_sticky_refusal_before_any_second_phase_or_copy() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let (world, entry) = crate::analysis::machine::fixtures::fixture(&[(0x101, "00")]);
    let mut session = AnalysisSession::new(
        AnalysisEngine::new(&algebra, &policies, &mut extension),
        AnalysisInput::new(world, entry).unwrap(),
        ExecutionConfig::default(),
    )
    .unwrap();
    let first = session
        .run_hybrid_uncached(
            HybridConfig {
                max_detail_work: 0,
                ..Default::default()
            },
            StorageReadPolicy::Abstract,
            &Control::default(),
        )
        .unwrap();
    assert!(first.coverage().baseline_current);
    // MAX is an overflow sentinel that intentionally does not charge/refuse
    // the payer. A finite amount above the actual remainder is a sticky refusal.
    assert_eq!(
        session.reserve_acquisition(ExecutionConfig::default().max_work),
        Some(FrontierReason::Work)
    );
    let work = session.work_used();
    let states = session.states_created();
    let transfers = session.transfers_used();
    assert!(matches!(
        session.run_hybrid_uncached(
            HybridConfig::default(),
            StorageReadPolicy::Discover,
            &Control::default()
        ),
        Err(SessionError::Work)
    ));
    assert_eq!(
        (
            session.work_used(),
            session.states_created(),
            session.transfers_used()
        ),
        (work, states, transfers)
    );
    assert!(first.coverage().baseline_current);
}

#[test]
fn wrong_snapshot_or_fork_cannot_change_the_original_hybrid_report() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let (world, entry) = crate::analysis::machine::fixtures::fixture(&[(0x101, "5f545000")]);
    let mut session = AnalysisSession::new(
        AnalysisEngine::new(&algebra, &policies, &mut extension),
        AnalysisInput::new(world, entry).unwrap(),
        ExecutionConfig::default(),
    )
    .unwrap();
    let original = session
        .run_hybrid_uncached(
            HybridConfig {
                max_detail_work: 0,
                ..Default::default()
            },
            StorageReadPolicy::Abstract,
            &Control::default(),
        )
        .unwrap();
    let fingerprint = original.baseline().world().fingerprint().unwrap();
    let revision = session.facts().revision();
    assert!(
        session
            .refine(
                &SnapshotId::offline("wrong"),
                [Observation::Storage {
                    address: address(0x101),
                    slot: U256::ZERO,
                    value: U256::from(7)
                }],
                &Control::default()
            )
            .is_err()
    );
    assert!(
        session
            .supply_runtime(
                crate::bytecode::Program::from_hex_with_fork("00", crate::Fork::Cancun).unwrap()
            )
            .is_err()
    );
    assert_eq!(session.facts().revision(), revision);
    assert_eq!(original.baseline().revision(), revision);
    assert_eq!(
        original.baseline().world().fingerprint().unwrap(),
        fingerprint
    );
    assert!(original.coverage().baseline_current);
}

#[test]
fn proxy_storage_selects_missing_implementation_and_preserves_delegate_owner() {
    thread::scope(|scope| {
        let mut f = fixture("5f5f5f5f6009546207a120f45000");
        f.storage
            .insert((address(0x101), U256::from(9)), U256::from(0x200));
        f.code.insert(address(0x200), "600154565b5b00".to_owned());
        f.storage
            .insert((address(0x101), U256::from(1)), U256::from(5));
        let server = Server::new(scope, f);
        let report = run(
            &server.input(),
            product_config(),
            HybridConfig::default(),
            &Control::default(),
        );
        assert_eq!(
            report.status(),
            Status::Converged,
            "{:?}",
            report.coverage()
        );
        assert_eq!(
            report.coverage().precision,
            RpcPrecisionOutcome::Current,
            "{:?}",
            report.coverage()
        );
        assert_eq!(
            report.coverage().acquisition.fetched_accounts,
            [address(0x200)]
        );
        assert_eq!(report.coverage().acquisition.fetched_storage.len(), 2);
        assert_eq!(report.coverage().restrictions.len(), 3);
        assert_eq!(server.count("eth_getStorageAt", address(0x101)), 2);
        assert_eq!(server.count("eth_getStorageAt", address(0x200)), 0);
        assert_eq!(server.count("eth_getCode", address(0x200)), 1);
        let detail = report.detail().unwrap();
        let implementation = detail.world().runtime(address(0x200)).unwrap();
        let target_pcs: Vec<_> = report
            .local_detail()
            .filter(|s| s.frame.code_address == address(0x200))
            .flat_map(|s| s.successors.iter())
            .filter(|(_, kind)| {
                *kind == MachineEdgeKind::Intraprocedural(crate::analysis::EdgeKind::Jump)
            })
            .map(|(to, _)| {
                implementation.blocks()[detail.states()[*to].active().basic_block_index].start_pc
            })
            .collect();
        assert_eq!(target_pcs, [5]);
        assert!(
            report
                .local_detail()
                .filter(|s| s.frame.code_address == address(0x200))
                .all(|s| s.frame.address == address(0x101))
        );
        assert_accounting(&report, product_config().max_work);
    });
}

#[test]
fn incomplete_original_baseline_cannot_start_optional_precision() {
    thread::scope(|scope| {
        let server = Server::new(scope, fixture("00"));
        let first = run(
            &server.input(),
            ExecutionConfig::default(),
            HybridConfig {
                max_detail_work: 0,
                ..Default::default()
            },
            &Control::default(),
        );
        assert!(first.baseline().coverage().baseline_current);
        let config = ExecutionConfig {
            max_work: first.baseline().coverage().work.prior + 1,
            ..Default::default()
        };
        let refused = run(
            &server.input(),
            config.clone(),
            HybridConfig::default(),
            &Control::default(),
        );
        assert_eq!(refused.status(), Status::Incomplete);
        assert_eq!(
            refused.coverage().baseline_relation,
            RpcBaselineRelation::Uncertified
        );
        assert_eq!(
            refused.coverage().precision,
            RpcPrecisionOutcome::BaselinePending
        );
        assert_eq!(refused.coverage().precision_work, 0);
        assert_eq!(refused.coverage().acquisition.rounds, 1);
        assert!(refused.detail().is_none());
        let baseline = refused.baseline().baseline();
        assert!(baseline.states().is_empty());
        assert_eq!(
            baseline.root_binding(),
            RootInputBinding::Physical {
                address: address(0x101)
            }
        );
        let spent = refused.work();
        assert_eq!(
            declared_source(baseline),
            ProcessSource::Runtime {
                source: address(0x101),
                hash: alloy_primitives::keccak256([0]),
                fork: crate::Fork::Osaka,
            }
        );
        let mut unpaid_projection = WorkBudget::new(0);
        assert_eq!(
            baseline.declared_root_source(&mut unpaid_projection),
            Err(ProjectionError::Work)
        );
        assert_eq!(unpaid_projection.used(), 0);
        assert_eq!(refused.work(), spent);
        assert_eq!(refused.status(), Status::Incomplete);
        assert_eq!(refused.coverage().acquisition.rounds, 1);
        assert_accounting(&refused, config.max_work);
    });
}

#[test]
fn initial_root_acquisition_failure_returns_error_without_any_baseline() {
    thread::scope(|scope| {
        let mut f = fixture("00");
        f.fail_code = Some(address(0x101));
        let server = Server::new(scope, f);
        let algebra = ValueAlgebra::new(ProductAlgebra::default());
        let policies = EnginePolicies::default();
        let mut extension = NoExtension;
        let result = analyze_rpc_hybrid(
            AnalysisEngine::new(&algebra, &policies, &mut extension),
            &server.input(),
            entry(),
            product_config(),
            HybridConfig::default(),
            &Control::default(),
        );
        assert!(matches!(
            result,
            Err(crate::analysis::RpcAnalysisError::Rpc(
                crate::world::rpc::RpcError::Remote { .. }
            ))
        ));
        assert_eq!(server.count("eth_getCode", address(0x101)), 1);
    });
}

#[test]
fn cumulative_state_and_transfer_limits_prevent_a_second_interpretation() {
    thread::scope(|scope| {
        let server = Server::new(scope, fixture("00"));
        let first = run(
            &server.input(),
            ExecutionConfig::default(),
            HybridConfig {
                max_detail_work: 0,
                ..Default::default()
            },
            &Control::default(),
        );
        for limit in [
            crate::analysis::Limit::States,
            crate::analysis::Limit::Transfers,
        ] {
            let mut config = ExecutionConfig::default();
            match limit {
                crate::analysis::Limit::States => {
                    config.analysis.max_states = first.coverage().acquisition.states_created
                }
                crate::analysis::Limit::Transfers => {
                    config.analysis.max_transfers = first.coverage().transfers
                }
                _ => unreachable!(),
            }
            let report = run(
                &server.input(),
                config.clone(),
                HybridConfig::default(),
                &Control::default(),
            );
            assert_eq!(
                report.status(),
                Status::Converged,
                "{:?}",
                report.coverage()
            );
            assert_eq!(
                report.coverage().precision,
                RpcPrecisionOutcome::Budget(limit)
            );
            assert_eq!(report.coverage().acquisition.rounds, 1);
            assert_eq!(
                report.coverage().acquisition.states_created,
                first.coverage().acquisition.states_created
            );
            assert_eq!(report.coverage().transfers, first.coverage().transfers);
            assert_eq!(report.coverage().precision_work, 0);
            assert_accounting(&report, config.max_work);
        }
    });
}

#[test]
fn original_payer_cancellation_is_not_replaced_by_a_different_ambient_control() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let (world, entry) = crate::analysis::machine::fixtures::fixture(&[(0x101, "00")]);
    let mut session = AnalysisSession::new(
        AnalysisEngine::new(&algebra, &policies, &mut extension),
        AnalysisInput::new(world, entry).unwrap(),
        ExecutionConfig::default(),
    )
    .unwrap();
    let owner = Control::new(Default::default());
    let original = session
        .run_hybrid_uncached(
            HybridConfig {
                max_detail_work: 0,
                ..Default::default()
            },
            StorageReadPolicy::Abstract,
            &owner,
        )
        .unwrap();
    let state = (
        session.work_used(),
        session.states_created(),
        session.transfers_used(),
        session.facts().revision(),
    );
    let ambient = Control::new(Default::default());
    ambient.cancellation().cancel();
    assert!(matches!(
        session.run_hybrid_uncached(
            HybridConfig::default(),
            StorageReadPolicy::Discover,
            &ambient,
        ),
        Err(SessionError::Cancelled(_))
    ));
    assert!(!owner.cancellation().is_cancelled());
    assert!(!session.payer_cancelled());
    assert_eq!(
        (
            session.work_used(),
            session.states_created(),
            session.transfers_used(),
            session.facts().revision()
        ),
        state
    );
    owner.cancellation().cancel();
    let unrelated = Control::new(Default::default());
    assert!(!unrelated.cancellation().is_cancelled());
    assert!(session.payer_cancelled());
    assert!(matches!(
        session.run_hybrid_uncached(
            HybridConfig::default(),
            StorageReadPolicy::Discover,
            &unrelated
        ),
        Err(SessionError::Cancelled(_))
    ));
    assert_eq!(
        (
            session.work_used(),
            session.states_created(),
            session.transfers_used(),
            session.facts().revision()
        ),
        state
    );
    assert!(original.coverage().baseline_current);
}

#[test]
fn refined_restart_refusal_retains_previous_prefix_at_its_original_revision() {
    thread::scope(|scope| {
        let server = Server::new(scope, fixture("5f545000"));
        let complete = run(
            &server.input(),
            product_config(),
            HybridConfig::default(),
            &Control::default(),
        );
        assert_eq!(complete.coverage().precision, RpcPrecisionOutcome::Current);
        // Fund the exact same rounds and final fact projection, then permit
        // only one further unit before the new process kernel. This selects a
        // real boundary, rather than inventing a different fee model/budget.
        let allowance = complete.latest().coverage().work.prior - complete.baseline().work() + 1;
        let partial = run(
            &server.input(),
            product_config(),
            HybridConfig {
                max_detail_work: allowance,
                ..Default::default()
            },
            &Control::default(),
        );
        assert_eq!(partial.status(), Status::Converged);
        assert_eq!(partial.coverage().precision, RpcPrecisionOutcome::Work);
        assert_eq!(partial.coverage().restrictions.len(), 1);
        assert!(
            partial.latest().detail().is_none(),
            "{:?}",
            partial.latest().coverage()
        );
        assert!(partial.detail_report().is_some());
        assert!(partial.local_detail().next().is_some());
        assert!(!partial.coverage().detail_at_current_revision);
        assert_ne!(
            partial.coverage().detail_revision,
            Some(partial.coverage().current_revision)
        );
        assert_eq!(
            partial.latest().baseline().revision(),
            partial.coverage().current_revision
        );
        assert!(
            partial
                .detail()
                .unwrap()
                .frontiers()
                .iter()
                .any(|f| matches!(f.reason, FrontierReason::MissingStorage { .. }))
        );
        assert!(
            partial
                .baseline()
                .baseline()
                .world()
                .account(address(0x101))
                .unwrap()
                .storage
                .is_empty()
        );
        assert_accounting(&partial, product_config().max_work);
        assert!(complete.coverage().detail_at_current_revision);
        assert!(complete.work() > partial.work());
    });
}

#[test]
fn late_implementation_error_after_committed_slot_keeps_restriction_and_old_baseline() {
    thread::scope(|scope| {
        let mut f = fixture("5f5f5f5f6009546207a120f45000");
        f.storage
            .insert((address(0x101), U256::from(9)), U256::from(0x200));
        f.code.insert(address(0x200), "00".to_owned());
        f.fail_code = Some(address(0x200));
        let server = Server::new(scope, f);
        let report = run(
            &server.input(),
            product_config(),
            HybridConfig::default(),
            &Control::default(),
        );
        assert_eq!(
            report.status(),
            Status::Converged,
            "{:?}",
            report.coverage()
        );
        assert_eq!(
            report.coverage().precision,
            RpcPrecisionOutcome::AcquisitionFailed
        );
        assert_eq!(report.coverage().restrictions.len(), 1);
        assert_eq!(
            report.coverage().baseline_relation,
            RpcBaselineRelation::InitialFactRestriction
        );
        assert_ne!(
            report.coverage().baseline_revision,
            report.coverage().current_revision
        );
        assert_eq!(report.failures()[0].failure().context.method, "eth_getCode");
        assert_eq!(
            report.failures()[0].failure().context.account,
            Some(address(0x200))
        );
        assert!(
            report
                .detail()
                .unwrap()
                .world()
                .account(address(0x200))
                .is_none()
        );
        assert!(
            report
                .baseline()
                .baseline()
                .world()
                .account(address(0x101))
                .unwrap()
                .storage
                .is_empty()
        );
        assert_eq!(server.count("eth_getCode", address(0x200)), 1);
        assert_accounting(&report, product_config().max_work);
    });
}

#[test]
fn same_proxy_at_generic_twenty_million_never_resets_or_rescues_unclosed_baseline() {
    thread::scope(|scope| {
        let mut f = fixture("5f5f5f5f6009546207a120f45000");
        f.storage
            .insert((address(0x101), U256::from(9)), U256::from(0x200));
        f.code.insert(address(0x200), "600154565b5b00".to_owned());
        f.storage
            .insert((address(0x101), U256::from(1)), U256::from(5));
        let server = Server::new(scope, f);
        let config = ExecutionConfig::default();
        let report = run(
            &server.input(),
            config.clone(),
            HybridConfig::default(),
            &Control::default(),
        );
        assert_accounting(&report, config.max_work);
        assert!(report.coverage().acquisition.states_created <= config.analysis.max_states);
        assert!(report.coverage().transfers <= config.analysis.max_transfers);
        // The frozen pre-optimization source refuses this baseline at20M.
        // A later lawful optimization may close it: only the no-rescue law,
        // declared cap and exact original accounting are persistent assertions.
        if !report.baseline().coverage().baseline_current {
            assert_eq!(report.status(), Status::Incomplete);
            assert_eq!(
                report.coverage().precision,
                RpcPrecisionOutcome::BaselinePending
            );
            assert_eq!(report.coverage().precision_work, 0);
            assert_eq!(report.coverage().acquisition.rounds, 1);
            assert!(report.coverage().acquisition.fetched_accounts.is_empty());
            assert!(report.coverage().acquisition.fetched_storage.is_empty());
            assert!(report.detail().is_none());
        }
    });
}

#[test]
fn symbolic_rpc_entry_keeps_original_namespace_and_only_conditions_paid_native_detail() {
    thread::scope(|scope| {
        let server = Server::new(scope, fixture("00"));
        let mut original = entry();
        original.environment.to = AddressInput::unknown_to();
        let values = original.environment.clone();
        let algebra = ValueAlgebra::new(ProductAlgebra::default());
        let policies = EnginePolicies::default();
        let mut extension = NoExtension;
        let report = analyze_rpc_hybrid(
            AnalysisEngine::new(&algebra, &policies, &mut extension),
            &server.input(),
            original,
            product_config(),
            HybridConfig::default(),
            &Control::default(),
        )
        .unwrap();
        let baseline = report.baseline().baseline();
        assert_eq!(
            report.status(),
            Status::Converged,
            "{:?}",
            report.coverage()
        );
        assert_eq!(
            baseline.root_binding(),
            RootInputBinding::Namespace {
                coordinate: address(0x101)
            }
        );
        assert!(matches!(
            baseline.entry().environment.to,
            AddressInput::Symbolic(_)
        ));
        assert_eq!(baseline.entry().environment.caller, values.caller);
        assert_eq!(baseline.entry().environment.value, values.value);
        assert_eq!(baseline.entry().environment.calldata, values.calldata);
        assert_eq!(
            declared_source(baseline),
            ProcessSource::InputProgram {
                hash: alloy_primitives::keccak256([0]),
                fork: crate::Fork::Osaka,
            }
        );
        assert!(
            baseline
                .outcomes()
                .iter()
                .any(|outcome| outcome.kind == crate::analysis::OutcomeKind::Failure)
        );
        assert!(
            baseline
                .outcomes()
                .iter()
                .all(|outcome| outcome.store.code(address(0x101)).is_none()),
            "public Process Stores, including F1, cannot publish the private template as physical code"
        );
        let detail_report = report.detail_report().expect("paid native precision");
        let detail = detail_report.detail().unwrap();
        assert_eq!(
            detail_report.coverage().detail_binding,
            HybridNativeBinding::SymbolicOwnerNamespace {
                namespace: address(0x101)
            }
        );
        let region = detail_report
            .coverage()
            .detail_regions
            .first()
            .expect("selected supplied program");
        assert_eq!(region.source, declared_source(baseline));
        let region_json = serde_json::to_value(region).unwrap();
        assert_eq!(
            region_json["source"],
            serde_json::to_value(region.source).unwrap()
        );
        assert!(region_json.get("code_address").is_none());
        let mut conditional = baseline.entry().clone();
        conditional.environment.to = address(0x101).into();
        assert_eq!(detail.entry(), &conditional);
        assert_eq!(
            detail.root_binding(),
            RootInputBinding::Physical {
                address: address(0x101)
            }
        );
        assert_ne!(
            detail_report.coverage().chosen,
            HybridInterpretation::Native
        );
        assert!(std::ptr::eq(report.chosen(), baseline));
        assert!(detail_report.coverage().work.preparation > 0);
        assert!(report.coverage().detail_at_current_revision);
        let sites = report.local_detail().collect::<Vec<_>>();
        assert!(!sites.is_empty());
        assert!(sites.iter().all(|site| site.scope
            == HybridLocalScope::NativeOwnerNamespacePrefix
            && site.binding
                == HybridNativeBinding::SymbolicOwnerNamespace {
                    namespace: address(0x101)
                }));
        assert!(report.coverage().restrictions.is_empty());
        assert_eq!(server.count("eth_getCode", address(0x101)), 1);
        assert_accounting(&report, product_config().max_work);
    });
}
