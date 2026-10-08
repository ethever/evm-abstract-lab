//! Fixed-world ownership, aliasing, rollback, sparse bytes, and resource bounds.

use evm_abstract::{
    Address, Fork, U256,
    domain::{AbstractValue, Domain, provenance::Origin},
    world::{
        AbstractLog, Account, ByteArray, Code, LogError, LogKey, RangeError, Store, World,
        WorldError,
    },
};

fn v(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn address(value: u8) -> Address {
    Address::from([value; 20])
}

// 字节搬运保留数值精度，同时增加运算来源；不能用常量来源替代完整 AbstractValue。
fn assert_numeric_eq(actual: &AbstractValue, expected: &AbstractValue) {
    assert_eq!(actual.constants(), expected.constants());
    assert_eq!(actual.known_bits(), expected.known_bits());
    assert_eq!(actual.interval(), expected.interval());
    assert_eq!(actual.congruence(), expected.congruence());
    assert_eq!(actual.may_be_zero(), expected.may_be_zero());
    assert_eq!(actual.may_be_nonzero(), expected.may_be_nonzero());
}

#[test]
fn fixed_world_distinguishes_missing_empty_and_delegated_code() {
    let mut world = World::new(Fork::Osaka, "block:0x1234");
    world.insert(address(1), Account::empty()).unwrap();
    let target = address(2);
    let delegated = Account::from_hex(
        &format!("ef0100{}", target.to_string().trim_start_matches("0x")),
        Fork::Osaka,
    )
    .unwrap();
    assert!(matches!(delegated.code, Code::Delegation(found) if found == target));
    world.insert(address(3), delegated).unwrap();
    assert!(world.account(address(4)).is_none());
    assert!(matches!(
        world.account(address(1)).unwrap().code,
        Code::Empty
    ));
    assert_eq!(world.raw_account_code(address(3)).unwrap().len(), 23);
    assert!(world.raw_account_code(address(4)).is_none());
    assert_eq!(world.provenance(), "block:0x1234");
}

#[test]
fn mixed_fork_insertion_is_rejected_without_changing_world() {
    let mut world = World::new(Fork::Cancun, "fixture");
    let account = Account::from_hex("00", Fork::Osaka).unwrap();
    assert!(matches!(
        world.insert(address(1), account),
        Err(WorldError::MixedFork { .. })
    ));
    assert!(world.accounts().is_empty());
    let mut delegated = Account::empty();
    delegated.code = Code::Delegation(address(2));
    assert!(matches!(
        world.insert(address(1), delegated),
        Err(WorldError::UnsupportedDelegation(Fork::Cancun))
    ));
}

#[test]
fn storage_defaults_and_account_ownership_are_explicit() {
    let domain = Domain::default();
    let mut world = World::new(Fork::Osaka, "fixture");
    world.insert(address(1), Account::empty()).unwrap();
    world.insert(address(2), Account::unknown()).unwrap();
    let mut store = Store::new(&world);
    assert_eq!(store.read(address(1), &v(7), domain), v(0));
    assert_eq!(store.read(address(2), &v(7), domain), AbstractValue::top());
    assert_eq!(store.read(address(3), &v(7), domain), AbstractValue::top());
    assert_eq!(store.read_transient(address(3), &v(7), domain), v(0));
    store.write(address(1), &v(7), &v(42), domain);
    assert_eq!(store.read(address(1), &v(7), domain), v(42));
    assert_eq!(store.read(address(2), &v(7), domain), AbstractValue::top());
}

#[test]
fn store_join_includes_missing_slots_defaults_and_balances() {
    let domain = Domain::default();
    let mut world = World::new(Fork::Osaka, "fixture");
    world.insert(address(1), Account::empty()).unwrap();
    let original = Store::new(&world);
    let mut changed = original.clone();
    changed.write(address(1), &v(7), &v(42), domain);
    changed.write_transient(address(1), &v(8), &v(19), domain);
    changed.write_balance(address(1), v(10));
    let joined = original.join(&changed, domain);
    assert_eq!(
        joined.read(address(1), &v(7), domain),
        domain.join(&v(0), &v(42))
    );
    assert_eq!(
        joined.read_transient(address(1), &v(8), domain),
        domain.join(&v(0), &v(19))
    );
    assert_eq!(joined.read_balance(address(1)), domain.join(&v(0), &v(10)));
    assert_eq!(joined, changed.join(&original, domain));
}

#[test]
fn unknown_slot_write_weakly_updates_all_aliases_and_defaults() {
    let domain = Domain::default();
    let mut world = World::new(Fork::Osaka, "fixture");
    world.insert(address(1), Account::empty()).unwrap();
    let mut store = Store::new(&world);
    store.write(address(1), &v(7), &v(42), domain);
    store.write(address(1), &AbstractValue::top(), &v(9), domain);
    assert_eq!(
        store.read(address(1), &v(7), domain),
        domain.join(&v(42), &v(9))
    );
    assert_eq!(
        store.read(address(1), &v(99), domain),
        domain.join(&v(0), &v(9))
    );
    let finite_alias = domain.join(&v(7), &v(8));
    store.write(address(1), &finite_alias, &v(1), domain);
    assert!(
        store
            .read(address(1), &v(7), domain)
            .contains(U256::from(42))
    );
    assert!(store.read(address(1), &v(8), domain).contains(U256::ZERO));
    assert!(
        store
            .read(address(1), &v(8), domain)
            .contains(U256::from(1))
    );
}

#[test]
fn snapshot_restores_storage_transient_state_and_balance() {
    let domain = Domain::default();
    let mut world = World::new(Fork::Osaka, "fixture");
    world.insert(address(1), Account::empty()).unwrap();
    let mut store = Store::new(&world);
    let original = store.clone();
    let snapshot = store.snapshot();
    store.write(address(1), &v(0), &v(42), domain);
    store.write_transient(address(1), &v(0), &v(7), domain);
    store.write_balance(address(1), v(99));
    store.restore(snapshot);
    assert_eq!(store, original);
}

#[test]
fn store_json_serializes_account_slot_observations() {
    let domain = Domain::default();
    let world = World::new(Fork::Osaka, "fixture");
    let mut store = Store::new(&world);
    store.write(address(1), &v(7), &v(42), domain);
    store.write_transient(address(2), &v(8), &v(19), domain);
    let json = serde_json::to_value(store).unwrap();
    assert_eq!(json["persistent"]["slots"].as_array().unwrap().len(), 1);
    assert_eq!(json["transient"]["slots"].as_array().unwrap().len(), 1);
}

#[test]
fn words_are_big_endian_and_calldata_is_zero_padded() {
    let domain = Domain::default();
    let bytes = ByteArray::exact(&[0x12, 0x34]);
    let loaded = bytes.read_word(&v(0), domain);
    assert_numeric_eq(&loaded, &AbstractValue::constant(U256::from(0x1234) << 240));
    assert!(
        loaded
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Arithmetic)
    );
    assert_numeric_eq(&bytes.read_word(&v(2), domain), &v(0));
    assert_numeric_eq(
        &bytes.read_word(&AbstractValue::constant(U256::MAX), domain),
        &v(0),
    );
    assert_numeric_eq(
        &bytes.read_word(&AbstractValue::constant(U256::from(usize::MAX)), domain),
        &v(0),
    );
    assert_eq!(
        bytes
            .slice(
                &AbstractValue::constant(U256::from(usize::MAX)),
                &v(2),
                64,
                domain
            )
            .unwrap()
            .exact_bytes(),
        Some(vec![0, 0])
    );
    assert_eq!(
        bytes.slice(&v(1), &v(3), 64, domain).unwrap().exact_bytes(),
        Some(vec![0x34, 0, 0])
    );
    assert_eq!(
        ByteArray::unknown().read_word(&v(0), domain),
        AbstractValue::top()
    );
}

#[test]
fn fresh_memory_zeroes_word_writes_and_mstore8_truncates() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    assert_numeric_eq(&memory.read_word(&v(100), domain), &v(0));
    memory.write_word(&v(1), &v(0x1234), 64, domain).unwrap();
    assert_eq!(memory.len(), &v(64));
    assert_numeric_eq(&memory.read_word(&v(1), domain), &v(0x1234));
    memory.write_byte(&v(0), &v(0x12ff), 64, domain).unwrap();
    assert_numeric_eq(&memory.byte_at(0, domain), &v(255));
    assert!(
        memory
            .byte_at(0, domain)
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Arithmetic)
    );
}

#[test]
fn byte_join_includes_zero_from_missing_keys() {
    let domain = Domain::default();
    let zero = ByteArray::exact(&[0]);
    let nonzero = ByteArray::exact(&[42]);
    let joined = zero.join(&nonzero, domain);
    assert_eq!(joined.byte_at(0, domain), domain.join(&v(0), &v(42)));
    assert_eq!(joined.len(), &v(1));
    let short = ByteArray::empty();
    let variable_length = short.join(&nonzero, domain);
    assert!(variable_length.byte_at(0, domain).contains(U256::ZERO));
    assert!(variable_length.byte_at(0, domain).contains(U256::from(42)));
}

#[test]
fn finite_write_aliases_preserve_untouched_possibilities() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    let offset = domain.join(&v(0), &v(1));
    memory.write_byte(&offset, &v(42), 32, domain).unwrap();
    assert_numeric_eq(&memory.byte_at(0, domain), &domain.join(&v(0), &v(42)));
    assert_numeric_eq(&memory.byte_at(1, domain), &domain.join(&v(0), &v(42)));
    assert_numeric_eq(&memory.byte_at(2, domain), &v(0));
}

#[test]
fn copy_return_data_preserves_uncopied_suffix_and_expands_output_range() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory
        .copy_from(
            &v(0),
            &ByteArray::exact(&[1, 2, 3, 4]),
            &v(0),
            &v(4),
            64,
            domain,
        )
        .unwrap();
    memory
        .copy_return_data(&v(1), &ByteArray::exact(&[9]), &v(3), 64, domain)
        .unwrap();
    assert_eq!(
        memory
            .slice(&v(0), &v(4), 64, domain)
            .unwrap()
            .exact_bytes(),
        Some(vec![1, 9, 3, 4])
    );
    memory
        .copy_return_data(&v(32), &ByteArray::empty(), &v(1), 64, domain)
        .unwrap();
    assert_eq!(memory.len(), &v(64));
}

#[test]
fn overlapping_copy_uses_source_snapshot() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory
        .copy_from(
            &v(0),
            &ByteArray::exact(&[1, 2, 3, 4]),
            &v(0),
            &v(4),
            32,
            domain,
        )
        .unwrap();
    let original = memory.clone();
    memory
        .copy_from(&v(1), &original, &v(0), &v(3), 32, domain)
        .unwrap();
    assert_eq!(
        memory
            .slice(&v(0), &v(4), 32, domain)
            .unwrap()
            .exact_bytes(),
        Some(vec![1, 1, 2, 3])
    );
}

#[test]
fn range_limits_are_typed_atomic_and_allow_zero_size_at_any_offset() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    let original = memory.clone();
    assert!(matches!(
        memory.write_word(&v(1), &v(42), 32, domain),
        Err(RangeError::TooLarge { .. })
    ));
    assert_eq!(memory, original);
    assert!(matches!(
        memory.slice(&v(0), &AbstractValue::top(), 32, domain),
        Err(RangeError::UnknownSize)
    ));
    assert!(matches!(
        memory.slice(&v(0), &AbstractValue::constant(U256::MAX), 32, domain),
        Err(RangeError::TooLarge { .. })
    ));
    memory
        .expand(&AbstractValue::constant(U256::MAX), &v(0), 32, domain)
        .unwrap();
    assert_eq!(memory, original);
    assert_eq!(
        memory
            .slice(&AbstractValue::constant(U256::MAX), &v(0), 32, domain)
            .unwrap(),
        ByteArray::empty()
    );
}

#[test]
fn unknown_memory_offset_is_conservative_without_allocating_a_range() {
    let domain = Domain::default();
    let mut memory = ByteArray::memory();
    memory
        .write_byte(&AbstractValue::top(), &v(42), 32, domain)
        .unwrap();
    assert_eq!(memory.len(), &AbstractValue::top());
    assert_eq!(memory.read_word(&v(0), domain), AbstractValue::top());
}

fn log_key() -> LogKey {
    LogKey {
        address: address(1),
        code_address: address(2),
        pc: 17,
    }
}

#[test]
fn event_summary_joins_site_visits_and_retains_possibility_on_missing_path() {
    let domain = Domain::default();
    let world = World::new(Fork::Osaka, "fixture");
    let original = Store::new(&world);
    let mut emitted = original.clone();
    emitted
        .emit_log(
            log_key(),
            AbstractLog {
                topics: vec![v(7)],
                data: ByteArray::exact(&[1]),
            },
            domain,
        )
        .unwrap();
    emitted
        .emit_log(
            log_key(),
            AbstractLog {
                topics: vec![v(9)],
                data: ByteArray::exact(&[3]),
            },
            domain,
        )
        .unwrap();
    let joined = original.join(&emitted, domain);
    let event = &joined.possible_logs()[&log_key()];
    assert_eq!(event.topics, vec![domain.join(&v(7), &v(9))]);
    assert_eq!(event.data.byte_at(0, domain), domain.join(&v(1), &v(3)));
    assert_eq!(joined, emitted.join(&original, domain));
    assert_eq!(joined, joined.join(&joined, domain));
    assert!(joined.work_size() > original.work_size());
    assert!(!joined.logs_unknown());
    assert_eq!(
        serde_json::to_value(&joined).unwrap()["possible_logs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn event_rollback_and_unknown_external_events_are_transaction_scoped() {
    let domain = Domain::default();
    let world = World::new(Fork::Osaka, "fixture");
    let mut store = Store::new(&world);
    let snapshot = store.snapshot();
    store
        .emit_log(
            log_key(),
            AbstractLog {
                topics: vec![],
                data: ByteArray::empty(),
            },
            domain,
        )
        .unwrap();
    store.havoc_all();
    assert!(store.logs_unknown());
    store.restore(snapshot);
    assert!(store.possible_logs().is_empty());
    assert!(!store.logs_unknown());
}

#[test]
fn event_topic_count_validation_is_typed_and_atomic() {
    let domain = Domain::default();
    let world = World::new(Fork::Osaka, "fixture");
    let mut store = Store::new(&world);
    store
        .emit_log(
            log_key(),
            AbstractLog {
                topics: vec![v(7)],
                data: ByteArray::empty(),
            },
            domain,
        )
        .unwrap();
    let original = store.clone();
    assert!(matches!(
        store.emit_log(
            log_key(),
            AbstractLog {
                topics: vec![],
                data: ByteArray::empty()
            },
            domain
        ),
        Err(LogError::InconsistentTopics { .. })
    ));
    assert!(matches!(
        store.emit_log(
            log_key(),
            AbstractLog {
                topics: vec![v(0); 5],
                data: ByteArray::empty()
            },
            domain
        ),
        Err(LogError::TooManyTopics(5))
    ));
    assert_eq!(store, original);
}
