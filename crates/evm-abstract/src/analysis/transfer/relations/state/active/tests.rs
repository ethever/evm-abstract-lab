//! Exercise the production consumer with a retained minimum, not an injected min fact.

use crate::{
    Fork,
    analysis::{
        ExecutionConfig,
        transfer::{TransferContext, initial},
    },
    domain::{
        AbstractValue, Domain,
        relational::{CheckResult, RelationState, ScalarQuery},
        symbolic::{ByteStateKind, ExprId},
    },
    resource::WorkBudget,
    world::{Account, ByteArray, Entry, World},
};
use alloy_primitives::{Address, U256};

fn v(n: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(n))
}

#[test]
fn proved_return_length_refines_retained_copy_minimum_in_active_and_unconditional_scopes() {
    let domain = Domain::default();
    let config = ExecutionConfig::default();
    for guarded in [false, true] {
        let address = Address::repeat_byte(0x71);
        let mut world = World::offline(Fork::Osaka, "active COPY", "one-byte return");
        world.insert(address, Account::empty()).unwrap();
        let mut payload = initial(&world, &Entry::new(address), domain).unwrap();
        let returndata = ByteArray::unknown();
        let returned_length = returndata.len().expression().unwrap().clone();
        let mut before = ByteArray::memory();
        let mut prior = vec![0xaa; 32];
        prior.extend([0xbb; 32]);
        before
            .copy_from(
                &v(32),
                &ByteArray::exact(&prior),
                &v(0),
                &v(64),
                128,
                domain,
            )
            .unwrap();
        let mut copied = before.clone();
        copied
            .copy_return_data(&v(32), &returndata, &v(16), 128, domain)
            .unwrap();
        let ByteStateKind::Copy { length, .. } = copied.state_root().kind() else {
            panic!("CALL output must retain its compound minimum");
        };
        let minimum = length.clone();
        let old = copied
            .symbolic_facts()
            .find(|(expression, _)| *expression == &minimum)
            .unwrap()
            .1
            .clone();
        let selector = ExprId::fresh().unwrap();
        let memory = if guarded {
            copied
                .join_guarded(
                    &before,
                    domain,
                    selector.clone(),
                    config.analysis.relations.expressions(),
                )
                .unwrap()
        } else {
            copied
        };
        assert_eq!(
            memory
                .symbolic_facts()
                .any(|(expression, _)| expression == &minimum),
            !guarded,
            "the guarded minimum lives exclusively in Scope.facts",
        );
        let history = memory.clone();
        let historical_byte = history.byte_at(33, domain);
        assert!(historical_byte.contains(U256::ZERO));
        assert!(historical_byte.contains(U256::from(0xaa)));
        payload.active_mut().memory = memory;
        let mut relations = RelationState::default();
        relations
            .observe(&returned_length, v(1).numeric(), &config.analysis.relations)
            .unwrap();
        assert_eq!(
            relations.assume(&selector, true, &config.analysis.relations),
            CheckResult::MayBeFeasible,
        );
        assert_eq!(
            relations.refine_numeric(&minimum, old.numeric(), domain, &config.analysis.relations),
            ScalarQuery::Unchanged,
            "retlen==1 is not a direct fact for the minimum key",
        );
        let mut budget = WorkBudget::new(10_000_000);
        let mut context = TransferContext {
            config: &config,
            domain,
            budget: &mut budget,
            root_address: None,
            input_scope: None,
            query_cache: Default::default(),
            procedure: false,
            discover_code: false,
        };
        let mut diagnostics = Vec::new();
        assert_eq!(
            super::super::refine(&mut payload, &relations, &mut context, &mut diagnostics, 0),
            Ok(true),
        );
        assert!(diagnostics.is_empty());
        let memory = &mut payload.active_mut().memory;
        assert_eq!(memory.state_root(), history.state_root());
        assert_eq!(memory.fresh_leaves(), history.fresh_leaves());
        assert_eq!(memory.len().singleton(), Some(U256::from(96)));
        for at in 33..64 {
            assert_eq!(
                memory.byte_at(at, domain).singleton(),
                Some(U256::from(0xaa))
            );
        }
        for at in 64..96 {
            assert_eq!(
                memory.byte_at(at, domain).singleton(),
                Some(U256::from(0xbb))
            );
        }
        let mut witnessed = None;
        assert!(
            memory
                .refine_active_copy_lengths(
                    config.analysis.relations.expressions(),
                    context.budget,
                    super::reserve,
                    &mut |expression, value, _| {
                        assert_eq!(expression, &minimum);
                        witnessed = Some(value.clone());
                        Ok(true)
                    },
                )
                .unwrap()
        );
        let witnessed = witnessed.expect("the retained copy key must remain in its original scope");
        assert_eq!(witnessed.singleton(), Some(U256::ONE));
        assert_eq!(witnessed.with_numeric(old.numeric().clone()), old);
        assert_eq!(history.byte_at(33, domain), historical_byte);
    }
}
