//! revm executes the finite implementation of the unknown root. The journal is
//! inspected after the child returns, before transaction transient-state cleanup.

use super::{fixture, support::*};
use alloy_primitives::{Address, U256};
use evm_abstract::analysis::OutcomeKind;
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::TxEnv,
    context_interface::{ContextTr, JournalTr},
    database::InMemoryDB,
    interpreter::{
        CallInputs, CallOutcome, Interpreter, interpreter::EthInterpreter, interpreter_types::Jumps,
    },
    primitives::{Bytes, TxKind},
    state::{AccountInfo, Bytecode},
};
use revm_bytecode::opcode;

#[derive(Default)]
struct Trace {
    resumed: usize,
    steps: Vec<(Address, Address, Address, bool, usize)>,
    writes: Vec<(U256, U256)>,
    callback: Option<(OutcomeKind, Vec<u8>)>,
    transient: Option<U256>,
}

impl<CTX: ContextTr> Inspector<CTX, EthInterpreter> for Trace {
    fn step(&mut self, interpreter: &mut Interpreter, context: &mut CTX) {
        let owner = interpreter.input.target_address;
        let pc = interpreter.bytecode.pc();
        if owner == address(U) && pc == self.resumed {
            self.transient = Some(context.journal_mut().tload(address(B), U256::ZERO));
        }
        if owner == address(B) {
            self.steps.push((
                interpreter.input.bytecode_address.unwrap_or(owner),
                owner,
                interpreter.input.caller_address,
                interpreter.runtime_flag.is_static,
                pc,
            ));
            if interpreter.bytecode.opcode() == opcode::SSTORE {
                self.writes.push((
                    interpreter.stack.peek(0).unwrap(),
                    interpreter.stack.peek(1).unwrap(),
                ));
            }
        }
    }

    fn call_end(&mut self, _: &mut CTX, inputs: &CallInputs, outcome: &mut CallOutcome) {
        if inputs.target_address == address(B) {
            let kind = if outcome.result.result.is_ok() {
                OutcomeKind::Return
            } else if outcome.result.result.is_revert() {
                OutcomeKind::Revert
            } else {
                OutcomeKind::Failure
            };
            assert!(
                self.callback
                    .replace((kind, outcome.result.output.to_vec()))
                    .is_none()
            );
        }
    }
}

pub(super) struct Observation {
    pub(super) callback_data: Vec<u8>,
    pub(super) slot: u64,
    pub(super) transient: u64,
    kind: OutcomeKind,
    trace: Trace,
}

impl Observation {
    pub(super) fn assert_callback(&self, code: &Fixture, kind: OutcomeKind, expected: u64) {
        assert_eq!(self.kind, kind);
        assert_eq!(self.callback_data, U256::from(expected).to_be_bytes::<32>());
        assert!(
            self.trace
                .steps
                .iter()
                .any(|step| step.4 == code.labels["update"])
        );
        assert!(
            self.trace
                .steps
                .iter()
                .any(|step| step.4 == code.labels["refused"])
        );
        assert!(self.trace.steps.iter().all(|step| {
            (step.0, step.1, step.2, step.3) == (address(B), address(B), address(U), false)
        }));
        assert_eq!(
            self.trace.writes,
            [(U256::ZERO, U256::from(99)), (U256::ZERO, U256::from(42))]
        );
        assert_eq!(self.slot, if kind == OutcomeKind::Return { 42 } else { 7 });
        assert_eq!(
            self.transient,
            if kind == OutcomeKind::Return { 99 } else { 0 }
        );
    }
}

pub(super) fn execute(code: &Fixture, bit: u16) -> Observation {
    let environment = fixture::concrete_environment(bit);
    let world = world(code, Some(&environment));
    let mut db = InMemoryDB::default();
    for (owner, account) in world.accounts() {
        let code = Bytecode::new_raw(Bytes::from(world.raw_account_code(*owner).unwrap()));
        db.insert_account_info(
            *owner,
            AccountInfo::new(U256::from(BALANCE), 0, code.hash_slow(), code),
        );
        for (key, value) in &account.storage {
            db.insert_account_storage(*owner, *key, value.singleton().unwrap())
                .unwrap();
        }
    }
    db.insert_account_info(
        address(USER),
        AccountInfo {
            balance: U256::MAX,
            ..AccountInfo::default()
        },
    );
    let context = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(world.fork().spec_id()))
        .modify_block_chained(|block| {
            block.gas_limit = 10_000_000;
            block.basefee = 0;
        })
        .with_db(db);
    let mut evm = context.build_mainnet_with_inspector(Trace {
        resumed: environment.labels["resumed"],
        ..Trace::default()
    });
    let result = evm
        .inspect_tx(
            TxEnv::builder()
                .caller(address(USER))
                .kind(TxKind::Call(address(U)))
                .gas_limit(10_000_000)
                .gas_price(0)
                .build()
                .unwrap(),
        )
        .unwrap();
    assert!(result.result.is_success());
    assert!(result.result.logs().is_empty());
    for owner in [B, U] {
        let account = result.state.get(&address(owner)).unwrap();
        assert_eq!(account.info.balance, U256::from(BALANCE));
        assert!(!account.is_selfdestructed());
    }
    let slot = result
        .state
        .get(&address(B))
        .and_then(|account| account.storage.get(&U256::ZERO))
        .map_or(U256::from(7), |slot| slot.present_value());
    let transient = evm
        .inspector
        .transient
        .expect("parent resumed after real callback");
    let (kind, callback_data) = evm
        .inspector
        .callback
        .clone()
        .expect("real callback outcome");
    assert_eq!(result.result.output().unwrap().as_ref(), callback_data);
    Observation {
        callback_data,
        slot: u64::try_from(slot).unwrap(),
        transient: u64::try_from(transient).unwrap(),
        kind,
        trace: evm.inspector,
    }
}
