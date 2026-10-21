//! Native results are checked through revm's independent transaction boundary.
use evm_abstract::analysis::AnalysisInput;
use evm_abstract::analysis::control::Control;

#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Address, Fork, U256,
    analysis::{self, ExecutionConfig, FrameCode, FrontierReason, OutcomeKind, Status},
    domain::EvmValue,
    ssa,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use revm::{
    Context, ExecuteEvm, MainBuilder, MainContext, context::TxEnv, database::InMemoryDB,
    primitives::TxKind, state::AccountInfo,
};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}
fn abstract_call(
    target: u64,
    input: EvmByteArray<NumericValue>,
    fork: Fork,
    config: ExecutionConfig,
) -> analysis::EvmWorldAnalysis<NumericValue> {
    native::run(
        &native::product(),
        AnalysisInput::new(
            EvmWorld::<NumericValue>::new(fork, "native oracle"),
            EvmEntry {
                address: address(target),
                environment: evm_abstract::world::EvmEnvironmentValues {
                    to: (address(target)).into(),
                    caller: (address(0x900)).into(),
                    value: EvmValue::<NumericValue>::constant(U256::ZERO),
                    calldata: input,
                    is_static: false,
                    ..evm_abstract::world::EvmEnvironmentValues::<NumericValue>::default()
                },
            },
        )
        .unwrap(),
        config,
        &Control::default(),
    )
    .unwrap()
}
fn compare(target: u64, input: &[u8], fork: Fork, success: bool) {
    let graph = abstract_call(
        target,
        EvmByteArray::<NumericValue>::exact(input),
        fork,
        ExecutionConfig::default(),
    );
    assert_eq!(
        graph.status(),
        Status::Converged,
        "{target:x} {fork}: {:?}",
        graph.frontiers()
    );
    assert_eq!(
        graph.states()[0].entry.active().code,
        FrameCode::Precompile(address(target))
    );
    let mut db = InMemoryDB::default();
    db.insert_account_info(
        address(0x900),
        AccountInfo {
            balance: U256::from(1_000_000_000),
            ..AccountInfo::default()
        },
    );
    let context = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(fork.spec_id()))
        .with_db(db);
    let mut evm = context.build_mainnet();
    let concrete = evm
        .transact(
            TxEnv::builder()
                .caller(address(0x900))
                .kind(TxKind::Call(address(target)))
                .gas_limit(10_000_000)
                .data(input.to_vec().into())
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        concrete.result.is_success(),
        success,
        "native {target:x} {fork}"
    );
    if success {
        let data = concrete.result.output().unwrap();
        assert!(
            graph
                .outcomes()
                .iter()
                .any(|o| o.kind == OutcomeKind::Return
                    && o.data.exact_bytes(native::product().borrowed()).as_deref()
                        == Some(data.as_ref()))
        );
    } else {
        assert!(
            !graph
                .outcomes()
                .iter()
                .any(|o| o.kind == OutcomeKind::Return)
        );
    }
    ssa::build_world(&graph).unwrap().verify(&graph).unwrap();
}

#[test]
fn native_successes_and_failure_cover_each_selected_fork() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for (target, input, success) in [
            (1, &[][..], true),
            (2, &b"hello"[..], true),
            (3, &b"hello"[..], true),
            (4, &b"arbitrary input"[..], true),
            (5, &[][..], true),
            (6, &[][..], true),
            (7, &[][..], true),
            (8, &[][..], true),
            (9, &[][..], false),
            (0x0a, &[][..], false),
        ] {
            compare(target, input, fork, success);
        }
    }
    compare(0x100, &[], Fork::Osaka, true);
}
#[test]
fn identity_output_is_copied_back_to_the_callers_requested_range() {
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "identity CALL");
    world
        .insert(
            address(0x101),
            EvmAccount::<NumericValue>::from_hex(
                "602a5f5260205f60205f5f60045af15060205ff3",
                Fork::Osaka,
            )
            .unwrap(),
        )
        .unwrap();
    let graph = native::run(
        &native::product(),
        AnalysisInput::new(
            world,
            EvmEntry {
                address: address(0x101),
                environment: evm_abstract::world::EvmEnvironmentValues {
                    to: (address(0x101)).into(),
                    caller: (address(0x900)).into(),
                    value: EvmValue::<NumericValue>::constant(U256::ZERO),
                    calldata: EvmByteArray::<NumericValue>::empty(),
                    is_static: false,
                    ..evm_abstract::world::EvmEnvironmentValues::<NumericValue>::default()
                },
            },
        )
        .unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .unwrap();
    assert_eq!(graph.status(), Status::Converged);
    assert!(graph.outcomes().iter().any(|o| {
        o.kind == OutcomeKind::Return
            && o.data
                .read_word(
                    &EvmValue::<NumericValue>::constant(U256::ZERO),
                    native::product().borrowed(),
                )
                .contains(U256::from(42))
    }));
    assert!(
        graph
            .states()
            .iter()
            .any(|s| s.entry.active().code == FrameCode::Precompile(address(4)))
    );
    ssa::build_world(&graph).unwrap();
}
#[test]
fn symbolic_native_inputs_continue_and_large_crypto_work_keeps_typed_frontiers() {
    let input = EvmByteArray::<NumericValue>::unknown();
    let unknown = abstract_call(4, input.clone(), Fork::Osaka, ExecutionConfig::default());
    assert_eq!(unknown.status(), Status::Converged);
    assert!(unknown.frontiers().is_empty());
    for kind in [
        OutcomeKind::Return,
        OutcomeKind::Revert,
        OutcomeKind::Failure,
    ] {
        assert!(
            unknown
                .outcomes()
                .iter()
                .any(|outcome| outcome.kind == kind)
        );
    }
    // Root entry binds the unobserved source once. Identity must preserve that
    // complete executed array, rather than the unbound constructor's identity.
    let bound_input = &unknown.states()[0].entry.active().calldata;
    assert!(
        unknown
            .outcomes()
            .iter()
            .any(|outcome| { outcome.kind == OutcomeKind::Return && &outcome.data == bound_input })
    );
    let mut blake = vec![0u8; 213];
    blake[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    blake[212] = 1;
    let work = abstract_call(
        9,
        EvmByteArray::<NumericValue>::exact(&blake),
        Fork::Osaka,
        ExecutionConfig {
            max_work: 2_000,
            ..ExecutionConfig::default()
        },
    );
    assert_eq!(work.status(), Status::Incomplete);
    assert!(
        work.frontiers()
            .iter()
            .any(|f| f.reason == FrontierReason::Work)
    );
    assert!(work.work() <= 2_000);
    assert!(ssa::build_world(&work).is_err());
}
