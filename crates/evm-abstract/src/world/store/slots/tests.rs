//! Checked reads traverse admitted histories on each actual numeric carrier.

use crate::{
    Fork,
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, FiniteSetAlgebra, ProductAlgebra, ValueAlgebra,
        relational::RelationLimits, symbolic::ExprId,
    },
    world::{EvmAccount, EvmStore, EvmWorld},
};
use alloy_primitives::{Address, U256};
use std::collections::BTreeSet;

const WRITES: usize = 4096;

fn deep_history<D: EvmNumericAlgebra>(numeric: D) {
    let algebra = ValueAlgebra::new(numeric);
    let domain = algebra.borrowed();
    let owner = Address::repeat_byte(0x61);
    let mut world = EvmWorld::<D::Element>::new(Fork::Osaka, "deep checked slot history");
    world.insert(owner, EvmAccount::empty()).unwrap();
    let mut store = EvmStore::new_checked(&world, domain).unwrap();
    for index in 0..WRITES {
        store
            .write_checked(
                owner,
                &EvmValue::constant(U256::from(index)),
                &EvmValue::constant(U256::from(index + 1)),
                domain,
            )
            .unwrap();
    }
    let observations = [(0, 1), (WRITES - 1, WRITES), (WRITES, 0)];
    // An exact numeric key can retain its input term. This reaches the numeric
    // reader without first recursively folding literals in ExprId::slot_read.
    let key = |index: usize| {
        EvmValue::<D::Element>::constant(U256::from(index))
            .with_expression(ExprId::fresh().unwrap())
    };
    for (index, expected) in observations {
        assert_eq!(
            store
                .read_checked(owner, &key(index), domain)
                .unwrap()
                .numeric(),
            EvmValue::<D::Element>::constant(U256::from(expected)).numeric(),
        );
    }

    // A may-alias continuation and both ITE branches must still traverse the
    // history, including the unlisted zero region beyond all concrete writes.
    let baseline = store.clone();
    let saved = store.snapshot();
    let replacement = U256::from(1_000_000);
    store
        .write_checked(
            owner,
            &EvmValue::top(),
            &EvmValue::constant(replacement),
            domain,
        )
        .unwrap();
    let joined = baseline
        .join_guarded(
            &store,
            domain,
            ExprId::fresh().unwrap(),
            domain.spec().relations().expressions(),
        )
        .unwrap();
    for effect in [&store, &joined] {
        for (index, original) in observations {
            let expected = BTreeSet::from([U256::from(original), replacement]);
            let actual = effect.read_checked(owner, &key(index), domain).unwrap();
            assert_eq!(actual.constants(), Some(&expected));
            assert!(expected.iter().all(|value| actual.contains(*value)));
        }
    }
    store.restore(saved);
    assert_eq!(store, baseline);
    for (index, expected) in observations {
        assert_eq!(
            store
                .read_checked(owner, &key(index), domain)
                .unwrap()
                .singleton(),
            Some(U256::from(expected)),
        );
    }
}

#[test]
fn deep_slot_history_keeps_checked_reads_on_native_carriers() {
    // The checked 4096-slot workload also admits eight later writes, a join and
    // its read: 6*S + 3*W + 6 nodes, S + W + 3 depth. Keep that finite policy.
    let spec = DomainSpec::default().with_relations(RelationLimits {
        max_nodes: 6 * WRITES + 3 * 8 + 6,
        max_depth: WRITES + 8 + 3,
        ..RelationLimits::default()
    });
    deep_history(ProductAlgebra::from_spec(spec));
    deep_history(FiniteSetAlgebra::from_spec(spec));
}
