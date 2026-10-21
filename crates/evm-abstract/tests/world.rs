//! Fixed-world ownership, aliasing, rollback, sparse bytes, and resource bounds.

#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Address, Fork, U256,
    domain::{
        EvmValue,
        provenance::Origin,
        relational::{CheckResult, EvmValueQuery, RelationLimits, RelationState},
        symbolic::{ExprId, ExprKind, SlotStateKind},
    },
    world::{
        Code, EvmAccount, EvmByteArray, EvmLog, EvmStore, EvmWorld, LogError, LogKey, RangeError,
        WorldError,
    },
};

fn v(value: u64) -> EvmValue<NumericValue> {
    EvmValue::<NumericValue>::constant(U256::from(value))
}

fn address(value: u8) -> Address {
    Address::from([value; 20])
}

// 字节搬运保留数值精度，同时增加运算来源；不能用常量来源替代完整 AbstractValue。
fn assert_numeric_eq(actual: &EvmValue<NumericValue>, expected: &EvmValue<NumericValue>) {
    assert_eq!(actual.constants(), expected.constants());
    assert_eq!(actual.known_bits(), expected.known_bits());
    assert_eq!(actual.interval(), expected.interval());
    assert_eq!(actual.congruence(), expected.congruence());
    assert_eq!(actual.may_be_zero(), expected.may_be_zero());
    assert_eq!(actual.may_be_nonzero(), expected.may_be_nonzero());
}

// State reads add a historical expression; retain every pre-existing value fact.
fn assert_state_value_eq(actual: &EvmValue<NumericValue>, expected: &EvmValue<NumericValue>) {
    let mut projected = actual.clone();
    projected.forget_expression();
    assert_eq!(&projected, expected);
    assert!(!actual.symbolic_limit_reached());
    if actual.singleton().is_none() {
        assert!(actual.expression().is_some());
    }
}

fn local_policy() -> RelationLimits {
    RelationLimits {
        max_nodes: 100_000,
        max_depth: 256,
        ..RelationLimits::default()
    }
}

#[test]
fn fixed_world_distinguishes_missing_empty_and_delegated_code() {
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "block:0x1234");
    world
        .insert(address(1), EvmAccount::<NumericValue>::empty())
        .unwrap();
    let target = address(2);
    let delegated = EvmAccount::<NumericValue>::from_hex(
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
    let mut world = EvmWorld::<NumericValue>::new(Fork::Cancun, "fixture");
    let account = EvmAccount::<NumericValue>::from_hex("00", Fork::Osaka).unwrap();
    assert!(matches!(
        world.insert(address(1), account),
        Err(WorldError::MixedFork { .. })
    ));
    assert!(world.accounts().is_empty());
    let mut delegated = EvmAccount::<NumericValue>::empty();
    delegated.code = Code::Delegation(address(2));
    assert!(matches!(
        world.insert(address(1), delegated),
        Err(WorldError::UnsupportedDelegation(Fork::Cancun))
    ));
}

#[test]
fn storage_defaults_and_account_ownership_are_explicit() {
    let domain = native::product();
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "fixture");
    world
        .insert(address(1), EvmAccount::<NumericValue>::empty())
        .unwrap();
    world
        .insert(address(2), EvmAccount::<NumericValue>::unknown())
        .unwrap();
    let mut store = EvmStore::<NumericValue>::new(&world, native::product().borrowed());
    assert_eq!(store.read(address(1), &v(7), domain.borrowed()), v(0));
    let account_two = store.read(address(2), &v(7), domain.borrowed());
    let missing = store.read(address(3), &v(7), domain.borrowed());
    assert_state_value_eq(&account_two, &EvmValue::<NumericValue>::top());
    assert_state_value_eq(&missing, &EvmValue::<NumericValue>::top());
    assert_ne!(account_two.expression(), missing.expression());
    assert_eq!(
        store.read_transient(address(3), &v(7), domain.borrowed()),
        v(0)
    );
    store.write(address(1), &v(7), &v(42), domain.borrowed());
    assert_eq!(store.read(address(1), &v(7), domain.borrowed()), v(42));
    let unchanged = store.read(address(2), &v(7), domain.borrowed());
    assert_state_value_eq(&unchanged, &EvmValue::<NumericValue>::top());
    assert_eq!(unchanged.expression(), account_two.expression());
}

#[test]
fn store_join_includes_missing_slots_defaults_and_balances() {
    let domain = native::product();
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "fixture");
    world
        .insert(address(1), EvmAccount::<NumericValue>::empty())
        .unwrap();
    let original = EvmStore::<NumericValue>::new(&world, native::product().borrowed());
    let mut changed = original.clone();
    changed.write(address(1), &v(7), &v(42), domain.borrowed());
    changed.write_transient(address(1), &v(8), &v(19), domain.borrowed());
    changed.write_balance(address(1), v(10));
    let joined = original.join(&changed, domain.borrowed());
    assert_state_value_eq(
        &joined.read(address(1), &v(7), domain.borrowed()),
        &domain.join(&v(0), &v(42)),
    );
    assert_state_value_eq(
        &joined.read_transient(address(1), &v(8), domain.borrowed()),
        &domain.join(&v(0), &v(19)),
    );
    assert_state_value_eq(
        &joined.read_balance(address(1)),
        &domain.join(&v(0), &v(10)),
    );
    let reversed = changed.join(&original, domain.borrowed());
    assert_eq!(joined.addresses(), reversed.addresses());
    assert_eq!(joined.slots(), reversed.slots());
    assert_eq!(joined.transient_slots(), reversed.transient_slots());
    let independent = original.join(&changed, domain.borrowed());
    let independent_read = independent.read(address(1), &v(7), domain.borrowed());
    let unrelated_source = EvmStore::<NumericValue>::new(
        &EvmWorld::<NumericValue>::new(Fork::Osaka, "fixture"),
        native::product().borrowed(),
    )
    .read(address(2), &v(7), domain.borrowed());
    let unrelated_selector = ExprId::fresh().unwrap();
    // Independent union selectors need not be syntactically equal. Both orders
    // must have the same complete alternatives, with one selector across planes.
    for candidate in [&joined, &reversed] {
        let policy = local_policy();
        let persistent = candidate.read(address(1), &v(7), domain.borrowed());
        let transient = candidate.read_transient(address(1), &v(8), domain.borrowed());
        let balance = candidate.read_balance(address(1));
        assert_state_value_eq(&persistent, &domain.join(&v(0), &v(42)));
        assert_state_value_eq(&transient, &domain.join(&v(0), &v(19)));
        assert_state_value_eq(&balance, &domain.join(&v(0), &v(10)));
        assert_state_value_eq(
            &candidate.read(address(1), &v(99), domain.borrowed()),
            &v(0),
        );
        for owner in [address(1), address(2)] {
            for input in [&original, &changed] {
                assert_eq!(candidate.code(owner), input.code(owner));
                assert_eq!(candidate.existence(owner), input.existence(owner));
                assert_eq!(candidate.nonce(owner), input.nonce(owner));
            }
        }
        let ExprKind::Ite {
            condition,
            then_value,
            else_value,
        } = balance.expression().unwrap().kind()
        else {
            panic!("the joined balance must retain its branch selector");
        };
        let then_balance = then_value.as_constant().unwrap();
        let else_balance = else_value.as_constant().unwrap();
        assert!(
            (then_balance == U256::ZERO && else_balance == U256::from(10))
                || (then_balance == U256::from(10) && else_balance == U256::ZERO)
        );
        let (then_input, else_input) = if then_balance == U256::ZERO {
            (&original, &changed)
        } else {
            (&changed, &original)
        };
        for plane in [false, true] {
            let root = candidate.slot_root(address(1), plane).unwrap();
            let SlotStateKind::Ite {
                condition: slot_condition,
                then_state,
                else_state,
            } = root.kind()
            else {
                panic!("both storage planes must retain the whole-store alternatives");
            };
            assert_eq!(slot_condition, condition);
            assert_eq!(
                then_state,
                &then_input.slot_root(address(1), plane).unwrap()
            );
            assert_eq!(
                else_state,
                &else_input.slot_root(address(1), plane).unwrap()
            );
            assert_eq!(
                candidate.slot_root(address(2), plane).unwrap(),
                original.slot_root(address(2), plane).unwrap()
            );
        }
        let values = [&persistent, &transient, &balance];
        let alternatives = [[0_u64, 42], [0, 19], [0, 10]];
        let assert_unselected = |relations: &RelationState| {
            assert_eq!(
                relations.check(
                    &policy,
                    native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
                ),
                CheckResult::MayBeFeasible
            );
            for (value, possible) in values.iter().zip(alternatives) {
                let expression = value.expression().unwrap();
                let EvmValueQuery::<NumericValue>::Bounds(bounds) = relations.local_value(
                    expression,
                    &policy,
                    native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed(),
                ) else {
                    panic!("an undetermined selector must retain both store alternatives");
                };
                for number in possible {
                    assert!(bounds.contains(U256::from(number)));
                    assert_eq!(
                        relations.can_equal(
                            expression,
                            U256::from(number),
                            &policy,
                            native::product_with(native::spec(8, 4, 256).with_relations(policy))
                                .borrowed()
                        ),
                        CheckResult::MayBeFeasible
                    );
                }
            }
        };
        let assert_selected = |relations: &RelationState, expected: [u64; 3]| {
            assert_eq!(
                relations.check(
                    &policy,
                    native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
                ),
                CheckResult::MayBeFeasible
            );
            for (value, number) in values.iter().zip(expected) {
                assert_eq!(
                    relations.local_value(
                        value.expression().unwrap(),
                        &policy,
                        native::product_with(native::spec(8, 4, 256).with_relations(policy))
                            .borrowed()
                    ),
                    EvmValueQuery::<NumericValue>::Exact(U256::from(number))
                );
                assert_eq!(
                    relations.can_equal(
                        value.expression().unwrap(),
                        U256::from(number),
                        &policy,
                        native::product_with(native::spec(8, 4, 256).with_relations(policy))
                            .borrowed()
                    ),
                    CheckResult::MayBeFeasible
                );
            }
        };
        assert_unselected(&RelationState::default());
        let mut unknown = RelationState::default();
        unknown
            .observe(
                persistent.expression().unwrap(),
                persistent.numeric(),
                &policy,
            )
            .unwrap();
        assert_unselected(&unknown);

        let unrelated_owner = candidate.read(address(2), &v(7), domain.borrowed());
        assert_ne!(unrelated_owner.expression(), unrelated_source.expression());
        assert_ne!(persistent.expression(), independent_read.expression());
        for expression in [
            unrelated_owner.expression().unwrap(),
            unrelated_source.expression().unwrap(),
            independent_read.expression().unwrap(),
            &unrelated_selector,
        ] {
            for number in [0, 42] {
                let mut relations = RelationState::default();
                relations
                    .observe(expression, v(number).numeric(), &policy)
                    .unwrap();
                assert_unselected(&relations);
            }
        }

        for expected @ [slot, expected_transient, expected_balance] in [[0, 0, 0], [42, 19, 10]] {
            let mut relations = RelationState::default();
            relations
                .observe(persistent.expression().unwrap(), v(slot).numeric(), &policy)
                .unwrap();
            // Independent literal arms let the observed read determine the
            // shared selector, including the other two planes.
            assert_selected(&relations, expected);
            for (value, wrong) in [
                (&transient, 19 - expected_transient),
                (&balance, 10 - expected_balance),
            ] {
                assert_eq!(
                    relations.can_equal(
                        value.expression().unwrap(),
                        U256::from(wrong),
                        &policy,
                        native::product_with(native::spec(8, 4, 256).with_relations(policy))
                            .borrowed()
                    ),
                    CheckResult::ProvenImpossible
                );
            }
            let mut contradictory = relations.clone();
            contradictory
                .observe(
                    persistent.expression().unwrap(),
                    v(42 - slot).numeric(),
                    &policy,
                )
                .unwrap();
            assert_eq!(
                contradictory.check(
                    &policy,
                    native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
                ),
                CheckResult::ProvenImpossible
            );
            assert_eq!(
                contradictory.local_value(
                    transient.expression().unwrap(),
                    &policy,
                    native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
                ),
                EvmValueQuery::<NumericValue>::ProvenImpossible
            );
            let mut explicit = RelationState::default();
            assert_eq!(
                explicit.assume(
                    condition,
                    then_balance == U256::from(expected_balance),
                    &policy,
                    native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
                ),
                CheckResult::MayBeFeasible
            );
            assert_selected(&explicit, expected);
            assert_eq!(
                relations.assume(
                    condition,
                    then_balance != U256::from(expected_balance),
                    &policy,
                    native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
                ),
                CheckResult::ProvenImpossible
            );
        }
        for slot in [0, 42] {
            for transient in [0, 19] {
                for balance in [0, 10] {
                    let triple = [slot, transient, balance];
                    let mut relations = RelationState::default();
                    for (value, number) in values.iter().zip(triple) {
                        relations
                            .observe(value.expression().unwrap(), v(number).numeric(), &policy)
                            .unwrap();
                    }
                    assert_eq!(
                        relations.check(
                            &policy,
                            native::product_with(native::spec(8, 4, 256).with_relations(policy))
                                .borrowed()
                        ),
                        if matches!(triple, [0, 0, 0] | [42, 19, 10]) {
                            CheckResult::MayBeFeasible
                        } else {
                            CheckResult::ProvenImpossible
                        },
                        "whole-store alternative {triple:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn unknown_slot_write_weakly_updates_all_aliases_and_defaults() {
    let domain = native::product();
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "fixture");
    world
        .insert(address(1), EvmAccount::<NumericValue>::empty())
        .unwrap();
    let mut store = EvmStore::<NumericValue>::new(&world, native::product().borrowed());
    store.write(address(1), &v(7), &v(42), domain.borrowed());
    store.write(
        address(1),
        &EvmValue::<NumericValue>::top(),
        &v(9),
        domain.borrowed(),
    );
    assert_state_value_eq(
        &store.read(address(1), &v(7), domain.borrowed()),
        &domain.join(&v(42), &v(9)),
    );
    assert_state_value_eq(
        &store.read(address(1), &v(99), domain.borrowed()),
        &domain.join(&v(0), &v(9)),
    );
    let finite_alias = domain.join(&v(7), &v(8));
    store.write(address(1), &finite_alias, &v(1), domain.borrowed());
    assert!(
        store
            .read(address(1), &v(7), domain.borrowed())
            .contains(U256::from(42))
    );
    assert!(
        store
            .read(address(1), &v(8), domain.borrowed())
            .contains(U256::ZERO)
    );
    assert!(
        store
            .read(address(1), &v(8), domain.borrowed())
            .contains(U256::from(1))
    );
}

#[test]
fn snapshot_restores_storage_transient_state_and_balance() {
    let domain = native::product();
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "fixture");
    world
        .insert(address(1), EvmAccount::<NumericValue>::empty())
        .unwrap();
    let mut store = EvmStore::<NumericValue>::new(&world, native::product().borrowed());
    let original = store.clone();
    let snapshot = store.snapshot();
    store.write(address(1), &v(0), &v(42), domain.borrowed());
    store.write_transient(address(1), &v(0), &v(7), domain.borrowed());
    store.write_balance(address(1), v(99));
    store.restore(snapshot);
    assert_eq!(store, original);
}

#[test]
fn store_json_serializes_account_slot_observations() {
    let domain = native::product();
    let world = EvmWorld::<NumericValue>::new(Fork::Osaka, "fixture");
    let mut store = EvmStore::<NumericValue>::new(&world, native::product().borrowed());
    store.write(address(1), &v(7), &v(42), domain.borrowed());
    store.write_transient(address(2), &v(8), &v(19), domain.borrowed());
    let json = serde_json::to_value(store).unwrap();
    assert_eq!(json["persistent"]["slots"].as_array().unwrap().len(), 1);
    assert_eq!(json["transient"]["slots"].as_array().unwrap().len(), 1);
}

#[test]
fn words_are_big_endian_and_calldata_is_zero_padded() {
    let domain = native::product();
    let bytes = EvmByteArray::<NumericValue>::exact(&[0x12, 0x34]);
    let loaded = bytes.read_word(&v(0), domain.borrowed());
    assert_numeric_eq(
        &loaded,
        &EvmValue::<NumericValue>::constant(U256::from(0x1234) << 240),
    );
    assert!(
        loaded
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Arithmetic)
    );
    assert_numeric_eq(&bytes.read_word(&v(2), domain.borrowed()), &v(0));
    assert_numeric_eq(
        &bytes.read_word(
            &EvmValue::<NumericValue>::constant(U256::MAX),
            domain.borrowed(),
        ),
        &v(0),
    );
    assert_numeric_eq(
        &bytes.read_word(
            &EvmValue::<NumericValue>::constant(U256::from(usize::MAX)),
            domain.borrowed(),
        ),
        &v(0),
    );
    assert_eq!(
        bytes
            .slice(
                &EvmValue::<NumericValue>::constant(U256::from(usize::MAX)),
                &v(2),
                domain.borrowed()
            )
            .unwrap()
            .exact_bytes(native::product().borrowed()),
        Some(vec![0, 0])
    );
    assert_eq!(
        bytes
            .slice(&v(1), &v(3), domain.borrowed())
            .unwrap()
            .exact_bytes(native::product().borrowed()),
        Some(vec![0x34, 0, 0])
    );
    assert_state_value_eq(
        &EvmByteArray::<NumericValue>::unknown().read_word(&v(0), domain.borrowed()),
        &EvmValue::<NumericValue>::top(),
    );
}

#[test]
fn fresh_memory_zeroes_word_writes_and_mstore8_truncates() {
    let domain = native::product();
    let mut memory = EvmByteArray::<NumericValue>::memory();
    assert_numeric_eq(&memory.read_word(&v(100), domain.borrowed()), &v(0));
    memory
        .write_word(&v(1), &v(0x1234), domain.borrowed())
        .unwrap();
    assert_eq!(memory.len(), &v(64));
    assert_numeric_eq(&memory.read_word(&v(1), domain.borrowed()), &v(0x1234));
    memory
        .write_byte(&v(0), &v(0x12ff), domain.borrowed())
        .unwrap();
    assert_numeric_eq(&memory.byte_at(0, domain.borrowed()), &v(255));
    assert!(
        memory
            .byte_at(0, domain.borrowed())
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Arithmetic)
    );
}

#[test]
fn byte_join_includes_zero_from_missing_keys() {
    let domain = native::product();
    let zero = EvmByteArray::<NumericValue>::exact(&[0]);
    let nonzero = EvmByteArray::<NumericValue>::exact(&[42]);
    let joined = zero.join(&nonzero, domain.borrowed());
    assert_state_value_eq(
        &joined.byte_at(0, domain.borrowed()),
        &domain.join(&v(0), &v(42)),
    );
    assert_eq!(joined.len(), &v(1));
    let short = EvmByteArray::<NumericValue>::empty();
    let variable_length = short.join(&nonzero, domain.borrowed());
    assert!(
        variable_length
            .byte_at(0, domain.borrowed())
            .contains(U256::ZERO)
    );
    assert!(
        variable_length
            .byte_at(0, domain.borrowed())
            .contains(U256::from(42))
    );
}

#[test]
fn finite_write_aliases_preserve_untouched_possibilities() {
    let domain = native::product();
    let mut memory = EvmByteArray::<NumericValue>::memory();
    let offset = domain.join(&v(0), &v(1));
    memory
        .write_byte(&offset, &v(42), domain.borrowed())
        .unwrap();
    assert_numeric_eq(
        &memory.byte_at(0, domain.borrowed()),
        &domain.join(&v(0), &v(42)),
    );
    assert_numeric_eq(
        &memory.byte_at(1, domain.borrowed()),
        &domain.join(&v(0), &v(42)),
    );
    assert_numeric_eq(&memory.byte_at(2, domain.borrowed()), &v(0));
}

#[test]
fn copy_return_data_preserves_uncopied_suffix_and_expands_output_range() {
    let domain = native::product();
    let mut memory = EvmByteArray::<NumericValue>::memory();
    memory
        .copy_from(
            &v(0),
            &EvmByteArray::<NumericValue>::exact(&[1, 2, 3, 4]),
            &v(0),
            &v(4),
            domain.borrowed(),
        )
        .unwrap();
    memory
        .copy_return_data(
            &v(1),
            &EvmByteArray::<NumericValue>::exact(&[9]),
            &v(3),
            domain.borrowed(),
        )
        .unwrap();
    assert_eq!(
        memory
            .slice(&v(0), &v(4), domain.borrowed())
            .unwrap()
            .exact_bytes(native::product().borrowed()),
        Some(vec![1, 9, 3, 4])
    );
    memory
        .copy_return_data(
            &v(32),
            &EvmByteArray::<NumericValue>::empty(),
            &v(1),
            domain.borrowed(),
        )
        .unwrap();
    assert_eq!(memory.len(), &v(64));
}

#[test]
fn overlapping_copy_uses_source_snapshot() {
    let domain = native::product();
    let mut memory = EvmByteArray::<NumericValue>::memory();
    memory
        .copy_from(
            &v(0),
            &EvmByteArray::<NumericValue>::exact(&[1, 2, 3, 4]),
            &v(0),
            &v(4),
            domain.borrowed(),
        )
        .unwrap();
    let original = memory.clone();
    memory
        .copy_from(&v(1), &original, &v(0), &v(3), domain.borrowed())
        .unwrap();
    assert_eq!(
        memory
            .slice(&v(0), &v(4), domain.borrowed())
            .unwrap()
            .exact_bytes(native::product().borrowed()),
        Some(vec![1, 1, 2, 3])
    );
}

#[test]
fn logical_ranges_ignore_materialization_caps_and_keep_invalid_updates_atomic() {
    let domain = native::product();
    let mut memory = EvmByteArray::<NumericValue>::memory();
    memory.write_word(&v(1), &v(42), domain.borrowed()).unwrap();
    assert_eq!(memory.len().singleton(), Some(U256::from(64)));
    assert_eq!(
        memory.read_word(&v(1), domain.borrowed()).singleton(),
        Some(U256::from(42))
    );
    let unbound = memory
        .slice(&v(0), &EvmValue::<NumericValue>::top(), domain.borrowed())
        .unwrap();
    assert!(unbound.len().numeric().is_top());
    assert!(unbound.len().expression().is_some());
    let maximal = memory
        .slice(
            &v(0),
            &EvmValue::<NumericValue>::constant(U256::MAX),
            domain.borrowed(),
        )
        .unwrap();
    assert_eq!(maximal.len().singleton(), Some(U256::MAX));
    assert!(
        maximal
            .exact_bytes_bounded(32, native::product().borrowed())
            .is_none()
    );
    let original = memory.clone();
    assert!(matches!(
        memory.write_word(
            &EvmValue::<NumericValue>::constant(U256::MAX),
            &v(42),
            domain.borrowed()
        ),
        Err(RangeError::OffsetTooLarge(_))
    ));
    assert_eq!(memory, original);
    memory
        .expand(
            &EvmValue::<NumericValue>::constant(U256::MAX),
            &v(0),
            domain.borrowed(),
        )
        .unwrap();
    assert_eq!(memory, original);
    assert_eq!(
        memory
            .slice(
                &EvmValue::<NumericValue>::constant(U256::MAX),
                &v(0),
                domain.borrowed()
            )
            .unwrap(),
        EvmByteArray::<NumericValue>::empty()
    );
}

#[test]
fn unknown_memory_offset_is_conservative_without_allocating_a_range() {
    let domain = native::product();
    let mut memory = EvmByteArray::<NumericValue>::memory();
    let offset = EvmValue::<NumericValue>::top().with_expression(ExprId::fresh().unwrap());
    memory
        .write_byte(&offset, &v(42), domain.borrowed())
        .unwrap();
    assert_state_value_eq(memory.len(), &EvmValue::<NumericValue>::top());
    let word = memory.read_word(&v(0), domain.borrowed());
    assert!(word.expression().is_some());
    assert!(!word.symbolic_limit_reached());
    assert!(word.contains(U256::ZERO));
    for shift in 0..32 {
        assert!(word.contains(U256::from(42) << (shift * 8)));
    }
    assert!(!word.contains(U256::ONE));
    assert_eq!(word.known_bits().zero(), !U256::from_be_bytes([42; 32]),);
    let policy = local_policy();
    for (at, expected) in [
        (0, U256::from(42) << 248),
        (31, U256::from(42)),
        (32, U256::ZERO),
    ] {
        let mut relations = RelationState::default();
        relations
            .observe(offset.expression().unwrap(), v(at).numeric(), &policy)
            .unwrap();
        assert_eq!(
            relations.local_value(
                word.expression().unwrap(),
                &policy,
                native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
            ),
            EvmValueQuery::<NumericValue>::Exact(expected),
        );
        assert_eq!(
            relations.can_equal(
                word.expression().unwrap(),
                U256::from(0x2a2a),
                &policy,
                native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
            ),
            CheckResult::ProvenImpossible,
            "a fixed write offset still excludes two nonzero bytes"
        );
    }
    // Independent per-byte joins lose the fact that only one offset was
    // written. MayBeFeasible is this precision gap, not a satisfying witness.
    let EvmValueQuery::<NumericValue>::Bounds(bounds) = RelationState::default().local_value(
        word.expression().unwrap(),
        &policy,
        native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed(),
    ) else {
        panic!("expected the documented unknown-offset correlation precision gap");
    };
    assert!(bounds.contains(U256::from(0x2a2a)));
    assert_eq!(
        RelationState::default().can_equal(
            word.expression().unwrap(),
            U256::from(0x2a2a),
            &policy,
            native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
        ),
        CheckResult::MayBeFeasible,
    );
    assert!(memory.stored_bytes().next().is_none());
    assert!(memory.work_size() < 1000);
}

fn log_key() -> LogKey {
    LogKey {
        address: address(1),
        code_address: address(2),
        code_hash: alloy_primitives::B256::ZERO,
        source_kind: evm_abstract::world::LogSourceKind::Runtime,
        pc: 17,
    }
}

#[test]
fn event_summary_joins_site_visits_and_retains_possibility_on_missing_path() {
    let domain = native::product();
    let world = EvmWorld::<NumericValue>::new(Fork::Osaka, "fixture");
    let original = EvmStore::<NumericValue>::new(&world, native::product().borrowed());
    let mut emitted = original.clone();
    emitted
        .emit_log(
            log_key(),
            EvmLog {
                topics: vec![v(7)],
                data: EvmByteArray::<NumericValue>::exact(&[1]),
            },
            domain.borrowed(),
        )
        .unwrap();
    emitted
        .emit_log(
            log_key(),
            EvmLog {
                topics: vec![v(9)],
                data: EvmByteArray::<NumericValue>::exact(&[3]),
            },
            domain.borrowed(),
        )
        .unwrap();
    let joined = original.join(&emitted, domain.borrowed());
    let event = &joined.possible_logs()[&log_key()];
    assert_eq!(event.topics, vec![domain.join(&v(7), &v(9))]);
    assert_state_value_eq(
        &event.data.byte_at(0, domain.borrowed()),
        &domain.join(&v(1), &v(3)),
    );
    assert_eq!(joined, emitted.join(&original, domain.borrowed()));
    assert_eq!(joined, joined.join(&joined, domain.borrowed()));
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
    let domain = native::product();
    let world = EvmWorld::<NumericValue>::new(Fork::Osaka, "fixture");
    let mut store = EvmStore::<NumericValue>::new(&world, native::product().borrowed());
    let snapshot = store.snapshot();
    store
        .emit_log(
            log_key(),
            EvmLog {
                topics: vec![],
                data: EvmByteArray::<NumericValue>::empty(),
            },
            domain.borrowed(),
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
    let domain = native::product();
    let world = EvmWorld::<NumericValue>::new(Fork::Osaka, "fixture");
    let mut store = EvmStore::<NumericValue>::new(&world, native::product().borrowed());
    store
        .emit_log(
            log_key(),
            EvmLog {
                topics: vec![v(7)],
                data: EvmByteArray::<NumericValue>::empty(),
            },
            domain.borrowed(),
        )
        .unwrap();
    let original = store.clone();
    assert!(matches!(
        store.emit_log(
            log_key(),
            EvmLog {
                topics: vec![],
                data: EvmByteArray::<NumericValue>::empty()
            },
            domain.borrowed()
        ),
        Err(LogError::InconsistentTopics { .. })
    ));
    assert!(matches!(
        store.emit_log(
            log_key(),
            EvmLog {
                topics: vec![v(0); 5],
                data: EvmByteArray::<NumericValue>::empty()
            },
            domain.borrowed()
        ),
        Err(LogError::TooManyTopics(5))
    ));
    assert_eq!(store, original);
}
