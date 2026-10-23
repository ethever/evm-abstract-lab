use crate::{A, address, fixture, insert, native, new_world, support::word};
use alloy_primitives::{B256, U256, keccak256};
use evm_abstract::{
    Fork,
    bytecode::Program,
    resource::WorkBudget,
    world::{
        Code, EvmStore,
        process::{
            CapturedProcessCode, OwnerCatalogue, OwnerCatalogueLimits, PreparedProcessStore,
            ProcessOwner, ProcessOwnerError,
        },
    },
};

#[test]
fn deployed_outside_runtime_is_covered_without_new_names_code_versions_or_keys() {
    let values = native::product();
    let mut world = new_world();
    insert(&mut world, A, &fixture(&[0x00]), &[(0, 7)]);
    let captured = CapturedProcessCode {
        source: address(A),
        hash: keccak256([0x00]),
        fork: world.fork(),
    };
    let catalogue = OwnerCatalogue::freeze(
        &[address(A)],
        &[captured],
        OwnerCatalogueLimits { names: 1, codes: 1 },
        &mut WorkBudget::new(2_000_000),
    )
    .unwrap();
    let mut store = EvmStore::new_checked(&world, values.borrowed()).unwrap();
    let initial = PreparedProcessStore::prepare(
        &store,
        &catalogue,
        ProcessOwner::Residual,
        values.borrowed(),
        &mut WorkBudget::new(20_000_000),
    )
    .unwrap();
    let key = initial.key();
    let stable = PreparedProcessStore::prepare(
        &store,
        &catalogue,
        ProcessOwner::Residual,
        values.borrowed(),
        &mut WorkBudget::new(20_000_000),
    )
    .unwrap();
    let mut joined = initial;
    // Model successful deployments through the public native Store boundary.
    // This tests source admission, not the allocation planner's state relation.
    for i in 0..32 {
        let outside = address(0x1000 + i);
        let program = Program::decode_with_fork(&[0x60, i as u8, 0x00], world.fork()).unwrap();
        store.begin_creation(outside);
        store.deploy_code(outside, program.clone());
        store.write(outside, &word(i), &word(i + 1), values.borrowed());
        assert!(matches!(store.code(outside), Some(Code::Runtime(_))));
        let prepared = PreparedProcessStore::prepare(
            &store,
            &catalogue,
            ProcessOwner::Residual,
            values.borrowed(),
            &mut WorkBudget::new(20_000_000),
        )
        .unwrap();
        assert_eq!(prepared.key(), key);
        assert_eq!(catalogue.classify(outside), ProcessOwner::Residual);
        assert!(matches!(
            catalogue.admit_code(
                CapturedProcessCode {
                    source: outside,
                    hash: keccak256(program.bytes()),
                    fork: program.fork()
                },
                &mut WorkBudget::new(2_000_000)
            ),
            Err(ProcessOwnerError::UncapturedCode(_))
        ));
        joined = joined
            .join(
                &prepared,
                values.borrowed(),
                &mut WorkBudget::new(20_000_000),
            )
            .unwrap();
        assert!(
            joined
                .equivalent(&stable, values.borrowed(), &mut WorkBudget::new(20_000_000))
                .unwrap(),
            "outside deployment must not grow the retained input layout or bounds"
        );
        let covering = joined
            .instantiate_for_process(values.borrowed(), &mut WorkBudget::new(20_000_000))
            .unwrap();
        assert!(
            covering.addresses().len() <= 2,
            "outside owners must not become keys"
        );
        assert!(covering.code(outside).is_none_or(|c| *c == Code::Unknown));
        assert!(
            covering
                .read(outside, &word(i), values.borrowed())
                .contains(U256::from(i + 1))
        );
        assert!(covering.read_balance(outside).contains(U256::MAX));
        assert!(
            covering
                .read(address(A), &word(0), values.borrowed())
                .contains(U256::from(7))
        );
        assert_eq!(catalogue.named().len(), 1);
        assert_eq!(catalogue.codes().len(), 1);
        assert_eq!(catalogue.unknown_targets().count(), 2);
    }
}

#[test]
fn source_hash_fork_or_membership_replacement_requires_a_new_epoch() {
    let code = CapturedProcessCode {
        source: address(A),
        hash: keccak256([0x00]),
        fork: Fork::Osaka,
    };
    let freeze = |names: &[alloy_primitives::Address], codes: &[CapturedProcessCode]| {
        OwnerCatalogue::freeze(
            names,
            codes,
            OwnerCatalogueLimits { names: 2, codes: 2 },
            &mut WorkBudget::new(2_000_000),
        )
        .unwrap()
    };
    let original = freeze(&[address(A)], &[code]);
    let key = original.key(ProcessOwner::Named(address(A))).unwrap();
    for changed in [
        CapturedProcessCode {
            hash: B256::ZERO,
            ..code
        },
        CapturedProcessCode {
            fork: Fork::Cancun,
            ..code
        },
        CapturedProcessCode {
            source: address(0x1000),
            ..code
        },
    ] {
        assert!(
            matches!(original.admit_code(changed, &mut WorkBudget::new(2_000_000)),
            Err(ProcessOwnerError::UncapturedCode(c)) if c == changed)
        );
        let replacement = freeze(&[address(A), changed.source], &[changed]);
        assert_ne!(replacement.epoch(), original.epoch());
        assert_ne!(
            replacement.key(ProcessOwner::Named(address(A))).unwrap(),
            key
        );
    }
}
