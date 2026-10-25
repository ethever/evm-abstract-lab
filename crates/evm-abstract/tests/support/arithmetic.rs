//! The pinned upstream EVM, independent of every domain's scalar evaluator.
use alloy_primitives::U256;
use evm_abstract::Fork;
use revm::{
    Context, InspectEvm, MainBuilder, MainContext,
    context::TxEnv,
    database::{BENCH_CALLER, BENCH_TARGET, BenchmarkDB},
    primitives::{Bytes, TxKind},
    state::Bytecode,
};

#[path = "assembly.rs"]
mod assembly;

pub fn evaluate(opcode: u8, operands: &[U256], fork: Fork) -> U256 {
    let mut code = assembly::Assembly::default();
    for value in operands.iter().rev() {
        code.push_word(*value);
    }
    // Return the whole word; no production transfer or membership constructs it.
    code.op(opcode).op(0x5f).op(0x52).push(32).op(0x5f).op(0xf3);
    let context = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(fork.spec_id()))
        .with_db(BenchmarkDB::new_bytecode(Bytecode::new_raw(Bytes::from(
            code.finish().bytes,
        ))));
    let mut evm = context.build_mainnet_with_inspector(revm::inspector::NoOpInspector);
    let result = evm
        .inspect_one_tx(
            TxEnv::builder()
                .caller(BENCH_CALLER)
                .kind(TxKind::Call(BENCH_TARGET))
                .gas_limit(1_000_000)
                .build()
                .unwrap(),
        )
        .unwrap();
    assert!(result.is_success(), "revm arithmetic failed: {result:?}");
    let bytes = result.output().unwrap();
    assert_eq!(bytes.len(), 32);
    U256::from_be_slice(bytes)
}
