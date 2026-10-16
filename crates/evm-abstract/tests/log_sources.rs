//! LOG sites retain captured code identity across creation and runtime calls.

use alloy_primitives::{B256, hex, keccak256};
use evm_abstract::{
    Address, Fork, U256,
    analysis::{ExecutionConfig, OutcomeKind, Status, analyze_world},
    domain::{AbstractValue, Domain},
    render,
    world::{AbstractLog, Account, ByteArray, Entry, LogKey, LogSourceKind, Store, World},
};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

fn push2(bytes: &mut Vec<u8>, number: usize) {
    let number = u16::try_from(number).unwrap();
    bytes.push(0x61);
    bytes.extend(number.to_be_bytes());
}

fn init(runtime: &[u8], prefix: &[u8]) -> Vec<u8> {
    let mut bytes = prefix.to_vec();
    let offset = prefix.len() + 13;
    push2(&mut bytes, runtime.len());
    push2(&mut bytes, offset);
    bytes.extend([0x5f, 0x39]);
    push2(&mut bytes, runtime.len());
    bytes.extend([0x5f, 0xf3]);
    assert_eq!(bytes.len(), offset);
    bytes.extend(runtime);
    bytes
}

fn factory(initcode: &[u8], repeated_call: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    push2(&mut bytes, initcode.len());
    push2(&mut bytes, 0);
    bytes.extend([0x5f, 0x39]);
    push2(&mut bytes, initcode.len());
    bytes.extend([0x5f, 0x5f, 0xf0]);
    bytes.extend(hex::decode("805f5550").unwrap());
    for _ in 0..if repeated_call { 2 } else { 1 } {
        bytes.extend(hex::decode("5f5f5f5f5f5f5462fffffff150").unwrap());
    }
    bytes.push(0x00);
    let offset = u16::try_from(bytes.len()).unwrap().to_be_bytes();
    bytes[4..6].copy_from_slice(&offset);
    bytes.extend(initcode);
    bytes
}

#[test]
fn actual_init_and_runtime_same_owner_same_pc_keep_distinct_log_arities() {
    for nonempty_data in [false, true] {
        let (prefix, runtime, pc, init_data, runtime_data) = if nonempty_data {
            // Both source PCs are 8. LOG0 leaves the unused topic on its stack;
            // LOG1 consumes it. Data comes from each frame's real MSTORE8.
            (
                hex::decode("60aa5f535f60015fa0").unwrap(),
                hex::decode("60bb5f535f60015fa100").unwrap(),
                8,
                vec![0xaa],
                vec![0xbb],
            )
        } else {
            // The Issue134 PC-3 LOG0 / LOG1 counterexample.
            (
                hex::decode("5f5f5fa0").unwrap(),
                hex::decode("5f5f5fa100").unwrap(),
                3,
                vec![],
                vec![],
            )
        };
        let initcode = init(&runtime, &prefix);
        let owner = address(0x101);
        let child = owner.create(0);
        for repeated_call in [false, true] {
            let code = factory(&initcode, repeated_call);
            let mut world = World::new(Fork::Osaka, "issue134:log-source");
            let mut account = Account::from_hex(&hex::encode(&code), world.fork()).unwrap();
            account.balance = word(100);
            world.insert(owner, account).unwrap();
            world.insert(child, Account::absent()).unwrap();
            let mut entry = Entry::new(owner);
            entry.environment.value = word(0);
            entry.environment.calldata = ByteArray::empty();
            let mut config = ExecutionConfig {
                max_work: 100_000_000,
                ..ExecutionConfig::default()
            };
            config.analysis.relations.enabled = false;
            let analysis = analyze_world(world, entry, config).unwrap();
            assert_eq!(
                analysis.status(),
                Status::Converged,
                "{:?}",
                analysis.frontiers()
            );
            let store = analysis
                .outcomes()
                .iter()
                .filter(|outcome| outcome.kind == OutcomeKind::Return)
                .map(|outcome| &outcome.store)
                .find(|store| store.possible_logs().len() == 2)
                .expect("retain both actual LOG effects after deployment and CALL");
            assert!(!store.logs_unknown());
            let init_key = LogKey {
                address: child,
                code_address: child,
                code_hash: keccak256(&initcode),
                source_kind: LogSourceKind::Init,
                pc,
            };
            let runtime_key = LogKey {
                code_hash: keccak256(&runtime),
                source_kind: LogSourceKind::Runtime,
                ..init_key.clone()
            };
            let init_log = &store.possible_logs()[&init_key];
            let runtime_log = &store.possible_logs()[&runtime_key];
            assert!(init_log.topics.is_empty());
            assert_eq!(runtime_log.topics.len(), 1);
            assert_eq!(runtime_log.topics[0].singleton(), Some(U256::ZERO));
            assert_eq!(init_log.data.exact_bytes(), Some(init_data.clone()));
            assert_eq!(runtime_log.data.exact_bytes(), Some(runtime_data.clone()));
            assert_eq!(store.raw_account_code(child), Some(runtime.clone()));
            for replay in [
                store.clone(),
                store
                    .summary_projection(Domain::default())
                    .unwrap()
                    .instantiate(Domain::default())
                    .unwrap(),
            ] {
                assert!(replay.possible_logs().contains_key(&init_key));
                assert!(replay.possible_logs().contains_key(&runtime_key));
            }
            let native = serde_json::to_value(store).unwrap();
            let report = serde_json::to_value(render::world::evidence(&analysis).unwrap()).unwrap();
            for key in [&init_key, &runtime_key] {
                let expected = serde_json::to_value(key).unwrap();
                for logs in [
                    native["possible_logs"].as_array().unwrap(),
                    report["outcomes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|outcome| outcome["store"]["possible_logs"].as_array())
                        .find(|logs| logs.len() == 2)
                        .unwrap(),
                ] {
                    assert!(logs.iter().any(|log| log["address"] == expected["address"]
                        && log["code_address"] == expected["code_address"]
                        && log["code_hash"] == expected["code_hash"]
                        && log["source_kind"] == expected["source_kind"]
                        && log["pc"] == expected["pc"]));
                }
            }
            let text = render::world::text(&analysis);
            assert!(text.contains("source=Init"));
            assert!(text.contains("source=Runtime"));
            assert!(text.contains(&keccak256(&initcode).to_string()));
            assert!(text.contains(&keccak256(&runtime).to_string()));
        }
    }
}

#[test]
fn mode_and_captured_code_versions_separate_sites_while_same_source_aggregates() {
    let domain = Domain::default();
    let world = World::new(Fork::Osaka, "issue134:log-identity");
    let mut store = Store::new_checked(&world, domain).unwrap();
    let init = LogKey {
        address: address(0x200),
        code_address: address(0x300),
        code_hash: B256::repeat_byte(1),
        source_kind: LogSourceKind::Init,
        pc: 3,
    };
    let runtime = LogKey {
        source_kind: LogSourceKind::Runtime,
        ..init.clone()
    };
    let other_version = LogKey {
        code_hash: B256::repeat_byte(2),
        ..runtime.clone()
    };
    for (key, topics, data) in [
        (init.clone(), vec![], vec![0xaa]),
        (runtime.clone(), vec![word(1)], vec![1]),
        (runtime.clone(), vec![word(2)], vec![2]),
        (other_version.clone(), vec![word(3), word(4)], vec![0xbb]),
    ] {
        store
            .emit_log(
                key,
                AbstractLog {
                    topics,
                    data: ByteArray::exact(&data),
                },
                domain,
            )
            .unwrap();
    }
    assert_eq!(store.possible_logs().len(), 3);
    assert!(!store.logs_unknown());
    assert_eq!(store.possible_logs()[&init].topics.len(), 0);
    assert_eq!(store.possible_logs()[&other_version].topics.len(), 2);
    let combined = &store.possible_logs()[&runtime];
    for marker in [U256::ONE, U256::from(2)] {
        assert!(combined.topics[0].contains(marker));
        assert!(
            combined
                .data
                .read_byte_checked(&word(0), domain)
                .unwrap()
                .contains(marker)
        );
    }
    let replay = store
        .summary_projection(domain)
        .unwrap()
        .instantiate(domain)
        .unwrap();
    assert_eq!(
        replay.possible_logs().keys().collect::<Vec<_>>(),
        store.possible_logs().keys().collect::<Vec<_>>()
    );
    let original = store.clone();
    let mut other = Store::new_checked(&world, domain).unwrap();
    other
        .emit_log(
            other_version,
            AbstractLog {
                topics: vec![word(5), word(6)],
                data: ByteArray::exact(&[0xcc]),
            },
            domain,
        )
        .unwrap();
    let joined = original.join(&other, domain);
    assert_eq!(joined.possible_logs().len(), 3);
    assert!(!joined.logs_unknown());
}
