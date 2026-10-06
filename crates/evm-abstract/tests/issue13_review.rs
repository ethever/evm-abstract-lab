//! Independent issue-13 acceptance regressions across call/store/SSA boundaries.
#[path = "summaries/oracle.rs"]
mod oracle;

use evm_abstract::{
    Fork, U256,
    analysis::{ExecutionConfig, OutcomeKind, Status, analyze_world},
    domain::{Domain, Value},
    ssa,
    world::{Account, ByteArray, Entry, World},
};
use oracle::address;

fn entry() -> Entry {
    Entry {
        address: address(0x101),
        environment: evm_abstract::world::EvmEnvironment {
            to: (address(0x101)).into(),
            caller: (address(0x1000)).into(),
            value: Value::constant(U256::ZERO),
            calldata: ByteArray::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironment::default()
        },
    }
}

fn call(target: u16, value: u8) -> String {
    format!("60205f5f5f60{value:02x}61{target:04x}620f4240f1")
}

#[test]
fn shared_implementation_transient_storage_is_isolated_between_proxies() {
    let root = format!(
        "{}505f515f55{}505f51600155{}505f5160025500",
        call(0x201, 0),
        call(0x202, 0),
        call(0x201, 0)
    );
    let proxy = "60205f5f5f610300620f4240f45060205ff3";
    let implementation = "5f5c600101805f5d5f5260205ff3";
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let mut world = World::new(fork, "review:two-proxy-transient");
        for (number, code) in [
            (0x101, root.as_str()),
            (0x201, proxy),
            (0x202, proxy),
            (0x300, implementation),
        ] {
            world
                .insert(address(number), Account::from_hex(code, fork).unwrap())
                .unwrap();
        }
        let graph = analyze_world(world.clone(), entry(), ExecutionConfig::default()).unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        assert_eq!(
            oracle::compare(&world, &entry(), &graph),
            OutcomeKind::Return
        );
        ssa::build_world(&graph).unwrap().verify(&graph).unwrap();
        let zero = Value::constant(U256::ZERO);
        assert!(
            graph
                .outcomes()
                .iter()
                .filter(|outcome| outcome.kind == OutcomeKind::Return)
                .any(|outcome| {
                    [(0, 1), (1, 1), (2, 2)].iter().all(|(slot, value)| {
                        let read = outcome.store.read(
                            address(0x101),
                            &Value::constant(U256::from(*slot)),
                            Domain::default(),
                        );
                        read.constants().is_some() && read.contains(U256::from(*value))
                    }) && [(0x201, 2), (0x202, 1), (0x300, 0)]
                        .iter()
                        .all(|(owner, value)| {
                            let read = outcome.store.read_transient(
                                address(*owner),
                                &zero,
                                Domain::default(),
                            );
                            read.constants().is_some() && read.contains(U256::from(*value))
                        })
                }),
            "shared implementation aliased transient owners: {:?}",
            graph.outcomes()
        );
    }
}

#[test]
fn value_call_to_confirmed_empty_account_creates_present_state() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let root = format!("{}506102003f5f5260205ff3", call(0x200, 5));
        let mut world = World::new(fork, "review:empty-value-call");
        let mut root_account = Account::from_hex(&root, fork).unwrap();
        root_account.balance = Value::constant(U256::from(100));
        world.insert(address(0x101), root_account).unwrap();
        world.insert(address(0x200), Account::absent()).unwrap();
        let graph = analyze_world(world.clone(), entry(), ExecutionConfig::default()).unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        assert_eq!(
            oracle::compare(&world, &entry(), &graph),
            OutcomeKind::Return
        );
        ssa::build_world(&graph).unwrap().verify(&graph).unwrap();
        assert!(
            graph
                .outcomes()
                .iter()
                .any(|outcome| outcome.kind == OutcomeKind::Return
                    && outcome.store.read_balance(address(0x200)).singleton()
                        == Value::constant(U256::from(5)).singleton()
                    && outcome.store.existence(address(0x200))
                        == evm_abstract::world::Existence::Present
                    && outcome
                        .data
                        .read_word(&Value::constant(U256::ZERO), Domain::default())
                        .contains(U256::from_be_slice(
                            alloy_primitives::keccak256([]).as_slice()
                        ))),
            "value transfer retained absent-account metadata: {:?}",
            graph.outcomes()
        );
    }
}

#[test]
fn balance_updates_preserve_sound_account_presence_for_mixed_values() {
    let mut world = World::new(Fork::Osaka, "review:balance-presence");
    world.insert(address(0x200), Account::absent()).unwrap();
    world.insert(address(0x201), Account::empty()).unwrap();
    let mut exact = evm_abstract::world::Store::new(&world);
    exact.write_balance(address(0x200), Value::constant(U256::from(5)));
    assert_eq!(
        exact.existence(address(0x200)),
        evm_abstract::world::Existence::Present
    );
    let mut mixed = evm_abstract::world::Store::new(&world);
    mixed.write_balance(
        address(0x200),
        Domain::default().join(
            &Value::constant(U256::ZERO),
            &Value::constant(U256::from(5)),
        ),
    );
    assert_eq!(
        mixed.existence(address(0x200)),
        evm_abstract::world::Existence::Unknown
    );
    mixed.write_balance(address(0x201), Value::constant(U256::ZERO));
    assert_eq!(
        mixed.existence(address(0x201)),
        evm_abstract::world::Existence::Present
    );
}

#[test]
fn prague_and_osaka_bls_native_operations_cover_valid_and_invalid_inputs() {
    // Infinity uses zero point coordinates. Mapping the zero field element is
    // also a valid nontrivial native operation; the independent oracle checks
    // the actual output rather than importing expected bytes from our transfer.
    for fork in [Fork::Prague, Fork::Osaka] {
        for (number, input_size, output_size) in [
            (0x0b, 256, 128),
            (0x0c, 160, 128),
            (0x0d, 512, 256),
            (0x0e, 288, 256),
            (0x0f, 384, 32),
            (0x10, 64, 128),
            (0x11, 128, 256),
        ] {
            let world = World::new(fork, "review:bls-native");
            let mut input = entry();
            input.address = address(number);
            input.environment.to = address(number).into();
            input.environment.calldata = ByteArray::exact(&vec![0; input_size]);
            let graph = analyze_world(
                world.clone(),
                input.clone(),
                ExecutionConfig {
                    max_work: 10_000_000,
                    ..ExecutionConfig::default()
                },
            )
            .unwrap();
            assert_eq!(
                graph.status(),
                Status::Converged,
                "{fork} native{number:x}: {:?}",
                graph.frontiers()
            );
            assert!(
                graph
                    .outcomes()
                    .iter()
                    .any(|outcome| outcome.kind == OutcomeKind::Return
                        && outcome.data.len() == &Value::constant(U256::from(output_size)))
            );
            assert_eq!(oracle::compare(&world, &input, &graph), OutcomeKind::Return);
            ssa::build_world(&graph).unwrap().verify(&graph).unwrap();
            input.environment.calldata = ByteArray::empty();
            let invalid =
                analyze_world(world.clone(), input.clone(), ExecutionConfig::default()).unwrap();
            assert_eq!(invalid.status(), Status::Converged);
            assert!(
                !invalid
                    .outcomes()
                    .iter()
                    .any(|outcome| outcome.kind == OutcomeKind::Return)
            );
            assert_eq!(
                oracle::compare(&world, &input, &invalid),
                OutcomeKind::Failure
            );
            ssa::build_world(&invalid)
                .unwrap()
                .verify(&invalid)
                .unwrap();
        }
    }
}
