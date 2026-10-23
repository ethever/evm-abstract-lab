//! Concrete call operands versus actual native process subscriptions.

use super::support::{A, Assembly, C, U, V, address, entry, insert, new_world, word};
use super::{evidence, oracle, run};
use alloy_primitives::U256;
use evm_abstract::{
    analysis::{OutcomeKind, ProcessAction, ProcessOwner, ProcessSource, Status},
    world::{EvmAccount, EvmByteArray, Existence},
};
use revm_bytecode::opcode;

#[test]
fn static_callcode_nonzero_and_subsequent_delegate_are_actual_process_actions() {
    let mut a = Assembly::default();
    a.call(opcode::STATICCALL, U as u16, 0, (0, 0), (0, 64))
        .op(opcode::POP)
        .return_range(0, 64);
    let mut u = Assembly::default();
    u.call(opcode::CALLCODE, C as u16, 9, (0, 0), (0, 64))
        .op(opcode::POP)
        .return_range(0, 64);
    let mut c = Assembly::default();
    c.op(opcode::CALLVALUE)
        .mstore(0)
        .call(opcode::DELEGATECALL, V as u16, 0, (0, 0), (32, 32))
        .op(opcode::POP)
        .return_range(0, 64);
    let mut d = Assembly::default();
    d.op(opcode::CALLVALUE).return_top();
    let mut world = new_world();
    insert(&mut world, A, &a.finish(), &[(0, 5)]);
    insert(&mut world, C, &c.finish(), &[]);
    insert(&mut world, V, &d.finish(), &[]);
    world
        .insert(
            address(U),
            EvmAccount {
                balance: word(9),
                nonce: word(0),
                existence: Existence::Present,
                storage_unknown: false,
                ..EvmAccount::unknown()
            },
        )
        .unwrap();
    let concrete = oracle::concrete_unknown(&world, U, &u.finish());
    let witness = oracle::execute_with_gas(&concrete, &entry(EvmByteArray::empty()), 400_000);
    assert_eq!(witness.kind, OutcomeKind::Return);
    assert_eq!(witness.output_word(0), U256::from(9));
    assert_eq!(witness.output_word(32), U256::from(9));
    for source in [C, V] {
        assert!(witness.steps.iter().any(|step| step.code == address(source)
            && step.owner == address(U)
            && step.caller == address(U)
            && step.is_static
            && step.value == U256::from(9)));
    }
    let analysis = run(world, 2_000_000_000);
    evidence("static-callcode-nonzero", &analysis);
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.frontiers().is_empty());
    witness.covers(&analysis);
    // Complete decoded root inventory proves only STATICCALL can act. Its
    // recursive children are read-only, so balances remain original facts.
    for outcome in analysis.outcomes() {
        assert_eq!(
            outcome.store.read_balance(address(U)).singleton(),
            Some(U256::from(9))
        );
    }
    let record = analysis.procedure_summaries().last().unwrap();
    assert!(record.closed && record.source_current);
    for (action, source) in [
        (ProcessAction::CallCode, C),
        (ProcessAction::DelegateCall, V),
    ] {
        assert!(record.regional_cfg.calls.iter().any(|call| {
            let callee = &record.procedures[call.callee];
            let frame = callee.frame.process.as_ref().unwrap();
            call.action==Some(action) && call.target==Some(ProcessOwner::Named(address(source)))
                && frame.owner==ProcessOwner::Named(address(U)) && frame.is_static
                && matches!(frame.source, ProcessSource::Runtime {source: actual,..} if actual==address(source))
                && callee.process_input.as_ref().unwrap().value.contains(U256::from(9))
                && !call.deliveries.is_empty()
        }), "missing actual {action:?} subscription retaining nonzero value");
    }
}

fn constructor_factory(create2: bool, revert_outer: bool) -> (super::support::Fixture, Vec<u8>) {
    let mut init = Assembly::default();
    init.push(1)
        .mstore(0)
        .call(opcode::CALL, A as u16, 0, (0, 32), (0, 0))
        .op(opcode::POP)
        .call(opcode::CALL, 3, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .store(0, 7)
        .push(0xab)
        .push(0)
        .push(0)
        .op(opcode::LOG1)
        .push(1)
        .push_label("runtime")
        .push(0)
        .op(opcode::CODECOPY)
        .return_range(0, 1)
        .data("runtime", &[opcode::STOP]);
    let init = init.finish().bytes;
    let mut factory = Assembly::default();
    factory
        .input(0)
        .op(opcode::ISZERO)
        .jump("outer", true)
        .store(0, 42)
        .op(opcode::STOP)
        .label("outer")
        .push(init.len() as u16)
        .push_label("init")
        .push(0)
        .op(opcode::CODECOPY);
    if create2 {
        factory.push(19);
    }
    factory
        .push(init.len() as u16)
        .push(0)
        .push(3)
        .op(if create2 {
            opcode::CREATE2
        } else {
            opcode::CREATE
        });
    if revert_outer {
        factory.op(opcode::POP).push(0).push(0).op(opcode::REVERT);
    } else {
        factory.return_top();
    }
    factory.data("init", &init);
    (factory.finish(), init)
}

#[test]
fn constructors_callbacks_and_enclosing_rollback_use_installed_native_continuations() {
    use evm_abstract::{
        analysis::{
            TransferSemantics,
            process_creation::{CreationResult, ReturnedData, StoreDisposition},
        },
        world::Code,
    };
    for create2 in [false, true] {
        for named in [false, true] {
            for revert_outer in [false, true] {
                let (factory, init) = constructor_factory(create2, revert_outer);
                let destination = if create2 {
                    address(A).create2_from_code(U256::from(19).to_be_bytes::<32>(), &init)
                } else {
                    address(A).create(0)
                };
                let action = if create2 {
                    ProcessAction::Create2
                } else {
                    ProcessAction::Create
                };
                let target = if named {
                    ProcessOwner::Named(destination)
                } else {
                    ProcessOwner::Residual
                };
                let mut world = new_world();
                insert(&mut world, A, &factory, &[(0, 0)]);
                if named {
                    world
                        .insert(
                            destination,
                            EvmAccount {
                                balance: word(11),
                                ..EvmAccount::empty()
                            },
                        )
                        .unwrap();
                }
                let witness =
                    oracle::execute_with_gas(&world, &entry(EvmByteArray::empty()), 1_000_000);
                assert_eq!(
                    witness.kind,
                    if revert_outer {
                        OutcomeKind::Revert
                    } else {
                        OutcomeKind::Return
                    }
                );
                assert!(witness.steps.iter().any(|step| step.is_init
                    && step.owner == destination
                    && step.caller == address(A)
                    && step.value == U256::from(3)));
                assert!(witness.steps.iter().any(|step| !step.is_init
                    && step.code == address(A)
                    && step.opcode == opcode::SSTORE
                    && step.caller == destination));
                if !revert_outer {
                    assert_eq!(
                        witness.output_word(0),
                        U256::from_be_slice(destination.as_slice())
                    );
                }
                let analysis = run(world, 2_000_000_000);
                evidence(
                    &format!("constructor-{action:?}-named-{named}-outer-revert-{revert_outer}"),
                    &analysis,
                );
                assert_eq!(analysis.status(), Status::Converged);
                assert!(analysis.frontiers().is_empty());
                witness.covers(&analysis);
                let record = analysis.procedure_summaries().last().unwrap();
                assert!(record.closed && record.source_current);
                let allocation = record
                    .regional_cfg
                    .calls
                    .iter()
                    .find(|call| {
                        call.action == Some(action)
                            && call.target == Some(target)
                            && call
                                .deliveries
                                .iter()
                                .any(|d| d.current && d.kind == OutcomeKind::Return)
                    })
                    .expect("actual entered constructor and current Return delivery");
                let ctor = &record.procedures[allocation.callee];
                assert_eq!(ctor.semantics, TransferSemantics::ConstructorProcess);
                let ctor_key = ctor.frame.process.as_ref().unwrap();
                assert!(matches!(ctor_key.source, ProcessSource::AnyInitCode { .. }));
                assert_eq!(ctor_key.owner, target);
                if named {
                    assert!(matches!(ctor.input.code(destination), Some(Code::Empty)));
                    assert_eq!(ctor.input.nonce(destination).singleton(), Some(U256::ONE));
                    assert_eq!(ctor.input.created_in_transaction(destination), Some(true));
                }
                let returned = allocation
                    .deliveries
                    .iter()
                    .find(|d| d.kind == OutcomeKind::Return && d.current)
                    .unwrap();
                assert_eq!(returned.creation.len(), 2);
                assert!(
                    returned
                        .creation
                        .iter()
                        .any(|case| case.store == StoreDisposition::CommitConstructor
                            && case.result == CreationResult::Address(target)
                            && case.returndata == ReturnedData::Empty)
                );
                assert!(returned.creation.iter().any(|case| case.store
                    == StoreDisposition::RestorePostNonceCheckpoint
                    && case.result == CreationResult::Zero
                    && case.returndata == ReturnedData::Empty));
                assert!(!returned.successors.is_empty());
                assert!(
                    record.regional_cfg.calls.iter().any(|call| {
                        matches!(
                            record.regional_cfg.nodes[call.from]
                                .frame
                                .process
                                .as_ref()
                                .unwrap()
                                .source,
                            ProcessSource::AnyInitCode { .. }
                        ) && call.action == Some(ProcessAction::Call)
                            && call.target == Some(ProcessOwner::Named(address(A)))
                            && call.deliveries.iter().any(|d| {
                                d.current
                                    && d.kind == OutcomeKind::Return
                                    && !d.successors.is_empty()
                            })
                    }),
                    "constructor has an actual callback subscription and resumed continuation"
                );
                for call in &record.regional_cfg.calls {
                    if !matches!(
                        call.action,
                        Some(ProcessAction::Create | ProcessAction::Create2)
                    ) {
                        assert_ne!(
                            record.procedures[call.callee].semantics,
                            TransferSemantics::ConstructorProcess,
                            "ordinary account calls must never execute initcode"
                        );
                    }
                }
                if revert_outer {
                    let root = &record.procedures[record.root_procedure.unwrap()];
                    for output in root
                        .outputs
                        .iter()
                        .filter(|o| o.kind == OutcomeKind::Revert)
                    {
                        // Relative outputs are before transaction finalization:
                        // rollback retains the genuine may-touch at address 3.
                        assert_ne!(output.store.touched_in_transaction(address(3)), Some(false));
                    }
                    for outcome in analysis
                        .outcomes()
                        .iter()
                        .filter(|o| o.kind == OutcomeKind::Revert)
                    {
                        assert_eq!(
                            outcome.store.nonce(address(A)).singleton(),
                            Some(U256::ZERO)
                        );
                        assert_eq!(
                            super::support::read(&outcome.store, A, 0).singleton(),
                            Some(U256::ZERO)
                        );
                        assert!(
                            !outcome.store.logs_unknown()
                                && outcome.store.possible_logs().is_empty()
                        );
                        assert_eq!(
                            outcome.store.created_in_transaction(destination),
                            Some(false)
                        );
                        assert_eq!(outcome.store.pending_destruction(destination), Some(false));
                        // EIP-161 consumes the touch before transaction-local
                        // metadata is cleared in the final public outcome.
                        assert_eq!(
                            outcome.store.touched_in_transaction(address(3)),
                            Some(false)
                        );
                        if named {
                            assert!(matches!(outcome.store.code(destination), Some(Code::Empty)));
                            assert_eq!(
                                outcome.store.nonce(destination).singleton(),
                                Some(U256::ZERO)
                            );
                            assert_eq!(
                                outcome.store.read_balance(destination).singleton(),
                                Some(U256::from(11))
                            );
                        }
                    }
                }
            }
        }
    }
}
