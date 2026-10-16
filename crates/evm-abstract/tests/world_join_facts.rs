//! Public API regressions for path-local operand guarantees at world joins.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{ExecutionConfig, OutcomeKind, Status, analyze_world},
    domain::{
        AbstractValue, Domain,
        symbolic::{ExprId, ExprLimits},
    },
    world::{Account, ByteArray, Entry, EvmEnvironment, Store, World},
};
use std::convert::Infallible;

fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn both_values(value: &AbstractValue) {
    assert!(value.contains(U256::ZERO), "lost zero: {value:?}");
    assert!(value.contains(U256::ONE), "lost one: {value:?}");
    assert!(value.may_be_zero());
    assert!(value.may_be_nonzero());
}

fn prove(condition: &ExprId, guard: &ExprId, truth: bool) -> Result<Option<bool>, Infallible> {
    Ok((condition == guard).then_some(truth))
}

#[test]
fn memory_join_does_not_promote_one_sided_guard_proofs() {
    let domain = Domain::default();
    let limits = ExprLimits::default();
    for byte_write in [false, true] {
        for truth in [false, true] {
            for different_roots in [false, true] {
                let mut yes = ByteArray::memory();
                let mut no = ByteArray::memory();
                if byte_write {
                    yes.write_byte(&word(31), &word(1), 128, domain).unwrap();
                    no.write_byte(&word(31), &word(0), 128, domain).unwrap();
                } else {
                    yes.write_word(&word(0), &word(1), 128, domain).unwrap();
                    no.write_word(&word(0), &word(0), 128, domain).unwrap();
                }
                let guard = ExprId::fresh().unwrap();
                let right = yes
                    .join_guarded(&no, domain, guard.clone(), limits)
                    .unwrap();
                let mut left = right.clone();
                assert!(
                    left.refine_guards(domain, limits, &mut |c| prove(c, &guard, truth))
                        .unwrap()
                );
                assert_eq!(left.state_root(), right.state_root());
                assert_eq!(
                    left.read_word_checked(&word(0), domain)
                        .unwrap()
                        .singleton(),
                    Some(U256::from(truth as u8))
                );
                if different_roots {
                    left.write_word(&word(64), &word(9), 128, domain).unwrap();
                }
                for (a, b) in [(&left, &right), (&right, &left)] {
                    let joined = a
                        .join_guarded(b, domain, ExprId::fresh().unwrap(), limits)
                        .unwrap();
                    assert!(!joined.symbolic_facts().any(|(e, _)| e == &guard));
                    both_values(&joined.read_word_checked(&word(0), domain).unwrap());
                    both_values(&joined.read_byte_checked(&word(31), domain).unwrap());
                    for (_, value) in joined.stored_bytes().filter(|(i, _)| *i == 31) {
                        both_values(value);
                    }
                    let mut zero = joined.clone();
                    assert!(
                        zero.refine_guards(domain, limits, &mut |c| prove(c, &guard, false))
                            .unwrap()
                    );
                    assert_eq!(
                        zero.read_word_checked(&word(0), domain)
                            .unwrap()
                            .singleton(),
                        Some(U256::ZERO)
                    );
                }
                // Both paths have the same external proof; distinct roots keep
                // this control from merely taking the identical-wrapper shortcut.
                let mut also_proved = left.clone();
                also_proved
                    .write_word(&word(96), &word(8), 128, domain)
                    .unwrap();
                let both = left
                    .join_guarded(&also_proved, domain, ExprId::fresh().unwrap(), limits)
                    .unwrap();
                assert!(both.symbolic_facts().any(|(e, _)| e == &guard));
                assert_eq!(
                    both.read_word_checked(&word(0), domain)
                        .unwrap()
                        .singleton(),
                    Some(U256::from(truth as u8))
                );
            }
        }
    }
}

fn read(store: &Store, owner: Address, transient: bool) -> AbstractValue {
    if transient {
        store
            .read_transient_checked(owner, &word(0), Domain::default())
            .unwrap()
    } else {
        store
            .read_checked(owner, &word(0), Domain::default())
            .unwrap()
    }
}

fn write(store: &mut Store, owner: Address, key: u64, value: u64, transient: bool) {
    if transient {
        store
            .write_transient_checked(owner, &word(key), &word(value), Domain::default())
            .unwrap();
    } else {
        store
            .write_checked(owner, &word(key), &word(value), Domain::default())
            .unwrap();
    }
}

#[test]
fn store_join_does_not_promote_one_sided_proofs_in_either_plane() {
    let domain = Domain::default();
    let limits = ExprLimits::default();
    let owner = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "issue134:join-facts");
    world.insert(owner, Account::empty()).unwrap();
    for transient in [false, true] {
        for truth in [false, true] {
            for different_roots in [false, true] {
                let mut yes = Store::new_checked(&world, domain).unwrap();
                let mut no = yes.clone();
                write(&mut yes, owner, 0, 1, transient);
                write(&mut no, owner, 0, 0, transient);
                let guard = ExprId::fresh().unwrap();
                let right = yes
                    .join_guarded(&no, domain, guard.clone(), limits)
                    .unwrap();
                let mut left = right.clone();
                assert!(
                    left.refine_guards(domain, limits, &mut |c| prove(c, &guard, truth))
                        .unwrap()
                );
                assert_eq!(
                    left.slot_root(owner, transient).unwrap(),
                    right.slot_root(owner, transient).unwrap()
                );
                assert_eq!(
                    read(&left, owner, transient).singleton(),
                    Some(U256::from(truth as u8))
                );
                if different_roots {
                    write(&mut left, owner, 1, 9, transient);
                }
                for (a, b) in [(&left, &right), (&right, &left)] {
                    let joined = a
                        .join_guarded(b, domain, ExprId::fresh().unwrap(), limits)
                        .unwrap();
                    assert!(!joined.symbolic_facts(transient).any(|(e, _)| e == &guard));
                    both_values(&read(&joined, owner, transient));
                    let cache = if transient {
                        joined.transient_slots()
                    } else {
                        joined.slots()
                    };
                    both_values(cache.get(&(owner, U256::ZERO)).unwrap());
                    let mut zero = joined.clone();
                    assert!(
                        zero.refine_guards(domain, limits, &mut |c| prove(c, &guard, false))
                            .unwrap()
                    );
                    assert_eq!(read(&zero, owner, transient).singleton(), Some(U256::ZERO));
                }
                let mut also_proved = left.clone();
                write(&mut also_proved, owner, 2, 8, transient);
                let both = left
                    .join_guarded(&also_proved, domain, ExprId::fresh().unwrap(), limits)
                    .unwrap();
                assert!(both.symbolic_facts(transient).any(|(e, _)| e == &guard));
                assert_eq!(
                    read(&both, owner, transient).singleton(),
                    Some(U256::from(truth as u8))
                );
            }
        }
    }
}

#[test]
fn joined_memory_retains_the_subsequent_evm_zero_branch() {
    let domain = Domain::default();
    let limits = ExprLimits::default();
    let guard = ExprId::fresh().unwrap();
    let mut yes = ByteArray::memory();
    let mut no = ByteArray::memory();
    yes.write_word(&word(0), &word(1), 64, domain).unwrap();
    no.write_word(&word(0), &word(0), 64, domain).unwrap();
    let right = yes
        .join_guarded(&no, domain, guard.clone(), limits)
        .unwrap();
    let mut left = right.clone();
    assert!(
        left.refine_guards(domain, limits, &mut |c| prove(c, &guard, true))
            .unwrap()
    );
    let joined = left
        .join_guarded(&right, domain, ExprId::fresh().unwrap(), limits)
        .unwrap();
    let calldata = joined.slice(&word(0), &word(32), 64, domain).unwrap();
    let owner = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "issue134:subsequent-zero-branch");
    // CALLDATALOAD(0); ISZERO; JUMPI zero. Each branch returns its marker.
    world
        .insert(
            owner,
            Account::from_hex("5f3515600e5760015f5260205ff35b5f5f5260205ff3", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = Entry {
        address: owner,
        environment: EvmEnvironment {
            to: owner.into(),
            value: word(0),
            calldata,
            ..EvmEnvironment::default()
        },
    };
    let analysis = analyze_world(world, entry, ExecutionConfig::default()).unwrap();
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    for marker in [U256::ZERO, U256::ONE] {
        assert!(
            analysis.outcomes().iter().any(|outcome| {
                outcome.kind == OutcomeKind::Return
                    && outcome
                        .data
                        .read_word_checked(&word(0), domain)
                        .unwrap()
                        .contains(marker)
            }),
            "missing branch returning {marker}"
        );
    }
}

#[test]
fn joined_numeric_caches_use_each_inputs_own_default() {
    let domain = Domain::default();
    let limits = ExprLimits::default();
    let mut exact = ByteArray::memory();
    exact.write_byte(&word(0), &word(1), 64, domain).unwrap();
    let unknown = ByteArray::unknown();
    for (left, right) in [(&exact, &unknown), (&unknown, &exact)] {
        let joined = left
            .join_guarded(right, domain, ExprId::fresh().unwrap(), limits)
            .unwrap();
        let byte = joined.read_byte_checked(&word(0), domain).unwrap();
        for value in [0, 1, 2, 255] {
            assert!(
                byte.contains(U256::from(value)),
                "lost input default: {byte:?}"
            );
        }
        let cached = joined
            .stored_bytes()
            .find(|(index, _)| *index == 0)
            .unwrap()
            .1;
        assert!(cached.contains(U256::from(2)));
    }

    let owner = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "issue134:join-defaults");
    world.insert(owner, Account::empty()).unwrap();
    for global in [false, true] {
        let mut exact = Store::new_checked(&world, domain).unwrap();
        write(&mut exact, owner, 0, 1, false);
        write(&mut exact, owner, 0, 1, true);
        let mut unknown = Store::new_checked(&world, domain).unwrap();
        if global {
            unknown.havoc_all();
        } else {
            unknown.havoc_account(owner);
        }
        for (left, right) in [(&exact, &unknown), (&unknown, &exact)] {
            let joined = left
                .join_guarded(right, domain, ExprId::fresh().unwrap(), limits)
                .unwrap();
            for transient in [false, true] {
                let value = read(&joined, owner, transient);
                let cache = if transient {
                    joined.transient_slots()
                } else {
                    joined.slots()
                };
                let cached = cache.get(&(owner, U256::ZERO)).unwrap();
                for candidate in [U256::ZERO, U256::ONE, U256::MAX] {
                    assert!(value.contains(candidate));
                    assert!(cached.contains(candidate));
                }
                let unlisted = if transient {
                    joined
                        .read_transient_checked(owner, &word(7), domain)
                        .unwrap()
                } else {
                    joined.read_checked(owner, &word(7), domain).unwrap()
                };
                assert!(unlisted.contains(U256::MAX));
            }
        }
    }
}
