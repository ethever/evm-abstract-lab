//! Namespace input, physical witnesses and conditional detail compose on core15.

use super::*;
use evm_abstract::{
    U256,
    analysis::{FrameCode, OutcomeKind, ProcessAction, ProcessSource, ProjectionError},
    resource::WorkBudget,
    world::{Code, EvmAccount, RootInputBinding},
};

fn analyze(
    world: EvmWorld<NumericValue>,
    entry: EvmEntry<NumericValue>,
    work: usize,
    allowance: usize,
    regions: usize,
) -> HybridAnalysis<NumericValue> {
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let original = entry.clone();
    let result = AnalysisEngine::new(&native::product(), &policies, &mut extension)
        .run_hybrid(
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig {
                max_work: work,
                root_terminal_coverage: false,
                ..ExecutionConfig::default()
            },
            HybridConfig {
                max_detail_work: allowance,
                max_detail_regions: regions,
            },
            &Control::default(),
        )
        .unwrap();
    assert_eq!(result.baseline().entry(), &original);
    assert_accounting(&result, work);
    result
}

#[test]
fn zero_state_input_source_is_paid_declared_provenance_without_execution() {
    for coordinate in [A, 3] {
        for physical in [false, true] {
            let mut world = new_world();
            insert(&mut world, coordinate, &branch(), &[(0, 5)]);
            let mut entry = unknown();
            entry.address = address(coordinate);
            entry.environment.to = if physical {
                address(coordinate).into()
            } else {
                AddressInput::unknown_to()
            };
            let expected = entry.binding();
            let report = analyze(world, entry, 1, usize::MAX, 1);
            assert_eq!(report.status(), Status::Incomplete);
            assert!(report.baseline().states().is_empty());
            assert!(report.baseline().procedure_summaries().is_empty());
            assert!(report.detail().is_none());
            assert_eq!(report.local_detail().count(), 0);
            assert_eq!(report.baseline().root_binding(), expected);
            assert_eq!(
                serde_json::to_value(report.baseline().entry()).unwrap()["binding"],
                serde_json::to_value(expected).unwrap()
            );
            assert_eq!(
                report
                    .baseline()
                    .declared_root_source(&mut WorkBudget::new(0)),
                Err(ProjectionError::Work)
            );
            let mut projection = WorkBudget::new(100_000);
            let source = report
                .baseline()
                .declared_root_source(&mut projection)
                .unwrap();
            assert!(projection.used() > 0);
            if physical && coordinate == 3 {
                assert!(
                    matches!(source, ProcessSource::Precompile { address: a, .. }
                    if a == address(3))
                );
            } else if physical {
                assert!(matches!(source, ProcessSource::Runtime { source: a, .. }
                    if a == address(coordinate)));
            } else {
                assert!(matches!(source, ProcessSource::InputProgram { hash, .. }
                    if hash == alloy_primitives::keccak256(&branch().bytes)));
            }
            for cancel_owner in [false, true] {
                let owner = analysis_control::Cancellation::new();
                let ambient = analysis_control::Cancellation::new();
                let mut projection =
                    analysis_control::with_cancellation(&owner, || WorkBudget::new(100_000));
                if cancel_owner {
                    owner.cancel();
                } else {
                    ambient.cancel();
                }
                let result = analysis_control::with_cancellation(&ambient, || {
                    report.baseline().declared_root_source(&mut projection)
                });
                assert_eq!(result, Err(ProjectionError::Cancelled));
                assert_eq!(projection.used(), 0);
            }
        }
    }
}

#[test]
fn chosen_namespace_model_covers_distinct_physical_code_at_equal_numeric_coordinate() {
    let mut p = Assembly::default();
    p.call(opcode::CALL, A as u16, 0, (0, 0), (0, 0))
        .return_top();
    let p = p.finish();
    let mut q = Assembly::default();
    q.store(0, 9).op(opcode::STOP);
    let q = q.finish();
    let mut templates = new_world();
    insert(&mut templates, A, &p, &[(0, 5)]);
    insert(&mut templates, U, &p, &[(0, 5)]);
    let mut physical = new_world();
    insert(&mut physical, U, &p, &[(0, 5)]);
    insert(&mut physical, A, &q, &[(0, 5)]);
    let concrete = EvmEntry::concrete(
        address(U),
        address(support::USER),
        support::word(0),
        EvmByteArray::empty(),
    );
    let witness = oracle::execute_with_gas(&physical, &concrete, 400_000);
    assert_eq!(witness.slot(A, 0), U256::from(9));
    assert_eq!(witness.slot(U, 0), U256::from(5));
    let mut input = unknown();
    input.environment.to = AddressInput::unknown_to();
    let report = analyze(templates, input, 2_000_000_000, 2_000_000, 1);
    assert!(report.coverage().baseline_current);
    assert_ne!(report.coverage().chosen, HybridInterpretation::Native);
    witness.covers(report.chosen());
    let mut callback = false;
    let mut physical_complement = false;
    for record in report.baseline().procedure_summaries() {
        assert!(record.closed && record.source_current);
        for call in &record.regional_cfg.calls {
            assert_ne!(call.target, Some(ProcessOwner::Named(address(A))));
            let callee = &record.procedures[call.callee];
            let key = callee.frame.process.as_ref().unwrap();
            if call.target == Some(ProcessOwner::RootNamespace) {
                callback |= matches!(key.source, ProcessSource::InputProgram { hash, .. }
                    if hash == alloy_primitives::keccak256(&p.bytes))
                    && call.deliveries.iter().any(|d| d.current);
            }
            if call.target == Some(ProcessOwner::Residual) {
                physical_complement |= matches!(key.source, ProcessSource::AnyRuntime { .. })
                    && callee
                        .process_input
                        .as_ref()
                        .unwrap()
                        .address
                        .contains(U256::from(A));
            }
        }
    }
    assert!(callback && physical_complement);
    assert!(
        report
            .chosen()
            .outcomes()
            .iter()
            .all(|o| o.store.code(address(A)).is_none())
    );
    assert!(matches!(
        report.coverage().detail_regions[0].source,
        ProcessSource::InputProgram { .. }
    ));
}

#[test]
fn delegated_namespace_and_physical_roots_retain_real_source_owner_and_static_mode() {
    let implementation = support::C;
    let mut p = Assembly::default();
    p.call(opcode::STATICCALL, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .call(opcode::DELEGATECALL, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .op(opcode::STOP);
    let p = p.finish();
    for physical in [false, true] {
        let mut world = new_world();
        world
            .insert(
                address(A),
                EvmAccount {
                    code: Code::Delegation(address(implementation)),
                    balance: support::word(1_000_000),
                    ..EvmAccount::empty()
                },
            )
            .unwrap();
        insert(&mut world, implementation, &p, &[]);
        let mut stop = Assembly::default();
        stop.op(opcode::STOP);
        insert(&mut world, U, &stop.finish(), &[]);
        let mut entry = unknown();
        if !physical {
            entry.environment.to = AddressInput::unknown_to();
        }
        let report = analyze(world, entry, 2_000_000_000, 100_000_000, 2);
        assert!(report.coverage().baseline_current && report.coverage().detail_current);
        let expected = ProcessSource::Runtime {
            source: address(implementation),
            hash: alloy_primitives::keccak256(&p.bytes),
            fork: evm_abstract::Fork::Osaka,
        };
        assert_eq!(report.coverage().detail_regions[0].source, expected);
        let owner = if physical {
            ProcessOwner::Named(address(A))
        } else {
            ProcessOwner::RootNamespace
        };
        for record in report.baseline().procedure_summaries() {
            let root = &record.procedures[record.root_procedure.unwrap()];
            let key = root.frame.process.as_ref().unwrap();
            assert_eq!(key.source, expected);
            assert_eq!(key.owner, owner);
        }
        assert!(
            report
                .local_detail()
                .any(|p| p.frame.code_address == address(U)
                    && p.frame.address == address(U)
                    && p.frame.is_static)
        );
        assert!(
            report
                .local_detail()
                .any(|p| p.frame.code_address == address(U)
                    && p.frame.address == address(A)
                    && !p.frame.is_static)
        );
        assert_eq!(
            report.coverage().chosen == HybridInterpretation::Native,
            physical
        );
    }
}

#[test]
fn unresolved_input_delegation_keeps_marker_provenance_without_invented_runtime_pc() {
    let mut world = new_world();
    world
        .insert(
            address(A),
            EvmAccount {
                code: Code::Delegation(address(A)),
                balance: support::word(1_000_000),
                ..EvmAccount::empty()
            },
        )
        .unwrap();
    let marker_hash = alloy_primitives::keccak256(world.raw_account_code(address(A)).unwrap());
    let mut stop = Assembly::default();
    stop.op(opcode::STOP);
    insert(&mut world, U, &stop.finish(), &[]);
    let mut input = unknown();
    input.environment.to = AddressInput::unknown_to();
    let report = analyze(world, input, 2_000_000_000, 2_000_000, 2);
    assert!(report.coverage().baseline_current);
    let declared = ProcessSource::InputDelegation {
        target: address(A),
        hash: marker_hash,
        fork: evm_abstract::Fork::Osaka,
    };
    assert_eq!(
        report
            .baseline()
            .declared_root_source(&mut WorkBudget::new(100_000))
            .unwrap(),
        declared
    );
    for record in report.baseline().procedure_summaries() {
        let root = &record.procedures[record.root_procedure.unwrap()];
        assert_eq!(root.frame.mode, FrameCode::Unknown);
        assert_eq!(root.frame.process.as_ref().unwrap().source, declared);
    }
    assert_eq!(report.coverage().detail_regions.len(), 1);
    assert!(matches!(report.coverage().detail_regions[0].source,
        ProcessSource::Runtime { source, .. } if source == address(U)));
    assert!(
        report
            .local_detail()
            .all(|p| p.frame.code_address != address(A))
    );
    assert!(
        report
            .detail()
            .unwrap()
            .states()
            .iter()
            .all(|s| s.executed_pcs.is_empty())
    );
    assert_eq!(report.coverage().chosen, HybridInterpretation::Process);
}

#[test]
fn conditional_precompile_coordinate_never_turns_supplied_runtime_into_physical_region() {
    let mut world = new_world();
    insert(&mut world, 3, &branch(), &[(0, 5)]);
    insert(&mut world, U, &branch(), &[]);
    let mut input = unknown();
    input.address = address(3);
    input.environment.to = AddressInput::unknown_to();
    let report = analyze(world, input, 100_000_000, 20_000_000, 2);
    assert!(report.coverage().baseline_current);
    assert_eq!(
        report.baseline().root_binding(),
        RootInputBinding::Namespace {
            coordinate: address(3)
        }
    );
    assert!(matches!(
        report
            .baseline()
            .declared_root_source(&mut WorkBudget::new(100_000))
            .unwrap(),
        ProcessSource::InputProgram { .. }
    ));
    assert_eq!(report.coverage().detail_regions.len(), 1);
    assert!(matches!(report.coverage().detail_regions[0].source,
        ProcessSource::Runtime { source, .. } if source == address(U)));
    assert_eq!(
        report.detail().unwrap().root_binding(),
        RootInputBinding::Physical {
            address: address(3)
        }
    );
    assert!(
        report
            .local_detail()
            .all(|p| p.frame.code_address != address(3))
    );
    assert_ne!(report.coverage().chosen, HybridInterpretation::Native);
}

#[test]
fn original_unknown_root_creation_has_closed_constructor_and_delivery_obligations() {
    let mut factory = Assembly::default();
    factory
        .push(0)
        .push(0)
        .push(0)
        .op(opcode::CREATE)
        .op(opcode::POP)
        .push(0)
        .push(0)
        .push(0)
        .push(0)
        .op(opcode::CREATE2)
        .op(opcode::POP)
        .op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &factory.finish(), &[(0, 5)]);
    let mut input = unknown();
    input.environment.to = AddressInput::unknown_to();
    let report = analyze(world, input, 2_000_000_000, 2_000_000, 1);
    assert!(report.coverage().baseline_current);
    let mut actions = std::collections::BTreeSet::new();
    let mut constructor = false;
    for record in report.baseline().procedure_summaries() {
        assert!(record.closed && record.source_current);
        for node in &record.regional_cfg.nodes {
            let receipt = node.transfer.as_ref().unwrap();
            assert!(receipt.current && receipt.complete && !receipt.visited_block);
            if let Some(process) = &receipt.process {
                for creation in &process.creations {
                    actions.insert(creation.action);
                }
            }
        }
        for call in &record.regional_cfg.calls {
            constructor |= record.procedures[call.callee].semantics
                == TransferSemantics::ConstructorProcess
                && call
                    .deliveries
                    .iter()
                    .any(|d| d.current && d.kind == OutcomeKind::Return);
        }
    }
    assert!(actions.contains(&ProcessAction::Create) && actions.contains(&ProcessAction::Create2));
    assert!(constructor);
}
