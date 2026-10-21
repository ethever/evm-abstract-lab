//! Offline multi-account revm differentials. The oracle loads the same bytes
//! and initial state, but does not reuse the abstract interpreter's transfers.
//! Finite fixtures establish regression evidence, not an EVM-wide proof.
use evm_abstract::analysis::AnalysisInput;
use evm_abstract::analysis::control::Control;

#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Fork, U256,
    analysis::{
        EvmWorldAnalysis, ExecutionConfig, FrameCode, MachineEdgeKind, OutcomeKind,
        ProcedureSummaryKind, Status,
    },
    domain::EvmValue,
    ssa,
    world::{Code, EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::TxEnv,
    database::InMemoryDB,
    interpreter::{Interpreter, interpreter::EthInterpreter, interpreter_types::Jumps},
    primitives::{Address, Bytes, TxKind, hex},
    state::{AccountInfo, Bytecode},
};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
struct Step {
    code: Address,
    address: Address,
    caller: Address,
    is_static: bool,
    pc: usize,
    stack: Vec<U256>,
}

#[derive(Default, Debug)]
struct Trace {
    steps: Vec<Step>,
}

impl<CTX> Inspector<CTX, EthInterpreter> for Trace {
    fn step(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        self.steps.push(Step {
            code: interpreter.input.bytecode_address.unwrap(),
            address: interpreter.input.target_address,
            caller: interpreter.input.caller_address,
            is_static: interpreter.runtime_flag.is_static,
            pc: interpreter.bytecode.pc(),
            stack: interpreter.stack.data().clone(),
        });
    }
}

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn fixture(name: &str, fork: Fork) -> (EvmWorld<NumericValue>, InMemoryDB) {
    let text = std::fs::read_to_string(format!(
        "{}/../../examples/worlds/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut world = EvmWorld::<NumericValue>::new(fork, json["provenance"].as_str().unwrap());
    let mut db = InMemoryDB::default();
    for input in json["accounts"].as_array().unwrap() {
        let address = input["address"].as_str().unwrap().parse().unwrap();
        let code = input["code"].as_str().unwrap();
        let balance: U256 = input["balance"].as_str().unwrap().parse().unwrap();
        let mut account = EvmAccount::<NumericValue>::from_hex(code, fork).unwrap();
        account.balance = EvmValue::<NumericValue>::constant(balance);
        assert_eq!(input["storage_unknown"], false);
        let raw = hex::decode(code.trim_start_matches("0x")).unwrap();
        let bytecode = Bytecode::new_raw(Bytes::from(raw));
        db.insert_account_info(
            address,
            AccountInfo::new(balance, 0, bytecode.hash_slow(), bytecode),
        );
        for (slot, value) in input["storage"].as_object().unwrap() {
            let slot: U256 = slot.parse().unwrap();
            let value: U256 = value.as_str().unwrap().parse().unwrap();
            account
                .storage
                .insert(slot, EvmValue::<NumericValue>::constant(value));
            db.insert_account_storage(address, slot, value).unwrap();
        }
        world.insert(address, account).unwrap();
    }
    db.insert_account_info(
        address(0x1000),
        AccountInfo {
            balance: U256::from(100_000_000),
            ..AccountInfo::default()
        },
    );
    (world, db)
}

fn covers_steps(analysis: &EvmWorldAnalysis<NumericValue>, trace: Trace) {
    for step in trace.steps {
        let matching: Vec<_> = analysis
            .states()
            .iter()
            .filter(|state| {
                let frame = state.active();
                frame.code_address == step.code
                    && frame.address == step.address
                    && frame.caller.as_concrete() == Some(step.caller)
                    && frame.is_static == step.is_static
                    && state.executed_pcs.contains(&step.pc)
            })
            .collect();
        // A recursive callee's actual analyzed blocks belong to the procedure
        // record. Its public evidence does not fabricate ordinary machine nodes.
        // Every available main-graph block-entry stack is still checked below;
        // compare() separately requires one outcome to cover all concrete effects.
        let in_procedure = analysis.procedure_summaries().iter().any(|record| {
            record.kind == ProcedureSummaryKind::KnownRecursion
                && record.closed
                && record
                    .root_procedure
                    .and_then(|root| record.procedures.get(root))
                    .is_some_and(|root| !root.outputs.is_empty())
                && record.procedures.iter().any(|procedure| {
                    let frame = &procedure.frame;
                    let Some(account) = analysis.world().account(step.code) else {
                        return false;
                    };
                    let Code::Runtime(program) = &account.code else {
                        return false;
                    };
                    frame.mode == FrameCode::Runtime
                        && frame.code_hash == alloy_primitives::keccak256(program.bytes())
                        && frame.code_address == step.code
                        && frame.address == step.address
                        && frame.caller.as_concrete() == Some(step.caller)
                        && frame.is_static == step.is_static
                        && !procedure.outputs.is_empty()
                        && procedure.covered_blocks.iter().any(|block| {
                            program.blocks().get(*block).is_some_and(|block| {
                                block.instructions.iter().any(|i| i.pc == step.pc)
                            })
                        })
                })
        });
        assert!(
            !matching.is_empty() || in_procedure,
            "concrete opcode/frame missing from analyzed graph and closed procedure: {step:?}"
        );
        if matching
            .iter()
            .any(|state| state.executed_pcs.first() == Some(&step.pc))
        {
            assert!(
                matching.iter().any(|state| {
                    let entry = &state.entry.active().stack;
                    entry.len() == step.stack.len()
                        && entry
                            .iter()
                            .zip(&step.stack)
                            .all(|(value, concrete)| value.contains(*concrete))
                }),
                "concrete block-entry stack not covered: {step:?}"
            );
        }
    }
}

fn compare(name: &str, fork: Fork) -> EvmWorldAnalysis<NumericValue> {
    let (world, db) = fixture(name, fork);
    let entry = EvmEntry {
        address: address(0x101),
        environment: evm_abstract::world::EvmEnvironmentValues {
            to: (address(0x101)).into(),
            caller: (address(0x1000)).into(),
            value: EvmValue::<NumericValue>::constant(U256::ZERO),
            calldata: EvmByteArray::<NumericValue>::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironmentValues::<NumericValue>::default()
        },
    };
    let analysis = native::run(
        &native::product(),
        AnalysisInput::new(world, entry).unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .unwrap();
    assert_eq!(analysis.status(), Status::Converged, "{name} {fork}");
    ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();
    let context = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(fork.spec_id()))
        .with_db(db);
    let mut evm = context.build_mainnet_with_inspector(Trace::default());
    let result = evm
        .inspect_tx(
            TxEnv::builder()
                .caller(address(0x1000))
                .kind(TxKind::Call(address(0x101)))
                .gas_limit(10_000_000)
                .build()
                .unwrap(),
        )
        .unwrap();
    assert!(result.result.is_success(), "{name}: {:?}", result.result);
    assert!(
        !evm.inspector.steps.is_empty(),
        "oracle did not execute fixture runtime bytecode"
    );
    assert!(
        evm.inspector
            .steps
            .iter()
            .any(|step| step.code != address(0x101)),
        "oracle did not enter any fixture callee"
    );
    let trace = format!("{:?}", evm.inspector);
    covers_steps(&analysis, evm.inspector);
    let concrete_output = result.result.output().unwrap();
    let covers = analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .any(|outcome| {
            outcome
                .data
                .len()
                .contains(U256::from(concrete_output.len()))
                && concrete_output.iter().enumerate().all(|(offset, byte)| {
                    outcome
                        .data
                        .byte_at(offset, native::product().borrowed())
                        .contains(U256::from(*byte))
                })
                && result.state.iter().all(|(owner, account)| {
                    if analysis.world().account(*owner).is_none() {
                        return true; // Sender/coinbase gas accounting is outside this model.
                    }
                    account.storage.iter().all(|(slot, value)| {
                        outcome
                            .store
                            .read(
                                *owner,
                                &EvmValue::<NumericValue>::constant(*slot),
                                native::product().borrowed(),
                            )
                            .contains(value.present_value())
                    }) && outcome
                        .store
                        .read_balance(*owner)
                        .contains(account.info.balance)
                })
                && result.result.logs().iter().all(|log| {
                    outcome.store.logs_unknown()
                        || outcome
                            .store
                            .possible_logs()
                            .iter()
                            .any(|(site, possible)| {
                                site.address == log.address
                                    && possible.topics.len() == log.data.topics().len()
                                    && possible.topics.iter().zip(log.data.topics()).all(
                                        |(possible, concrete)| {
                                            possible
                                                .contains(U256::from_be_slice(concrete.as_slice()))
                                        },
                                    )
                                    && possible
                                        .data
                                        .len()
                                        .contains(U256::from(log.data.data.len()))
                                    && log.data.data.iter().enumerate().all(|(offset, byte)| {
                                        possible
                                            .data
                                            .byte_at(offset, native::product().borrowed())
                                            .contains(U256::from(*byte))
                                    })
                            })
                })
        });
    assert!(
        covers,
        "{name} {fork}: no abstract outcome jointly covers concrete return, storage and balances\nconcrete={result:?}\nabstract={:?}\ntrace={trace}",
        analysis.outcomes()
    );
    assert!(
        analysis
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Call)
    );
    analysis
}

fn outcome_slots(
    analysis: &EvmWorldAnalysis<NumericValue>,
) -> BTreeMap<(Address, U256), EvmValue<NumericValue>> {
    let mut stores = analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .map(|outcome| &outcome.store);
    let first = stores.next().unwrap().clone();
    stores
        .fold(first, |joined, store| {
            joined.join(store, native::product().borrowed())
        })
        .slots()
        .iter()
        .map(|(key, value)| (*key, value.clone()))
        .collect()
}

fn finite_contains(value: &EvmValue<NumericValue>, expected: u64) {
    assert!(
        value.constants().is_some(),
        "expected a finite effect, got {value}"
    );
    assert!(
        value.contains(U256::from(expected)),
        "{value} misses {expected}"
    );
}

#[test]
fn calls_returns_and_account_effects_cover_revm_under_each_supported_fork() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for name in [
            "call-return-branch",
            "proxy-storage",
            "callcode-context",
            "returndata-copy",
            "revert-rollback",
            "static-write",
            "reentry",
            "log-rollback",
        ] {
            compare(name, fork);
        }
    }
}

#[test]
fn returned_word_controls_the_caller_branch() {
    let analysis = compare("call-return-branch", Fork::Osaka);
    finite_contains(&outcome_slots(&analysis)[&(address(0x101), U256::ZERO)], 1);
}

#[test]
fn shared_implementation_preserves_proxy_storage_caller_and_callvalue() {
    let analysis = compare("proxy-storage", Fork::Osaka);
    let slots = outcome_slots(&analysis);
    for (proxy, counter, callvalue) in [(0x201, 6, 7), (0x202, 10, 11)] {
        for (slot, expected) in [(0, counter), (1, proxy), (2, 0x101), (3, callvalue)] {
            finite_contains(&slots[&(address(proxy), U256::from(slot))], expected);
        }
    }
    assert_eq!(
        slots[&(address(0x300), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::from(99))
    );
}

#[test]
fn callcode_uses_proxy_caller_and_explicit_callvalue() {
    let analysis = compare("callcode-context", Fork::Osaka);
    let slots = outcome_slots(&analysis);
    for proxy in [0x201, 0x202] {
        finite_contains(&slots[&(address(proxy), U256::from(2))], proxy);
        finite_contains(&slots[&(address(proxy), U256::from(3))], 3);
    }
}

#[test]
fn revert_and_static_failure_restore_callee_state() {
    let analysis = compare("revert-rollback", Fork::Osaka);
    let slots = outcome_slots(&analysis);
    assert_eq!(
        slots[&(address(0x101), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::from(3))
    );
    finite_contains(&slots[&(address(0x101), U256::from(1))], 42);
    assert_eq!(
        slots[&(address(0x101), U256::from(2))],
        EvmValue::<NumericValue>::constant(U256::ZERO)
    );
    assert_eq!(
        slots[&(address(0x200), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::from(4))
    );
    assert!(
        analysis
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Revert)
    );
    let analysis = compare("static-write", Fork::Osaka);
    let slots = outcome_slots(&analysis);
    assert_eq!(
        slots[&(address(0x101), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::ZERO)
    );
    assert_eq!(
        slots[&(address(0x200), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::from(4))
    );
    assert!(
        analysis
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Failure)
    );
}

#[test]
fn reentrant_read_observes_the_current_transaction_store() {
    let analysis = compare("reentry", Fork::Osaka);
    let slots = outcome_slots(&analysis);
    assert_eq!(
        slots[&(address(0x101), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::from(2))
    );
    finite_contains(&slots[&(address(0x101), U256::from(1))], 1);
    assert!(
        analysis
            .states()
            .iter()
            .any(|state| state.key.frames.len() == 3)
    );
}

#[test]
fn reverted_child_logs_do_not_leak_into_returned_store() {
    let analysis = compare("log-rollback", Fork::Osaka);
    for outcome in analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
    {
        assert!(!outcome.store.logs_unknown());
        assert!(
            outcome
                .store
                .possible_logs()
                .keys()
                .all(|site| site.address == address(0x101))
        );
    }
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return
                && !outcome.store.possible_logs().is_empty())
    );
}
