//! Native byte/word source facts remain complete in checked snapshot identities.

use crate::{
    Fork,
    domain::{
        AbstractValue,
        provenance::Origin,
        symbolic::{ExprId, ExprLimits},
    },
    world::{
        Account, Code, Existence, FingerprintError, SnapshotEnvironment, SnapshotIdentity, World,
    },
};
use alloy_primitives::{Address, B256, U256, keccak256};
use revm_bytecode::opcode;

// Reference the pre-streaming encoding exactly, using only small valid fixtures.
fn legacy_fingerprint(world: &World) -> B256 {
    fn length(bytes: &mut Vec<u8>, count: usize) {
        bytes.extend_from_slice(&(count as u64).to_be_bytes());
    }
    fn scalar(bytes: &mut Vec<u8>, value: &AbstractValue) {
        let encoded = serde_json::to_vec(value).unwrap();
        length(bytes, encoded.len());
        bytes.extend(encoded);
    }
    let mut bytes = b"evm-abstract-world-v2".to_vec();
    bytes.push(match world.fork() {
        Fork::Cancun => 0,
        Fork::Prague => 1,
        Fork::Osaka => 2,
    });
    match world.identity() {
        SnapshotIdentity::Offline { label } => {
            bytes.push(0);
            length(&mut bytes, label.len());
            bytes.extend(label.as_bytes());
        }
        SnapshotIdentity::Chain {
            chain_id,
            block_hash,
        } => {
            bytes.push(1);
            bytes.extend(chain_id.to_be_bytes::<32>());
            bytes.extend(block_hash.as_slice());
        }
    }
    length(&mut bytes, world.accounts().len());
    for (address, account) in world.accounts() {
        bytes.extend(address.as_slice());
        bytes.push(match account.existence {
            Existence::Unknown => 0,
            Existence::Present => 1,
            Existence::Absent => 2,
        });
        match crate::world::raw_code(&account.code) {
            Some(code) => {
                bytes.push(1);
                length(&mut bytes, code.len());
                bytes.extend(code);
            }
            None => bytes.push(0),
        }
        scalar(&mut bytes, &account.balance);
        scalar(&mut bytes, &account.nonce);
        bytes.push(u8::from(account.storage_unknown));
        length(&mut bytes, account.storage.len());
        for (slot, value) in &account.storage {
            bytes.extend(slot.to_be_bytes::<32>());
            scalar(&mut bytes, value);
        }
    }
    if let Some(environment) = world.snapshot_environment() {
        bytes.extend(b"rpc-environment-v1");
        bytes.extend(serde_json::to_vec(environment).unwrap());
    }
    keccak256(bytes)
}

fn fixture(fork: Fork, anchored: bool) -> World {
    let mut world = if anchored {
        World::anchored(
            fork,
            U256::MAX,
            B256::repeat_byte(0x17),
            "fingerprint observations",
        )
    } else {
        World::offline(fork, "fixture 中文 \" quoted", "fingerprint observations")
    };
    let mut account = Account::from_hex("600160025500", fork).unwrap();
    let word = AbstractValue::unsigned_range(U256::ONE, U256::from(9))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap())
        .with_origin(Origin::Storage);
    account.storage_unknown = true;
    account.balance = word.clone();
    account.storage.insert(U256::from(9), word);
    account
        .storage
        .insert(U256::from(3), AbstractValue::constant(U256::from(7)));
    world.insert(Address::repeat_byte(0x11), account).unwrap();
    let mut delegated = Account::empty();
    if matches!(fork, Fork::Prague | Fork::Osaka) {
        delegated.code = Code::Delegation(Address::repeat_byte(0x22));
    }
    world.insert(Address::repeat_byte(0x12), delegated).unwrap();
    world
        .insert(Address::repeat_byte(0x13), Account::unknown())
        .unwrap();
    world
        .insert(Address::repeat_byte(0x14), Account::absent())
        .unwrap();
    world.snapshot_environment = Some(SnapshotEnvironment {
        number: U256::from(23),
        parent_hash: B256::repeat_byte(0x24),
        timestamp: U256::from(25),
        coinbase: Address::repeat_byte(0x26),
        prevrandao: B256::repeat_byte(0x27),
        gas_limit: U256::from(28),
        base_fee: Some(U256::from(29)),
        blob_base_fee: None,
        excess_blob_gas: Some(U256::from(30)),
        blob_gas_used: Some(U256::from(31)),
    });
    world
}

#[test]
fn streaming_fingerprint_preserves_legacy_encoding_and_full_symbolic_facts() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for anchored in [false, true] {
            let world = fixture(fork, anchored);
            let expected = legacy_fingerprint(&world);
            assert_eq!(world.fingerprint().unwrap(), expected);
            assert_eq!(world.clone().fingerprint().unwrap(), expected);
            assert_eq!(world.fingerprint().unwrap(), expected);
            let mut no_environment = world.clone();
            no_environment.snapshot_environment = None;
            assert_eq!(
                no_environment.fingerprint().unwrap(),
                legacy_fingerprint(&no_environment)
            );
            assert_ne!(no_environment.fingerprint().unwrap(), expected);
        }
    }
}

#[test]
fn fingerprint_binds_numeric_guarantees_and_distinct_source_identity() {
    let world = fixture(Fork::Osaka, true);
    let expected = world.fingerprint().unwrap();
    let address = Address::repeat_byte(0x11);
    let mut changed = world.clone();
    let account = changed.accounts.get_mut(&address).unwrap();
    account.balance = account.balance.clone().with_numeric(
        AbstractValue::unsigned_range(U256::ONE, U256::from(8))
            .unwrap()
            .numeric()
            .clone(),
    );
    assert_ne!(changed.fingerprint().unwrap(), expected);
    let mut changed = world.clone();
    let account = changed.accounts.get_mut(&address).unwrap();
    account.balance = account
        .balance
        .clone()
        .with_expression(ExprId::fresh().unwrap());
    assert_ne!(changed.fingerprint().unwrap(), expected);
    let mut changed = world.clone();
    changed.accounts.get_mut(&address).unwrap().nonce = AbstractValue::constant(U256::ONE);
    assert_ne!(changed.fingerprint().unwrap(), expected);
}

#[test]
fn fingerprint_cancel_is_explicit_and_does_not_mutate_observations() {
    let world = fixture(Fork::Osaka, true);
    let expected = world.fingerprint().unwrap();
    let before = serde_json::to_value(&world).unwrap();
    let cancellation = embedded_smt::Cancellation::new();
    cancellation.cancel();
    embedded_smt::with_cancellation(&cancellation, || {
        assert!(matches!(
            world.fingerprint(),
            Err(FingerprintError::Cancelled)
        ));
    });
    assert_eq!(serde_json::to_value(&world).unwrap(), before);
    assert_eq!(world.fingerprint().unwrap(), expected);
}

#[test]
fn fingerprint_rejected_native_graph_is_typed_encoding_error_without_panic() {
    let policy = ExprLimits {
        max_nodes: 2000,
        max_depth: 400,
    };
    let mut expression = ExprId::fresh().unwrap();
    for _ in 0..300 {
        expression =
            ExprId::operation(opcode::ADD, &[expression, ExprId::fresh().unwrap()], policy)
                .unwrap();
    }
    let mut world = World::new(Fork::Osaka, "oversized native scalar graph");
    let mut account = Account::unknown();
    account.balance = AbstractValue::top().with_expression(expression.clone());
    world.insert(Address::repeat_byte(0x31), account).unwrap();
    assert!(matches!(
        world.fingerprint(),
        Err(FingerprintError::Encoding(_))
    ));
    assert_eq!(
        world.accounts()[&Address::repeat_byte(0x31)]
            .balance
            .expression(),
        Some(&expression)
    );
}
