use super::{address, budget, catalogue, domain, prepare, store, word};
use crate::{
    Fork,
    world::process::{
        CapturedProcessCode, OwnerCatalogue, OwnerCatalogueLimits, ProcessOwner, ProcessOwnerError,
    },
};
use alloy_primitives::B256;
use std::collections::BTreeSet;

#[test]
fn unknown_targets_cover_every_named_alias_and_every_outside_owner() {
    let catalogue = catalogue();
    assert_eq!(catalogue.codes().len(), 1);
    assert_eq!(catalogue.classify(address(0xaaa)), ProcessOwner::Residual);
    assert_eq!(catalogue.classify(address(0xbbb)), ProcessOwner::Residual);
    assert_eq!(
        catalogue.classify(address(0x100)),
        ProcessOwner::Named(address(0x100))
    );
    let targets: BTreeSet<_> = catalogue.unknown_targets().collect();
    assert_eq!(
        targets,
        BTreeSet::from([
            ProcessOwner::Named(address(0x100)),
            ProcessOwner::Named(address(0x200)),
            ProcessOwner::Residual,
        ])
    );
    for actual in [0x100, 0x200, 0xaaa, 0xbbb, 0xccc] {
        assert!(targets.contains(&catalogue.classify(address(actual))));
    }
}

#[test]
fn epochs_bound_names_code_versions_and_forks_and_refuse_truncation() {
    let limits = OwnerCatalogueLimits { names: 3, codes: 2 };
    let old = catalogue();
    let code = *old.codes().first().unwrap();
    old.admit_code(code, &mut budget()).unwrap();
    assert!(matches!(
        old.admit_code(
            CapturedProcessCode {
                hash: B256::ZERO,
                ..code
            },
            &mut budget()
        ),
        Err(ProcessOwnerError::UncapturedCode(_))
    ));
    let reordered = OwnerCatalogue::freeze(
        &[address(0x200), address(0x100)],
        &[code],
        limits,
        &mut budget(),
    )
    .unwrap();
    assert_eq!(old.epoch(), reordered.epoch());
    for (names, codes) in [
        (
            vec![address(0x100), address(0x200), address(0xaaa)],
            vec![code],
        ),
        (
            vec![address(0x100), address(0x200)],
            vec![CapturedProcessCode {
                hash: B256::ZERO,
                ..code
            }],
        ),
        (
            vec![address(0x100), address(0x200)],
            vec![CapturedProcessCode {
                fork: Fork::Cancun,
                ..code
            }],
        ),
    ] {
        let expanded = OwnerCatalogue::freeze(&names, &codes, limits, &mut budget()).unwrap();
        assert_ne!(old.epoch(), expanded.epoch());
    }
    assert_eq!(old.classify(address(0xaaa)), ProcessOwner::Residual);
    assert!(matches!(
        OwnerCatalogue::freeze(
            &[address(0x100), address(0x200)],
            &[code],
            OwnerCatalogueLimits { names: 1, codes: 1 },
            &mut budget(),
        ),
        Err(ProcessOwnerError::Capacity)
    ));
    assert!(matches!(
        OwnerCatalogue::freeze(&[address(0x100)], &[code], limits, &mut budget(),),
        Err(ProcessOwnerError::UnnamedCode(_))
    ));
    assert!(matches!(
        old.key(ProcessOwner::Named(address(0xaaa))),
        Err(ProcessOwnerError::UnnamedOwner(_))
    ));
}

#[test]
fn residual_preparation_has_one_shape_and_named_owners_remain_separate() {
    let domain = domain();
    let catalogue = catalogue();
    let first = store(&domain);
    let mut second = first.clone();
    second.write(address(0xaaa), &word(3), &word(7), domain.borrowed());
    second.write(address(0xccc), &word(7), &word(91), domain.borrowed());
    second.write_transient(address(0xbbb), &word(7), &word(17), domain.borrowed());
    second.write_balance(address(0xddd), word(77));
    let a = prepare(
        &first,
        &catalogue,
        catalogue.classify(address(0xaaa)),
        &domain,
    );
    let b = prepare(
        &second,
        &catalogue,
        catalogue.classify(address(0xbbb)),
        &domain,
    );
    assert_eq!(a.key(), b.key());
    assert!(a.equivalent(&b, domain.borrowed(), &mut budget()).unwrap());
    let joined = a.join(&b, domain.borrowed(), &mut budget()).unwrap();
    assert!(
        a.equivalent(&joined, domain.borrowed(), &mut budget())
            .unwrap()
    );
    let named_a = prepare(
        &first,
        &catalogue,
        ProcessOwner::Named(address(0x100)),
        &domain,
    );
    let named_b = prepare(
        &first,
        &catalogue,
        ProcessOwner::Named(address(0x200)),
        &domain,
    );
    assert!(matches!(
        named_a.join(&named_b, domain.borrowed(), &mut budget()),
        Err(ProcessOwnerError::Shape)
    ));
    assert_eq!(
        catalogue.unknown_targets().count(),
        3,
        "Store growth must not grow the catalogue"
    );
}

#[test]
fn prepared_join_is_idempotent_commutative_associative_and_epoch_checked() {
    let domain = domain();
    let catalogue = catalogue();
    let initial = store(&domain);
    let mut inputs = Vec::new();
    for value in [3, 7, 19] {
        let mut input = initial.clone();
        input.write(address(0x100), &word(0), &word(value), domain.borrowed());
        inputs.push(prepare(&input, &catalogue, ProcessOwner::Residual, &domain));
    }
    let [a, b, c] = inputs.as_slice() else {
        unreachable!()
    };
    let join = |left: &crate::world::process::PreparedProcessStore<_>,
                right: &crate::world::process::PreparedProcessStore<_>| {
        left.join(right, domain.borrowed(), &mut budget()).unwrap()
    };
    let equal = |left: &crate::world::process::PreparedProcessStore<_>,
                 right: &crate::world::process::PreparedProcessStore<_>| {
        left.equivalent(right, domain.borrowed(), &mut budget())
            .unwrap()
    };
    assert!(equal(a, &join(a, a)));
    assert!(equal(&join(a, b), &join(b, a)));
    assert!(equal(&join(&join(a, b), c), &join(a, &join(b, c))));
    let next = OwnerCatalogue::freeze(
        &[address(0x100), address(0x200), address(0xaaa)],
        &catalogue.codes().iter().copied().collect::<Vec<_>>(),
        OwnerCatalogueLimits { names: 3, codes: 1 },
        &mut budget(),
    )
    .unwrap();
    let new = prepare(&initial, &next, ProcessOwner::Residual, &domain);
    assert!(matches!(
        a.join(&new, domain.borrowed(), &mut budget()),
        Err(ProcessOwnerError::Shape)
    ));
}
