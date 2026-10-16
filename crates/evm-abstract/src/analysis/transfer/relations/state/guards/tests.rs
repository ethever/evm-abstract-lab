//! Direct branch facts refine live and rollback wrappers without native queries.
use crate::{
    Fork,
    analysis::{
        ExecutionConfig, FrontierReason,
        transfer::{TransferContext, initial},
    },
    domain::{AbstractValue, Domain, symbolic::ExprId},
    resource::WorkBudget,
    world::{Account, ByteArray, Entry, Snapshot, World},
};
use alloy_primitives::{Address, U256};

#[test]
fn direct_guards_refine_live_bytes_and_saved_storage_together() {
    let domain = Domain::default();
    let config = ExecutionConfig::default();
    let address = Address::repeat_byte(0x11);
    let mut world = World::offline(Fork::Osaka, "local guards", "unknown storage");
    world.insert(address, Account::unknown()).unwrap();
    let mut payload = initial(&world, &Entry::new(address), domain).unwrap();
    let guard = ExprId::fresh().unwrap();
    let bytes = ByteArray::exact(&[1])
        .join_guarded(
            &ByteArray::exact(&[3]),
            domain,
            guard.clone(),
            config.analysis.relations.expressions(),
        )
        .unwrap();
    payload.active_mut().memory = bytes.clone();
    payload.active_mut().returndata = bytes;
    let before = payload.store.clone();
    let mut changed = before.clone();
    changed
        .write_checked(
            address,
            &AbstractValue::constant(U256::ZERO),
            &AbstractValue::constant(U256::from(7)),
            domain,
        )
        .unwrap();
    let guarded = changed
        .join_guarded(
            &before,
            domain,
            guard.clone(),
            config.analysis.relations.expressions(),
        )
        .unwrap();
    payload.store = guarded.clone();
    payload.active_mut().saved_store = Snapshot::from_state(guarded);
    payload
        .relations
        .assume(&guard, true, &config.analysis.relations);
    let relations = payload.relations.clone();
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
        super::refine(
            &mut payload,
            &relations,
            &mut context,
            &mut diagnostics,
            0,
            (domain, config.analysis.relations.expressions())
        ),
        Ok(true)
    );
    assert_eq!(payload.active().memory, ByteArray::exact(&[1]));
    assert_eq!(payload.active().returndata, ByteArray::exact(&[1]));
    assert!(diagnostics.is_empty());
    assert_eq!(payload.store, changed);
    assert_eq!(payload.active().saved_store.state(), &changed);
}

#[test]
fn absent_guard_proof_retains_wrapper_and_work_refusal_stops_before_rewrite() {
    let domain = Domain::default();
    let config = ExecutionConfig::default();
    let address = Address::repeat_byte(0x22);
    let mut world = World::offline(Fork::Osaka, "local guards", "unknown condition");
    world.insert(address, Account::empty()).unwrap();
    let mut payload = initial(&world, &Entry::new(address), domain).unwrap();
    let guard = ExprId::fresh().unwrap();
    payload.active_mut().memory = ByteArray::exact(&[1])
        .join_guarded(
            &ByteArray::exact(&[3]),
            domain,
            guard,
            config.analysis.relations.expressions(),
        )
        .unwrap();
    let before = payload.clone();
    let relations = payload.relations.clone();
    let mut diagnostics = Vec::new();
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
    assert_eq!(
        super::refine(
            &mut payload,
            &relations,
            &mut context,
            &mut diagnostics,
            0,
            (domain, config.analysis.relations.expressions())
        ),
        Ok(true)
    );
    assert_eq!(payload, before);
    assert!(diagnostics.is_empty());
    let mut budget = WorkBudget::new(0);
    context.budget = &mut budget;
    assert_eq!(
        super::refine(
            &mut payload,
            &relations,
            &mut context,
            &mut diagnostics,
            0,
            (domain, config.analysis.relations.expressions())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(payload, before);
}
