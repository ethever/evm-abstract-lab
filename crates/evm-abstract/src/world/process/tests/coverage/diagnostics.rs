//! Concrete control for the original matrix's captured-code entry failure.

use super::{Trace, address, emit_call, insert};
use crate::{Fork, world::process::ProcessAction};
use alloy_primitives::U256;
use revm::{
    Context, InspectEvm, MainBuilder, MainContext, context::TxEnv, database::InMemoryDB,
    primitives::TxKind, state::AccountInfo,
};
use revm_bytecode::opcode;

#[test]
fn concrete_original_root_is_osaka_precompile_while_corrected_root_executes_runtime() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for root in [0x100, 0x101] {
            let mut code = emit_call(ProcessAction::Call, address(0x200), 0, 0, 0);
            code.extend([opcode::POP, opcode::STOP]);
            let mut db = InMemoryDB::default();
            insert(&mut db, address(root), code, 0);
            insert(&mut db, address(0x200), vec![opcode::STOP], 0);
            db.insert_account_info(
                address(0x900),
                AccountInfo {
                    balance: U256::from(100_000_000),
                    ..AccountInfo::default()
                },
            );
            let context = Context::mainnet()
                .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(fork.spec_id()))
                .with_db(db);
            let mut evm = context.build_mainnet_with_inspector(Trace::default());
            let result = evm
                .inspect_tx(
                    TxEnv::builder()
                        .caller(address(0x900))
                        .kind(TxKind::Call(address(root)))
                        .gas_limit(10_000_000)
                        .build()
                        .unwrap(),
                )
                .unwrap();
            let entered_runtime = evm
                .inspector
                .0
                .iter()
                .any(|(source, _, _, _)| *source == address(0x200));
            assert_eq!(entered_runtime, fork != Fork::Osaka || root != 0x100);
            eprintln!(
                "PROCESS_OWNER_RED_DIAGNOSIS {}",
                serde_json::json!({"fork":fork,"root":address(root),"captured_runtime_entered":entered_runtime,"transaction_success":result.result.is_success()})
            );
        }
    }
}
