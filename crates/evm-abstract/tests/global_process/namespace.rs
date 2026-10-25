//! Installed namespace/source, reciprocal alias and checkpoint witnesses.

use super::support::{A, Assembly, C, U, USER, V, address, entry, insert, new_world, read, word};
use super::{evidence, fixture, oracle, run_input};
use alloy_primitives::{U256, keccak256};
use evm_abstract::{
    analysis::{OutcomeKind, ProcessAction, ProcessOwner, ProcessSource, Status},
    domain::NumericValue,
    world::{AddressInput, Code, EvmByteArray, EvmEntry, EvmWorld},
};
use revm_bytecode::opcode;

fn symbolic(
    world: EvmWorld<NumericValue>,
) -> evm_abstract::analysis::EvmWorldAnalysis<NumericValue> {
    let mut input = entry(EvmByteArray::empty());
    input.environment.to = AddressInput::unknown_to();
    run_input(world, input, 1_000_000_000)
}

fn concrete_entry() -> EvmEntry<NumericValue> {
    EvmEntry::concrete(address(U), address(USER), word(0), EvmByteArray::empty())
}

fn assert_current(analysis: &evm_abstract::analysis::EvmWorldAnalysis<NumericValue>) {
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis
            .frontiers()
            .iter()
            .map(|f| &f.reason)
            .collect::<Vec<_>>()
    );
    assert!(analysis.frontiers().is_empty());
    assert!(!analysis.procedure_summaries().is_empty());
    for record in analysis.procedure_summaries() {
        assert!(record.closed && record.source_current);
        for node in &record.regional_cfg.nodes {
            let transfer = node.transfer.as_ref().expect("installed Body transfer");
            assert!(transfer.current && transfer.complete && !transfer.visited_block);
        }
        for call in &record.regional_cfg.calls {
            assert!(!call.deliveries.is_empty());
            assert!(call.deliveries.iter().all(|returned| returned.current));
        }
    }
}

#[test]
fn namespace_input_source_and_physical_names_are_distinct_on_installed_model() {
    let mut a = Assembly::default();
    a.store(0, 9).op(opcode::STOP);
    let program = a.finish();
    let mut world = new_world();
    insert(&mut world, A, &program, &[(0, 5)]);
    insert(&mut world, U, &program, &[(0, 5)]);
    let concrete = oracle::execute_with_gas(&world, &concrete_entry(), 100_000);
    let analysis = symbolic(world);
    assert_current(&analysis);
    concrete.covers(&analysis);
    for record in analysis.procedure_summaries() {
        let root = &record.procedures[record.root_procedure.unwrap()];
        let key = root.frame.process.as_ref().unwrap();
        assert_eq!(key.owner, ProcessOwner::RootNamespace);
        assert_eq!(
            key.source,
            ProcessSource::InputProgram {
                hash: keccak256(&program.bytes),
                fork: evm_abstract::Fork::Osaka,
            }
        );
        let frame = serde_json::to_value(&root.frame).unwrap();
        assert_eq!(frame.as_object().unwrap().len(), 1);
        assert_eq!(frame["process"]["owner"], "RootNamespace");
        assert!(!frame.to_string().contains(&address(A).to_string()));
        assert!(
            root.input.code(address(A)).is_none(),
            "no physical code fact at namespace bits"
        );
        for output in &root.outputs {
            assert!(output.store.code(address(A)).is_none());
        }
    }
    assert!(
        analysis
            .outcomes()
            .iter()
            .all(|o| o.store.code(address(A)).is_none())
    );
    evidence("namespace-input-source-physical-partition", &analysis);
}

#[test]
fn literal_namespace_physical_target_has_its_own_opaque_control_and_effect() {
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
    let witness = oracle::execute_with_gas(&physical, &concrete_entry(), 400_000);
    assert_eq!(witness.kind, OutcomeKind::Return);
    assert_eq!(witness.output_word(0), U256::ONE);
    assert_eq!(witness.slot(A, 0), U256::from(9));
    assert_eq!(witness.slot(U, 0), U256::from(5));
    assert!(witness.steps.iter().any(|s| s.code == address(A)
        && s.owner == address(A)
        && s.caller == address(U)
        && s.opcode == opcode::SSTORE));
    let analysis = symbolic(templates);
    assert_current(&analysis);
    witness.covers(&analysis);
    let mut root_callback = false;
    let mut physical_complement = false;
    for record in analysis.procedure_summaries() {
        for call in &record.regional_cfg.calls {
            assert_ne!(call.target, Some(ProcessOwner::Named(address(A))));
            if call.action != Some(ProcessAction::Call) {
                continue;
            }
            let callee = &record.procedures[call.callee];
            let key = callee.frame.process.as_ref().unwrap();
            if call.target == Some(ProcessOwner::RootNamespace) {
                root_callback |= matches!(key.source, ProcessSource::InputProgram { hash, .. }
                    if hash == keccak256(&p.bytes));
            }
            if call.target == Some(ProcessOwner::Residual) {
                let invocation = callee.process_input.as_ref().unwrap();
                physical_complement |= matches!(key.source, ProcessSource::AnyRuntime { .. })
                    && invocation.address.contains(U256::from(A));
            }
        }
    }
    assert!(
        root_callback && physical_complement,
        "callback P and physical A/Q are separate alternatives"
    );
    assert!(
        analysis
            .outcomes()
            .iter()
            .all(|o| o.store.code(address(A)).is_none())
    );
    evidence("literal-namespace-P-versus-physical-Q", &analysis);
}

#[test]
fn reciprocal_named_callback_write_read_and_joint_mutable_planes_are_covered() {
    let mut p = Assembly::default();
    p.input(0).op(opcode::ISZERO).jump("outer", true);
    p.store(0, 11)
        .push(11)
        .push(0)
        .op(opcode::TSTORE)
        .op(opcode::STOP);
    p.label("outer")
        .store(0, 9)
        .push(1)
        .mstore(0)
        .call(opcode::CALL, U as u16, 0, (0, 32), (0, 0))
        .op(opcode::POP)
        .push(0)
        .op(opcode::SLOAD)
        .return_top();
    let p = p.finish();
    let mut world = new_world();
    insert(&mut world, A, &p, &[(0, 5)]);
    insert(&mut world, U, &p, &[(0, 5)]);
    let witness = oracle::execute_with_gas(&world, &concrete_entry(), 500_000);
    assert_eq!(witness.output_word(0), U256::from(11));
    assert_eq!(witness.slot(U, 0), U256::from(11));
    let analysis = symbolic(world);
    assert_current(&analysis);
    witness.covers(&analysis);
    let mut reciprocal_delivery = false;
    for record in analysis.procedure_summaries() {
        for call in &record.regional_cfg.calls {
            if call.action == Some(ProcessAction::Call)
                && call.target == Some(ProcessOwner::Named(address(U)))
                && record.procedures[record.regional_cfg.nodes[call.from].procedure]
                    .frame
                    .process
                    .as_ref()
                    .unwrap()
                    .owner
                    == ProcessOwner::RootNamespace
            {
                let callee = &record.procedures[call.callee];
                assert!(matches!(callee.frame.process.as_ref().unwrap().source,
                    ProcessSource::Runtime { source, .. } if source == address(U)));
                for output in callee
                    .outputs
                    .iter()
                    .filter(|o| o.kind == OutcomeKind::Return)
                {
                    let store = output
                        .store
                        .instantiate(super::native::product().borrowed())
                        .unwrap();
                    // These facts must coexist in one returned Store, before
                    // transaction finalization clears transient state.
                    assert!(read(&store, U, 0).contains(U256::from(11)));
                    assert!(
                        store
                            .read_transient(
                                address(U),
                                &word(0),
                                super::native::product().borrowed()
                            )
                            .contains(U256::from(11))
                    );
                    assert!(store.nonce(address(U)).contains(U256::ZERO));
                    assert_eq!(store.created_in_transaction(address(U)), Some(false));
                    assert_eq!(store.pending_destruction(address(U)), Some(false));
                    assert!(matches!(store.code(address(U)), Some(Code::Runtime(_))));
                }
                reciprocal_delivery |= call
                    .deliveries
                    .iter()
                    .any(|ret| ret.current && ret.kind == OutcomeKind::Return);
            }
        }
    }
    assert!(
        reciprocal_delivery,
        "a current child return must actually resume the namespace caller"
    );
    evidence("reciprocal-named-callback-read11", &analysis);
}

#[test]
fn symbolic_alias_child_and_outer_rollback_cover_all_call_owners() {
    for op in [opcode::CALL, opcode::DELEGATECALL, opcode::CALLCODE] {
        for outer_reverts in [false, true] {
            let mut parent = Assembly::default();
            parent
                .store(0, 9)
                .call(op, C as u16, 0, (0, 0), (0, 0))
                .op(opcode::POP);
            if outer_reverts {
                parent.push(0).push(0).op(opcode::REVERT);
            } else {
                parent.push(0).op(opcode::SLOAD).return_top();
            }
            let parent = parent.finish();
            let mut child = Assembly::default();
            child
                .store(0, 11)
                .push(11)
                .push(0)
                .op(opcode::TSTORE)
                .push(0)
                .push(0)
                .op(opcode::REVERT);
            let mut world = new_world();
            insert(&mut world, A, &parent, &[(0, 5)]);
            insert(&mut world, U, &parent, &[(0, 5)]);
            insert(&mut world, C, &child.finish(), &[(0, 5)]);
            let witness = oracle::execute_with_gas(&world, &concrete_entry(), 500_000);
            assert_eq!(
                witness.kind,
                if outer_reverts {
                    OutcomeKind::Revert
                } else {
                    OutcomeKind::Return
                }
            );
            assert_eq!(
                witness.slot(U, 0),
                U256::from(if outer_reverts { 5 } else { 9 })
            );
            assert_eq!(witness.slot(C, 0), U256::from(5));
            let analysis = symbolic(world);
            assert_current(&analysis);
            witness.covers(&analysis);
            if outer_reverts {
                for output in analysis
                    .outcomes()
                    .iter()
                    .filter(|o| o.kind == OutcomeKind::Revert)
                {
                    assert_eq!(read(&output.store, U, 0).singleton(), Some(U256::from(5)));
                    assert_eq!(read(&output.store, C, 0).singleton(), Some(U256::from(5)));
                    assert_eq!(output.store.nonce(address(U)).singleton(), Some(U256::ZERO));
                    assert_eq!(output.store.created_in_transaction(address(U)), Some(false));
                    assert_eq!(output.store.pending_destruction(address(U)), Some(false));
                }
            }
            let expected = if op == opcode::CALL {
                ProcessOwner::Named(address(C))
            } else {
                ProcessOwner::RootNamespace
            };
            assert!(analysis.procedure_summaries().iter().any(|r| {
                r.regional_cfg.calls.iter().any(|call| {
                    call.target == Some(ProcessOwner::Named(address(C)))
                        && r.procedures[call.callee]
                            .frame
                            .process
                            .as_ref()
                            .unwrap()
                            .owner
                            == expected
                })
            }));
        }
    }
}

#[test]
fn symbolic_alias_static_modes_preserve_state_and_prohibit_child_writes() {
    for op in [
        opcode::CALL,
        opcode::STATICCALL,
        opcode::DELEGATECALL,
        opcode::CALLCODE,
    ] {
        let mut parent = Assembly::default();
        parent
            .call(
                op,
                C as u16,
                if op == opcode::CALLCODE { 9 } else { 0 },
                (0, 0),
                (0, 0),
            )
            .return_top();
        let parent = parent.finish();
        let mut child = Assembly::default();
        child.store(0, 11).op(opcode::STOP);
        let mut world = new_world();
        insert(&mut world, A, &parent, &[(0, 5)]);
        insert(&mut world, U, &parent, &[(0, 5)]);
        insert(&mut world, C, &child.finish(), &[(0, 5)]);
        let mut wrapper = Assembly::default();
        wrapper
            .call(opcode::STATICCALL, U as u16, 0, (0, 0), (0, 32))
            .op(opcode::POP)
            .return_range(0, 32);
        insert(&mut world, V, &wrapper.finish(), &[]);
        let concrete =
            EvmEntry::concrete(address(V), address(USER), word(0), EvmByteArray::empty());
        let witness = oracle::execute_with_gas(&world, &concrete, 500_000);
        assert_eq!(witness.kind, OutcomeKind::Return);
        assert_eq!(witness.output_word(0), U256::ZERO);
        assert!(witness.steps.iter().any(|s| s.code == address(C)
            && s.is_static
            && s.opcode == opcode::SSTORE
            && (op != opcode::CALLCODE || s.value == U256::from(9))));
        let mut input = entry(EvmByteArray::empty());
        input.environment.caller = AddressInput::unknown_caller();
        input.environment.to = AddressInput::unknown_to();
        input.environment.is_static = true;
        let analysis = run_input(world, input, 2_000_000_000);
        eprintln!(
            "NAMESPACE_STATIC_JSON {}",
            serde_json::json!({"opcode":op,
            "status":analysis.status(), "work":analysis.work(),
            "bodies":analysis.procedure_summaries().iter().map(|r|r.regional_cfg.nodes.len()).sum::<usize>(),
            "frontiers":analysis.frontiers().iter().map(|f|format!("{:?}",f.reason)).collect::<Vec<_>>() })
        );
        assert_current(&analysis);
        witness.covers(&analysis);
        assert!(
            analysis
                .outcomes()
                .iter()
                .any(|o| o.kind == OutcomeKind::Return)
        );
        for outcome in analysis.outcomes() {
            assert_eq!(read(&outcome.store, U, 0).singleton(), Some(U256::from(5)));
            assert_eq!(read(&outcome.store, C, 0).singleton(), Some(U256::from(5)));
            assert_eq!(
                outcome.store.nonce(address(U)).singleton(),
                Some(U256::ZERO)
            );
            assert_eq!(
                outcome.store.read_balance(address(U)).singleton(),
                Some(U256::from(1_000_000))
            );
        }
        assert!(
            analysis
                .procedure_summaries()
                .iter()
                .any(|r| r
                    .procedures
                    .iter()
                    .any(|p| p.frame.process.as_ref().unwrap().owner
                        == ProcessOwner::RootNamespace
                        && p.frame.is_static))
        );
    }
}

#[test]
fn symbolic_creator_nonce_and_lifecycle_cover_creation_and_outer_rollback() {
    for action in [opcode::CREATE, opcode::CREATE2] {
        for outer_reverts in [false, true] {
            let mut bytes = vec![];
            if action == opcode::CREATE2 {
                bytes.push(opcode::PUSH0);
            }
            bytes.extend([
                opcode::PUSH0,
                opcode::PUSH0,
                opcode::PUSH0,
                action,
                opcode::POP,
            ]);
            if outer_reverts {
                bytes.extend([opcode::PUSH0, opcode::PUSH0, opcode::REVERT]);
            } else {
                bytes.push(opcode::STOP);
            }
            let program = fixture(&bytes);
            let mut world = new_world();
            insert(&mut world, A, &program, &[(0, 5)]);
            insert(&mut world, U, &program, &[(0, 5)]);
            let witness = oracle::execute_with_gas(&world, &concrete_entry(), 500_000);
            let analysis = symbolic(world);
            assert_current(&analysis);
            witness.covers(&analysis);
            assert!(
                analysis
                    .outcomes()
                    .iter()
                    .filter(|o| o.kind == witness.kind)
                    .any(|o| o
                        .store
                        .nonce(address(U))
                        .contains(U256::from(u64::from(!outer_reverts)))
                        && o.store.created_in_transaction(address(U)) == Some(false)
                        && o.store.pending_destruction(address(U)) == Some(false))
            );
            assert!(analysis.procedure_summaries().iter().any(|r| {
                r.regional_cfg.calls.iter().any(|call| {
                    matches!(
                        call.action,
                        Some(ProcessAction::Create | ProcessAction::Create2)
                    ) && r.procedures[r.regional_cfg.nodes[call.from].procedure]
                        .frame
                        .process
                        .as_ref()
                        .unwrap()
                        .owner
                        == ProcessOwner::RootNamespace
                        && matches!(
                            r.procedures[call.callee]
                                .frame
                                .process
                                .as_ref()
                                .unwrap()
                                .source,
                            ProcessSource::AnyInitCode { .. }
                        )
                })
            }));
        }
    }
}

#[test]
fn private_namespace_bits_do_not_select_a_physical_precompile_for_the_input_program() {
    let mut p = Assembly::default();
    p.store(0, 9).op(opcode::STOP);
    let p = p.finish();
    let mut world = new_world();
    insert(&mut world, 3, &p, &[(0, 5)]);
    insert(&mut world, U, &p, &[(0, 5)]);
    let witness = oracle::execute_with_gas(&world, &concrete_entry(), 100_000);
    assert_eq!(witness.slot(U, 0), U256::from(9));
    let mut input = EvmEntry::concrete(address(3), address(USER), word(0), EvmByteArray::empty());
    input.environment.to = AddressInput::unknown_to();
    let analysis = run_input(world, input, 100_000_000);
    assert_current(&analysis);
    witness.covers(&analysis);
    for record in analysis.procedure_summaries() {
        let root = &record.procedures[record.root_procedure.unwrap()];
        assert_eq!(root.frame.mode, evm_abstract::analysis::FrameCode::Runtime);
        assert!(
            matches!(root.frame.process.as_ref().unwrap().source, ProcessSource::InputProgram { hash, .. }
            if hash == keccak256(&p.bytes))
        );
    }
}

#[test]
fn namespace_delegation_to_equal_physical_bits_does_not_resolve_the_template_again() {
    let mut q = Assembly::default();
    q.store(0, 9).op(opcode::STOP);
    let q = q.finish();
    let marker = evm_abstract::world::EvmAccount {
        code: Code::Delegation(address(A)),
        balance: word(1_000_000),
        storage: [(U256::ZERO, word(5))].into(),
        storage_unknown: false,
        ..evm_abstract::world::EvmAccount::empty()
    };
    let mut template = new_world();
    template.insert(address(A), marker.clone()).unwrap();
    template.insert(address(U), marker.clone()).unwrap();
    let mut physical = new_world();
    insert(&mut physical, A, &q, &[(0, 5)]);
    physical.insert(address(U), marker).unwrap();
    let witness = oracle::execute_with_gas(&physical, &concrete_entry(), 100_000);
    assert_eq!(witness.kind, OutcomeKind::Return);
    assert_eq!(witness.slot(U, 0), U256::from(9));
    let analysis = symbolic(template);
    assert_current(&analysis);
    witness.covers(&analysis);
    for record in analysis.procedure_summaries() {
        let root = &record.procedures[record.root_procedure.unwrap()];
        assert_eq!(root.frame.mode, evm_abstract::analysis::FrameCode::Unknown);
        assert!(matches!(root.frame.process.as_ref().unwrap().source,
            ProcessSource::InputDelegation { target, .. } if target == address(A)));
    }
}

#[test]
fn outer_input_binding_and_declared_source_exist_without_admitted_process_frames() {
    use evm_abstract::{analysis::FrameCode, resource::WorkBudget, world::RootInputBinding};
    let p = fixture(&[opcode::STOP]);
    for coordinate in [A, 3] {
        for concrete in [false, true] {
            let mut world = new_world();
            insert(&mut world, coordinate, &p, &[(0, 5)]);
            let mut input = EvmEntry::concrete(
                address(coordinate),
                address(USER),
                word(0),
                EvmByteArray::empty(),
            );
            if !concrete {
                input.environment.to = AddressInput::unknown_to();
            }
            let analysis = run_input(world, input, 1);
            assert_eq!(analysis.status(), Status::Incomplete);
            assert!(analysis.states().is_empty());
            assert!(analysis.procedure_summaries().is_empty());
            let expected = if concrete {
                RootInputBinding::Physical {
                    address: address(coordinate),
                }
            } else {
                RootInputBinding::Namespace {
                    coordinate: address(coordinate),
                }
            };
            assert_eq!(analysis.root_binding(), expected);
            let json = serde_json::to_value(analysis.entry()).unwrap();
            assert_eq!(json["binding"], serde_json::to_value(expected).unwrap());
            let mut projection = WorkBudget::new(100_000);
            let source = analysis.declared_root_source(&mut projection).unwrap();
            if concrete && coordinate == 3 {
                assert_eq!(
                    source,
                    ProcessSource::Precompile {
                        address: address(3),
                        fork: evm_abstract::Fork::Osaka
                    }
                );
                assert_eq!(
                    analysis.root_code(&mut projection).unwrap(),
                    FrameCode::Precompile(address(3))
                );
            } else {
                assert_eq!(
                    analysis.root_code(&mut projection).unwrap(),
                    FrameCode::Runtime
                );
                if concrete {
                    assert!(
                        matches!(source, ProcessSource::Runtime { source, .. } if source == address(coordinate))
                    );
                } else {
                    assert!(
                        matches!(source, ProcessSource::InputProgram { hash, .. } if hash == keccak256(&p.bytes))
                    );
                }
            }
            assert!(
                analysis
                    .declared_root_source(&mut WorkBudget::new(0))
                    .is_err()
            );
        }
    }
}

#[test]
fn symbolic_root_keeps_a_genuinely_observed_delegated_runtime_source() {
    let p = fixture(&[
        opcode::PUSH1,
        9,
        opcode::PUSH0,
        opcode::SSTORE,
        opcode::STOP,
    ]);
    let marker = evm_abstract::world::EvmAccount {
        code: Code::Delegation(address(C)),
        balance: word(1_000_000),
        storage: [(U256::ZERO, word(5))].into(),
        storage_unknown: false,
        ..evm_abstract::world::EvmAccount::empty()
    };
    let mut world = new_world();
    world.insert(address(A), marker.clone()).unwrap();
    world.insert(address(U), marker).unwrap();
    insert(&mut world, C, &p, &[(0, 5)]);
    let witness = oracle::execute_with_gas(&world, &concrete_entry(), 100_000);
    assert_eq!(witness.slot(U, 0), U256::from(9));
    let analysis = symbolic(world);
    assert_current(&analysis);
    witness.covers(&analysis);
    let expected = ProcessSource::Runtime {
        source: address(C),
        hash: keccak256(&p.bytes),
        fork: evm_abstract::Fork::Osaka,
    };
    assert_eq!(
        analysis
            .declared_root_source(&mut evm_abstract::resource::WorkBudget::new(100_000))
            .unwrap(),
        expected
    );
    for record in analysis.procedure_summaries() {
        let root = &record.procedures[record.root_procedure.unwrap()];
        assert_eq!(
            root.frame.process.as_ref().unwrap().owner,
            ProcessOwner::RootNamespace
        );
        assert_eq!(root.frame.process.as_ref().unwrap().source, expected);
    }
}
