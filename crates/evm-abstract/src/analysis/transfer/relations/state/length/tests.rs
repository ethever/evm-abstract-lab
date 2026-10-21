//! The production consumer queries only the composite length, never its arms.

use crate::domain::ValueAlgebra;

use crate::{
    Fork,
    analysis::{ExecutionConfig, transfer::initial},
    domain::{
        AbstractValue, Domain,
        relational::{QueryReason, RelationState, ScalarQuery},
        symbolic::{ExprId, ExprLimits},
    },
    resource::WorkBudget,
    world::{Account, ByteArray, Entry, World},
};
use alloy_primitives::{Address, U256};

fn v(n: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(n))
}

fn before() -> ByteArray {
    let mut memory = ByteArray::memory();
    let mut bytes = vec![0xaa; 32];
    bytes.extend([0xbb; 32]);
    memory
        .copy_from(
            &v(32),
            &ByteArray::exact(&bytes),
            &v(0),
            &v(64),
            128,
            Domain::default(),
        )
        .unwrap();
    memory
}

#[test]
fn composite_proof_narrows_both_unknown_arms_without_changing_guard_or_frozen_read() {
    let domain = Domain::default();
    let config = ExecutionConfig::default();
    let limits = config.analysis.relations;
    let selector = ExprId::fresh().unwrap();
    let returned = ByteArray::unknown();
    let reverted = ByteArray::unknown();
    let mut yes = before();
    let mut no = yes.clone();
    yes.copy_return_data(&v(32), &returned, &v(16), 128, domain)
        .unwrap();
    no.copy_return_data(&v(32), &reverted, &v(16), 128, domain)
        .unwrap();
    let memory = yes
        .join_guarded(&no, domain, selector.clone(), limits.expressions())
        .unwrap();
    let history = memory.clone();
    let frozen_read = history.read_byte_checked(&v(33), domain).unwrap();
    assert!(frozen_read.contains(U256::ZERO));
    assert!(frozen_read.contains(U256::from(0xaa)));
    let returndata = returned
        .join_guarded(&reverted, domain, selector.clone(), limits.expressions())
        .unwrap();
    let expression = returndata.len().expression().unwrap().clone();
    assert!(returndata.len().numeric().is_top());
    let mut relations = RelationState::default();
    relations
        .observe(&expression, v(1).numeric(), &limits)
        .unwrap();
    let original_relations = relations.clone();
    let address = Address::repeat_byte(0x71);
    let mut world = World::offline(Fork::Osaka, "length consequences", "paired unknown return");
    world.insert(address, Account::empty()).unwrap();
    let mut payload = initial(&world, &Entry::new(address), ValueAlgebra::new(&domain)).unwrap();
    payload.active_mut().memory = memory;
    payload.active_mut().returndata = returndata.clone();
    let mut budget = WorkBudget::new(1_000_000);
    let mut context = crate::analysis::transfer::testing::context(
        &config,
        &mut budget,
        ValueAlgebra::new(&domain),
    );
    assert_eq!(
        super::refine(&mut payload, &relations, &mut context),
        Ok(())
    );
    let memory = &payload.active().memory;
    assert_eq!(memory.state_root(), history.state_root());
    assert_eq!(memory.fresh_leaves(), history.fresh_leaves());
    assert_eq!(memory.len(), history.len());
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
    // Each source is exact only under its original selector. The shared F is
    // unchanged and still proves neither truth value of that selector.
    for arm in [&returned, &reverted] {
        assert!(
            !memory
                .symbolic_facts()
                .any(|(e, _)| Some(e) == arm.len().expression())
        );
    }
    assert_eq!(relations, original_relations);
    assert_eq!(
        relations.refine_numeric(&selector, AbstractValue::top().numeric(), domain, &limits),
        ScalarQuery::Unchanged
    );
    assert_eq!(payload.active().returndata, returndata);
    let once = payload.active().memory.clone();
    assert_eq!(
        super::refine(&mut payload, &relations, &mut context),
        Ok(())
    );
    assert_eq!(payload.active().memory, once);
    assert_eq!(
        history.read_byte_checked(&v(33), domain).unwrap(),
        frozen_read
    );
    assert!(!context.budget.exhausted());
}

#[test]
fn cancellation_and_optional_query_planning_budget_or_metadata_refusal_are_atomic() {
    let domain = Domain::default();
    let config = ExecutionConfig::default();
    let address = Address::repeat_byte(0x72);
    let mut world = World::offline(Fork::Osaka, "length admission", "optional precision");
    world.insert(address, Account::empty()).unwrap();
    let mut original = initial(&world, &Entry::new(address), ValueAlgebra::new(&domain)).unwrap();
    let returndata = ByteArray::unknown();
    let expression = returndata.len().expression().unwrap().clone();
    let mut memory = before();
    memory
        .copy_return_data(&v(32), &returndata, &v(16), 128, domain)
        .unwrap();
    original.active_mut().memory = memory;
    original.active_mut().returndata = returndata;
    let mut relations = RelationState::default();
    relations
        .observe(&expression, v(1).numeric(), &config.analysis.relations)
        .unwrap();
    // Too little owner capacity for the fixed local quantum is optional, even
    // after paying cache preparation. It must not become a Work frontier.
    for capacity in [0, super::QUERY_WORK - 1] {
        let mut payload = original.clone();
        let mut budget = WorkBudget::new(capacity);
        let mut context = crate::analysis::transfer::testing::context(
            &config,
            &mut budget,
            ValueAlgebra::new(&domain),
        );
        assert_eq!(
            super::refine(&mut payload, &relations, &mut context),
            Ok(())
        );
        assert_eq!(payload, original);
        assert!(!context.budget.exhausted());
    }
    // The private attempt can also run out of its quantum after admission.
    // Its full reservation stays paid, while the owner's ledger stays usable.
    let mut expensive = relations.clone();
    let range = AbstractValue::unsigned_range(U256::from(3), U256::from(9)).unwrap();
    for _ in 0..8 {
        expensive
            .observe(
                &ExprId::fresh().unwrap(),
                range.numeric(),
                &config.analysis.relations,
            )
            .unwrap();
    }
    let mut payload = original.clone();
    let mut budget = WorkBudget::new(1_000_000);
    let mut context = crate::analysis::transfer::testing::context(
        &config,
        &mut budget,
        ValueAlgebra::new(&domain),
    );
    assert_eq!(
        super::refine(&mut payload, &expensive, &mut context),
        Ok(())
    );
    assert_eq!(payload, original);
    assert!(context.budget.used() >= super::QUERY_WORK);
    assert!(!context.budget.exhausted());
    let proof = v(1).with_expression(expression);
    for (limits, capacity) in [
        (ExprLimits::default(), 0),
        (
            ExprLimits {
                max_nodes: 1,
                max_depth: 1,
            },
            1_000_000,
        ),
    ] {
        let mut memory = original.active().memory.clone();
        let mut budget = WorkBudget::new(capacity);
        assert_eq!(
            memory.import_length_bound(&proof, limits, &mut budget),
            Ok(false)
        );
        assert_eq!(memory, original.active().memory);
        assert!(!budget.exhausted());
    }
    // A large length expression is rejected from cached sizes, before walking
    // it or allocating a distributed fact tree.
    let mut large = ExprId::fresh().unwrap();
    for _ in 0..20 {
        large = ExprId::ite(
            ExprId::fresh().unwrap(),
            large,
            ExprId::fresh().unwrap(),
            ExprLimits::default(),
        )
        .unwrap();
    }
    let mut memory = original.active().memory.clone();
    let mut budget = WorkBudget::new(1_000_000);
    assert_eq!(
        memory.import_length_bound(
            &v(1).with_expression(large),
            ExprLimits::default(),
            &mut budget
        ),
        Ok(false)
    );
    assert_eq!(budget.used(), 0);
    assert_eq!(memory, original.active().memory);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        let mut payload = original.clone();
        let mut budget = WorkBudget::new(1_000_000);
        let mut context = crate::analysis::transfer::testing::context(
            &config,
            &mut budget,
            ValueAlgebra::new(&domain),
        );
        assert_eq!(
            super::refine(&mut payload, &relations, &mut context),
            Err(crate::analysis::FrontierReason::Relations(
                QueryReason::Cancelled
            ))
        );
        assert_eq!(payload, original);
        assert_eq!(
            payload.active_mut().memory.import_length_bound(
                &proof,
                ExprLimits::default(),
                context.budget
            ),
            Err(QueryReason::Cancelled)
        );
        assert_eq!(payload, original);
    });
}
