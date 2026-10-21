use super::*;
use crate::{
    Fork,
    domain::provenance::Origin,
    world::{AbstractLog, Account, ByteArray, LogKey, LogSourceKind, World},
};
use alloy_primitives::B256;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

fn reference(store: &Store, owner: Address, domain: Domain, selector: ExprId) -> Store {
    let mut deleted = store.clone();
    deleted.delete_account(owner);
    store
        .join_guarded(
            &deleted,
            domain,
            selector,
            domain.spec().relations().expressions(),
        )
        .unwrap()
}

fn assert_owner(left: &Store, right: &Store, owner: Address, domain: Domain) {
    for (a, b) in [
        (&left.persistent, &right.persistent),
        (&left.transient, &right.transient),
    ] {
        assert_eq!(a.root_at(owner), b.root_at(owner));
        assert_eq!(a.default_at(owner), b.default_at(owner));
        assert_eq!(
            a.slots
                .range((owner, U256::ZERO)..=(owner, U256::MAX))
                .collect::<Vec<_>>(),
            b.slots
                .range((owner, U256::ZERO)..=(owner, U256::MAX))
                .collect::<Vec<_>>()
        );
        for key in [word(1), word(2), word(5), word(99)] {
            assert_eq!(
                a.read_checked_with(owner, &key, crate::domain::ValueAlgebra::new(&domain)),
                b.read_checked_with(owner, &key, crate::domain::ValueAlgebra::new(&domain))
            );
        }
    }
    assert_eq!(left.read_balance(owner), right.read_balance(owner));
    assert_eq!(left.nonce(owner), right.nonce(owner));
    assert_eq!(left.code(owner), right.code(owner));
    assert_eq!(left.existence(owner), right.existence(owner));
}

#[test]
fn same_selector_matches_full_join_with_aliases_history_and_unaffected_owners() {
    let domain = Domain::default();
    let limits = domain.spec().relations().expressions();
    let owner = address(0x101);
    let other = address(0x201);
    let unlisted = address(0x301);
    let mut world = World::new(Fork::Osaka, "owner deletion reference");
    let mut account = Account::unknown();
    account.balance = word(7);
    account.nonce = word(2);
    account.storage.insert(
        U256::from(5),
        AbstractValue::unsigned_range(U256::from(7), U256::from(8))
            .unwrap()
            .with_expression(ExprId::fresh().unwrap())
            .with_origin(Origin::Calldata),
    );
    world.insert(owner, account).unwrap();
    world.insert(other, Account::empty()).unwrap();
    let mut old = Store::new(&world);
    let historical = old
        .read_checked_with(owner, &word(5), crate::domain::ValueAlgebra::new(&domain))
        .unwrap();
    // This write retains an old read variable in the unchanged operand facts.
    old.write_checked(other, &word(5), &historical, domain)
        .unwrap();
    old.write_transient_checked(other, &word(5), &historical, domain)
        .unwrap();
    let saved = old.snapshot();
    let saved_state = saved.state().clone();
    let finite = domain
        .join(&word(1), &word(2))
        .with_expression(ExprId::fresh().unwrap());
    let open = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    for transient in [false, true] {
        for (key, value) in [
            (word(1), word(11)),
            (finite.clone(), word(13)),
            (open.clone(), word(17)),
        ] {
            if transient {
                old.write_transient_checked(owner, &key, &value, domain)
                    .unwrap();
            } else {
                old.write_checked(owner, &key, &value, domain).unwrap();
            }
        }
    }
    old.created.insert(owner, None);
    old.pending_destruction.insert(owner, None);
    old.may_touch_account(owner);
    let site = LogKey {
        address: owner,
        code_address: other,
        code_hash: B256::ZERO,
        source_kind: LogSourceKind::Runtime,
        pc: 7,
    };
    old.emit_log(
        site.clone(),
        AbstractLog {
            topics: vec![historical.clone()],
            data: ByteArray::exact(&[1, 2, 3]),
        },
        domain,
    )
    .unwrap();
    old.check_existing(limits).unwrap();
    let frozen = old.clone();
    let selector = ExprId::fresh().unwrap();
    let expected = reference(&old, owner, domain, selector.clone());
    let mut local = old.clone();
    local
        .join_deleted_guarded(
            owner,
            crate::domain::ValueAlgebra::new(&domain),
            selector.clone(),
        )
        .unwrap();
    local.check_existing(limits).unwrap();

    for address in [owner, other, unlisted] {
        assert_owner(&local, &expected, address, domain);
    }
    for (a, b, reference_plane) in [
        (&local.persistent, &old.persistent, &expected.persistent),
        (&local.transient, &old.transient, &expected.transient),
    ] {
        assert_eq!(a.base, b.base);
        assert_eq!(a.global_default, b.global_default);
        assert_eq!(a.operands, b.operands);
        assert_eq!(a.initial_independence, b.initial_independence);
        assert_eq!(a.root_at(other), b.root_at(other));
        assert_eq!(a.root_at(unlisted), b.root_at(unlisted));
        assert_ne!(a.root_at(owner), a.root_at(unlisted));
        assert!(a.operands.contains_key(historical.expression().unwrap()));
        for key in [&finite, &open] {
            assert_eq!(
                a.read_checked_with(owner, key, crate::domain::ValueAlgebra::new(&domain)),
                reference_plane.read_checked_with(
                    owner,
                    key,
                    crate::domain::ValueAlgebra::new(&domain)
                )
            );
        }
    }
    assert_eq!(local.balance_default, old.balance_default);
    assert_eq!(local.possible_logs, old.possible_logs);
    assert_eq!(
        local.possible_logs[&site].data.state_root(),
        old.possible_logs[&site].data.state_root()
    );
    assert_eq!(local.logs_unknown, old.logs_unknown);
    assert_eq!(local.created, old.created);
    assert_eq!(local.pending_destruction, old.pending_destruction);
    assert_eq!(local.touched, old.touched);
    assert_eq!(old, frozen);
    assert_eq!(saved.state(), &saved_state);
    assert_eq!(
        saved
            .state()
            .read_checked_with(owner, &word(5), crate::domain::ValueAlgebra::new(&domain))
            .unwrap(),
        historical
    );

    // The same branch choice must select both planes and both scalar values.
    for retain in [false, true] {
        let mut projected = local.clone();
        assert!(
            projected
                .refine_guards(domain, limits, &mut |condition| {
                    Ok::<_, ()>((condition == &selector).then_some(retain))
                })
                .unwrap()
        );
        for transient in [false, true] {
            let plane = if transient {
                &projected.transient
            } else {
                &projected.persistent
            };
            let before = if transient {
                &old.transient
            } else {
                &old.persistent
            };
            for key in [word(1), word(2), word(99), finite.clone(), open.clone()] {
                let actual = plane
                    .read_checked_with(owner, &key, crate::domain::ValueAlgebra::new(&domain))
                    .unwrap();
                if retain {
                    assert_eq!(
                        actual.numeric(),
                        before
                            .read_checked_with(
                                owner,
                                &key,
                                crate::domain::ValueAlgebra::new(&domain)
                            )
                            .unwrap()
                            .numeric()
                    );
                } else {
                    assert_eq!(actual.singleton(), Some(U256::ZERO));
                }
            }
        }
        assert_eq!(
            projected.read_balance(owner).singleton(),
            Some(U256::from(if retain { 7 } else { 0 }))
        );
        assert_eq!(
            projected.nonce(owner).singleton(),
            Some(U256::from(if retain { 2 } else { 0 }))
        );
        assert_eq!(
            projected
                .persistent
                .operands
                .get(historical.expression().unwrap()),
            old.persistent
                .operands
                .get(historical.expression().unwrap())
        );
    }
}

#[test]
fn implicit_owner_keeps_global_defaults_other_sources_and_prior_must_facts() {
    let domain = Domain::default();
    let mut old = Store::new(&World::new(Fork::Osaka, "implicit owner deletion"));
    // Make both planes unobserved, with explicit strong and finite weak effects.
    old.havoc_all();
    let owner = address(3);
    let other = address(0x201);
    let key = domain
        .join(&word(1), &word(2))
        .with_expression(ExprId::fresh().unwrap());
    old.write_checked(other, &word(1), &word(7), domain)
        .unwrap();
    old.write_transient_checked(other, &key, &word(9), domain)
        .unwrap();
    let selector = ExprId::fresh().unwrap();
    let expected = reference(&old, owner, domain, selector.clone());
    let mut local = old.clone();
    local
        .join_deleted_guarded(owner, crate::domain::ValueAlgebra::new(&domain), selector)
        .unwrap();
    for address in [owner, other, address(0x999)] {
        assert_owner(&local, &expected, address, domain);
    }
    assert_eq!(local.global_storage_default(), old.global_storage_default());
    assert_eq!(
        local.global_transient_default(),
        old.global_transient_default()
    );
    assert_eq!(local.balance_default(), old.balance_default());
    assert_eq!(
        local.persistent.initial_independence,
        old.persistent.initial_independence
    );
    assert_eq!(
        local.transient.initial_independence,
        old.transient.initial_independence
    );
    assert_eq!(local.persistent.roots.len(), old.persistent.roots.len() + 1);
    assert_eq!(local.transient.roots.len(), old.transient.roots.len() + 1);
}

#[test]
fn code_presence_and_lifecycle_flags_match_full_reference_before_cleanup() {
    let domain = Domain::default();
    let owner = address(0x101);
    for code in [
        Code::Empty,
        Code::Unknown,
        Account::from_hex("00", Fork::Osaka).unwrap().code,
    ] {
        for pending in [Some(true), None] {
            for touch in [Some(false), None, Some(true)] {
                let mut world = World::new(Fork::Osaka, "owner deletion flags");
                world
                    .insert(
                        owner,
                        Account {
                            code: code.clone(),
                            ..Account::empty()
                        },
                    )
                    .unwrap();
                let mut old = Store::new(&world);
                old.pending_destruction.insert(owner, pending);
                old.touched.insert(owner, touch);
                old.created.insert(owner, Some(true));
                let selector = ExprId::fresh().unwrap();
                let expected = reference(&old, owner, domain, selector.clone());
                let mut local = old.clone();
                local
                    .join_deleted_guarded(
                        owner,
                        crate::domain::ValueAlgebra::new(&domain),
                        selector,
                    )
                    .unwrap();
                assert_owner(&local, &expected, owner, domain);
                assert_eq!(local.pending_destruction, old.pending_destruction);
                assert_eq!(local.created, old.created);
                assert_eq!(local.touched, old.touched);
            }
        }
    }
}
