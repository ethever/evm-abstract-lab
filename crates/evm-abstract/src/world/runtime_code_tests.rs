//! Account JSON preserves captured code without repeating its decoded listing.

use super::*;
use crate::analysis::{EvmFrameState, FrameCode, FrameKey};
use crate::world::test_support::{TestAccount, TestBytes, TestSnapshot, TestStore, TestWorld};
type TestFrame = EvmFrameState<crate::domain::NumericValue>;
use serde_json::{Value, json};

#[test]
fn runtime_json_round_trips_all_raw_bytes_under_each_supported_fork() {
    let all_bytes: Vec<_> = (0..=u8::MAX).collect();
    // STOP leaves unreachable invalid code and a final truncated PUSH2 behind.
    let trailing = [0x00, 0xfe, 0x5b, 0x1e, 0x61, 0xab];
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for bytes in [&[][..], &all_bytes, &trailing] {
            let program = Program::decode_with_fork(bytes, fork).unwrap();
            let code = Code::Runtime(program.clone());
            let json = serde_json::to_value(&code).unwrap();
            assert_eq!(json.as_object().unwrap().len(), 1);
            let runtime = &json["Runtime"];
            assert_eq!(runtime.as_object().unwrap().len(), 3);
            assert_eq!(runtime["fork"], fork.to_string());
            assert_eq!(runtime["byte_len"], bytes.len());
            assert!(runtime.get("blocks").is_none());
            let bytecode = runtime["bytecode"].as_str().unwrap();
            assert!(bytecode.starts_with("0x"));
            assert_eq!(bytecode.len(), 2 + 2 * bytes.len());
            assert_eq!(hex::decode(bytecode).unwrap(), bytes);
            let decoded = Program::from_hex_with_fork(
                bytecode,
                runtime["fork"].as_str().unwrap().parse().unwrap(),
            )
            .unwrap();
            assert_eq!(decoded, program);
            assert_eq!(Code::Runtime(decoded), code);
        }
    }
}

#[test]
fn other_code_variants_and_direct_program_fields_keep_their_native_contract() {
    let address = Address::repeat_byte(0x31);
    assert_eq!(serde_json::to_value(Code::Empty).unwrap(), json!("Empty"));
    assert_eq!(
        serde_json::to_value(Code::Unknown).unwrap(),
        json!("Unknown")
    );
    assert_eq!(
        serde_json::to_value(Code::Delegation(address)).unwrap(),
        json!({"Delegation": address})
    );
    let marker = format!("0xef0100{}", hex::encode(address.as_slice()));
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        assert_eq!(TestAccount::from_hex("", fork).unwrap().code, Code::Empty);
        assert!(matches!(
            TestAccount::from_hex("ef0001", fork),
            Err(DecodeError::UnsupportedEof)
        ));
        if fork == Fork::Cancun {
            assert!(matches!(
                TestAccount::from_hex(&marker, fork).unwrap().code,
                Code::Runtime(_)
            ));
        } else {
            assert_eq!(
                TestAccount::from_hex(&marker, fork).unwrap().code,
                Code::Delegation(address)
            );
            assert!(matches!(
                Program::from_hex_with_fork(&marker, fork),
                Err(DecodeError::DelegatedCode { address: target }) if target == address
            ));
            assert!(matches!(
                TestAccount::from_hex("ef01", fork),
                Err(DecodeError::InvalidDelegation(_))
            ));
        }
    }

    let program = Program::from_hex("602a005bfe61ab").unwrap();
    let listing = serde_json::to_value(&program).unwrap();
    assert_eq!(listing.as_object().unwrap().len(), 3);
    assert_eq!(listing["fork"], "osaka");
    assert_eq!(listing["byte_len"], 7);
    assert_eq!(listing["blocks"][0]["instructions"][0]["opcode"], 0x60);
    assert!(listing.get("bytecode").is_none());
    // The single-program envelope uses this same direct Program serializer.
    assert_eq!(json!({"program": &program})["program"], listing);

    let mut world = TestWorld::new(program.fork(), "direct Program serialization");
    world
        .insert(
            address,
            TestAccount {
                code: Code::Runtime(program.clone()),
                ..TestAccount::empty()
            },
        )
        .unwrap();
    let frame = TestFrame {
        key: FrameKey {
            code_address: address,
            code_hash: keccak256(program.bytes()),
            mode: FrameCode::Runtime,
            address,
            address_value: AddressInput::Concrete(address),
            caller: AddressInput::Concrete(address),
            is_static: false,
            basic_block_index: 0,
            stack_height: 0,
            jump_history: Vec::new(),
        },
        code: FrameCode::Runtime,
        program: Some(program),
        stack: Vec::new(),
        memory: TestBytes::memory(),
        calldata: TestBytes::exact(&[]),
        environment_calldata: true,
        returndata: TestBytes::exact(&[]),
        call_value: TestValue::constant(U256::ZERO),
        address_operand: None,
        caller_operand: None,
        saved_store: TestStore::new(
            &world,
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
        )
        .snapshot(),
    };
    let json = serde_json::to_value(frame).unwrap();
    assert_eq!(json["program"], listing);
    let address_key = serde_json::to_value(address).unwrap();
    let runtime = &json["saved_store"]["codes"][address_key.as_str().unwrap()]["Runtime"];
    assert_eq!(runtime["bytecode"], "0x602a005bfe61ab");
    assert!(runtime.get("blocks").is_none());
}

#[test]
fn store_history_keeps_every_code_version_with_raw_hex_sized_bodies() {
    #[derive(Serialize)]
    struct History<'a> {
        world: &'a TestWorld,
        saved_stores: [&'a TestSnapshot; 2],
        store: &'a TestStore,
    }

    let addresses = [0x31, 0x32, 0x33, 0x34].map(Address::repeat_byte);
    let bytes = [0x5b, 0x60, 0x01, 0x50].repeat(16);
    let original = Program::decode(&bytes).unwrap();
    let mut world = TestWorld::new(original.fork(), "captured code versions");
    for address in addresses {
        world
            .insert(
                address,
                TestAccount {
                    code: Code::Runtime(original.clone()),
                    ..TestAccount::empty()
                },
            )
            .unwrap();
    }
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let before = store.snapshot();
    let mut middle_bytes = bytes.clone();
    middle_bytes.extend([0x00, 0x61, 0xab]);
    let middle = Program::decode(&middle_bytes).unwrap();
    store.begin_creation(addresses[0]);
    store.deploy_code(addresses[0], middle.clone());
    let deployed = store.snapshot();
    let mut latest_bytes = bytes;
    latest_bytes.extend([0xfe, 0x7f, 0xcd]);
    let latest = Program::decode(&latest_bytes).unwrap();
    store.deploy_code(addresses[0], latest.clone());
    assert_eq!(
        before.state().code(addresses[0]),
        Some(&Code::Runtime(original.clone()))
    );
    assert_eq!(
        deployed.state().code(addresses[0]),
        Some(&Code::Runtime(middle))
    );
    assert_eq!(store.code(addresses[0]), Some(&Code::Runtime(latest)));
    assert_ne!(before.state().code(addresses[0]), store.code(addresses[0]));

    let compact = serde_json::to_vec(&History {
        world: &world,
        saved_stores: [&before, &deployed],
        store: &store,
    })
    .unwrap();
    let json: Value = serde_json::from_slice(&compact).unwrap();
    assert_eq!(json["world"]["accounts"].as_object().unwrap().len(), 4);
    assert_eq!(json["saved_stores"].as_array().unwrap().len(), 2);
    let mut versions = Vec::new();
    for (address, account) in world.accounts() {
        let Code::Runtime(program) = &account.code else {
            panic!("fixture must retain each runtime");
        };
        let key = serde_json::to_value(address).unwrap();
        versions.push((
            format!("/world/accounts/{}/code/Runtime", key.as_str().unwrap()),
            program,
        ));
    }
    for (path, captured) in [
        ("/saved_stores/0", before.state()),
        ("/saved_stores/1", deployed.state()),
        ("/store", &store),
    ] {
        assert_eq!(
            json.pointer(&format!("{path}/codes"))
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            addresses.len()
        );
        for address in addresses {
            let Some(Code::Runtime(program)) = captured.code(address) else {
                panic!("fixture must retain each captured runtime");
            };
            let key = serde_json::to_value(address).unwrap();
            versions.push((
                format!("{path}/codes/{}/Runtime", key.as_str().unwrap()),
                program,
            ));
        }
    }

    // Reinsert the unchanged public Program listing into the actual document
    // to measure the previous expansion, without a second Store serializer.
    let mut expanded = json.clone();
    let mut raw_bytes = 0;
    let mut runtime_bytes = 0;
    let mut metadata_bytes = 0;
    let mut removed_bytes = 0;
    for (path, program) in &versions {
        let runtime = json.pointer(path).unwrap();
        assert_eq!(runtime.as_object().unwrap().len(), 3);
        assert!(runtime.get("blocks").is_none());
        let restored = Program::from_hex_with_fork(
            runtime["bytecode"].as_str().unwrap(),
            runtime["fork"].as_str().unwrap().parse().unwrap(),
        )
        .unwrap();
        assert_eq!(&restored, *program);
        assert_eq!(runtime["byte_len"], program.byte_len());
        let body_bytes = serde_json::to_vec(runtime).unwrap().len();
        let metadata = br#"{"fork":"","byte_len":,"bytecode":"0x"}"#.len()
            + program.fork().to_string().len()
            + program.byte_len().to_string().len();
        assert_eq!(body_bytes, 2 * program.byte_len() + metadata);
        assert!(metadata <= 64);
        raw_bytes += program.byte_len();
        runtime_bytes += body_bytes;
        metadata_bytes += metadata;
        removed_bytes += serde_json::to_vec(program).unwrap().len() - body_bytes;
        *expanded.pointer_mut(path).unwrap() = serde_json::to_value(program).unwrap();
    }
    let expanded_bytes = serde_json::to_vec(&expanded).unwrap().len();
    assert_eq!(versions.len(), 4 * addresses.len());
    assert_eq!(runtime_bytes, 2 * raw_bytes + metadata_bytes);
    assert!(runtime_bytes <= 2 * raw_bytes + 64 * versions.len());
    assert_eq!(expanded_bytes, compact.len() + removed_bytes);
    assert!(expanded_bytes > 2 * compact.len());
    println!(
        "runtime_code_history: versions={} raw_bytes={raw_bytes} \
         metadata_bytes={metadata_bytes} runtime_bytes={runtime_bytes} \
         compact_bytes={} decoded_expanded_bytes={expanded_bytes}",
        versions.len(),
        compact.len()
    );

    let mut rolled_back = store;
    rolled_back.restore(before.clone());
    assert_eq!(&rolled_back, before.state());
    assert_eq!(
        serde_json::to_value(&rolled_back).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
}
