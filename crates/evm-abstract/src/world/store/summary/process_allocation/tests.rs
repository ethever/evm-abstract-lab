//! Actual Store witnesses for the frozen central transformation, not dispatch.
use super::super::EvmStoreSummary;
use super::ProcessOwner;
use crate::analysis::process_creation::Collision;
use crate::analysis::{ProcessAction, process_creation as creation};
use crate::world::process::{CapturedProcessCode, OwnerCatalogue, OwnerCatalogueLimits};
use crate::{
    Fork,
    bytecode::Program,
    domain::{EvmValue, NumericValue, ProductAlgebra, ValueAlgebra},
    resource::{CloneError, WorkBudget},
    world::{Code, EvmAccount, EvmStore, EvmWorld, Existence},
};
use alloy_primitives::{Address, B256, U256};
use std::sync::Arc;

#[path = "fees.rs"]
mod fees;
#[path = "witness.rs"]
mod witness;

type Domain = ValueAlgebra<ProductAlgebra>;
type Store = EvmStore<NumericValue>;
type State = EvmStoreSummary<NumericValue>;
fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}
fn word(value: u64) -> EvmValue<NumericValue> {
    EvmValue::constant(U256::from(value))
}
fn domain() -> Domain {
    ValueAlgebra::new(ProductAlgebra::default())
}
fn catalogue(names: &[Address]) -> Arc<OwnerCatalogue> {
    OwnerCatalogue::freeze(
        names,
        &[],
        OwnerCatalogueLimits {
            names: names.len(),
            codes: 0,
        },
        &mut WorkBudget::new(10_000_000),
    )
    .unwrap()
}
fn initial(fork: Fork, creator: Address, destination: Address) -> (EvmWorld<NumericValue>, Store) {
    let mut world = EvmWorld::new(fork, "allocation frozen central witness");
    let mut account = EvmAccount::from_hex("00", fork).unwrap();
    account.balance = word(100);
    account.nonce = word(7);
    world.insert(creator, account).unwrap();
    let mut account = EvmAccount::empty();
    account.balance = word(23);
    account.storage_unknown = true;
    account.storage.insert(U256::from(5), word(9));
    world.insert(destination, account).unwrap();
    world.insert(address(3), EvmAccount::empty()).unwrap();
    let d = domain();
    let mut store = Store::new(&world, d.borrowed());
    store.write_transient(destination, &word(5), &word(11), d.borrowed());
    (world, store)
}
fn prepared(store: &Store, names: &OwnerCatalogue, d: &Domain, budget: &mut WorkBudget) -> State {
    store
        .summary_projection_with_payment(d.borrowed(), budget)
        .unwrap()
        .process_complement(names, d.borrowed(), budget)
        .unwrap()
}
fn covers(upper: &State, native: &Store, names: &OwnerCatalogue, d: &Domain) -> bool {
    let mut budget = WorkBudget::new(100_000_000);
    let upper = upper
        .instantiate_with_payment(d.borrowed(), &mut budget)
        .unwrap()
        .summary_projection_with_payment(d.borrowed(), &mut budget)
        .unwrap();
    // Compare in the SAME owner quotient. Raw native acquisition-dependency
    // must-facts (outside global false) are not process replay dependencies
    // (independent Top, outside global true). Both numeric planes, every named
    // field, defaults, lifecycle and logs still participate in whole coverage.
    let lower = prepared(native, names, d, &mut budget);
    upper.covers(&lower, d.borrowed())
}
// Same guard arguments and self-collision override as frozen process/allocation.rs.
fn plan(
    state: &State,
    names: &OwnerCatalogue,
    creator: ProcessOwner,
    action: ProcessAction,
    budget: &mut WorkBudget,
) -> creation::CreationPlan {
    let nonce = state.process_nonce(creator, budget).unwrap();
    let targets: Vec<_> = names
        .unknown_targets()
        .map(|owner| creation::Destination {
            owner,
            collision: if owner == creator && owner != ProcessOwner::Residual {
                Collision::NonZeroNonce
            } else {
                state.process_collision(owner, budget).unwrap()
            },
        })
        .collect();
    creation::prepare(
        creation::CreationRequest {
            action,
            creator,
            is_static: false,
            depth: creation::Admission::Unknown,
            nonce: nonce.map_or(
                creation::CreatorNonce::Unknown,
                creation::CreatorNonce::Exact,
            ),
            funding: creation::Admission::Unknown,
            initcode_size: creation::Admission::Unknown,
            code: creation::ConstructorCode::AnyInitCode,
        },
        &targets,
        0,
        budget,
    )
    .unwrap()
}

#[test]
fn named_begin_covers_checked_endowment_prefund_and_resets_both_planes() {
    let (creator, destination) = (address(0x101), address(0x202));
    let d = domain();
    let names = catalogue(&[creator, destination, address(3)]);
    let (_, mut native) = initial(Fork::Cancun, creator, destination);
    let mut budget = WorkBudget::new(100_000_000);
    let before = prepared(&native, &names, &d, &mut budget);
    assert_eq!(
        before
            .process_collision(ProcessOwner::Named(destination), &mut budget)
            .unwrap(),
        Collision::Available
    );
    let post = before
        .process_increment_nonce(ProcessOwner::Named(creator), d.borrowed(), &mut budget)
        .unwrap();
    native.write_nonce(creator, word(8));
    assert!(covers(&post, &native, &names, &d));
    let entry = post
        .process_begin_constructor(
            ProcessOwner::Named(creator),
            ProcessOwner::Named(destination),
            d.borrowed(),
            &mut budget,
        )
        .unwrap();
    // Concrete successful guard: value 7, funds 100, prefund 23. Top need only
    // cover these checked results; it is not an affordability certificate.
    native.write_balance(creator, word(93));
    native.write_balance(destination, word(30));
    native.begin_creation(destination);
    assert!(covers(&entry, &native, &names, &d));
    let replay = entry
        .instantiate_with_payment(d.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(
        replay.read(destination, &word(5), d.borrowed()).singleton(),
        Some(U256::ZERO)
    );
    assert_eq!(
        replay
            .read_transient(destination, &word(5), d.borrowed())
            .singleton(),
        Some(U256::ZERO)
    );
    assert_eq!(replay.nonce(destination).singleton(), Some(U256::ONE));
    assert!(matches!(replay.code(destination), Some(Code::Empty)));
    assert_eq!(replay.existence(destination), Existence::Present);
    assert_eq!(replay.created_in_transaction(destination), Some(true));
    assert_eq!(replay.pending_destruction(destination), Some(false));
    assert_eq!(replay.touched_in_transaction(destination), Some(true));
    assert!(entry.balances.get(&creator).unwrap().numeric().is_top());
    assert!(entry.balances.get(&destination).unwrap().numeric().is_top());
}

#[test]
fn constructor_reset_has_no_initial_storage_acquisition_dependency() {
    let (creator, destination) = (address(0x101), address(0x202));
    let d = domain();
    let names = catalogue(&[creator, destination, address(3)]);
    let (world, native) = initial(Fork::Cancun, creator, destination);
    let mut budget = WorkBudget::new(100_000_000);
    let input = prepared(&native, &names, &d, &mut budget);
    let post = input
        .process_increment_nonce(ProcessOwner::Named(creator), d.borrowed(), &mut budget)
        .unwrap();
    let child = post
        .process_begin_constructor(
            ProcessOwner::Named(creator),
            ProcessOwner::Named(destination),
            d.borrowed(),
            &mut budget,
        )
        .unwrap();
    let replay = child
        .instantiate_with_payment(d.borrowed(), &mut budget)
        .unwrap();
    assert!(
        replay
            .missing_initial_slots(&world, destination, &word(123))
            .is_empty(),
        "creation reset must sever the old World/RPC storage dependency"
    );
}

#[test]
fn unknown_collision_keeps_both_branches_and_occupied_code_is_preserved() {
    let d = domain();
    let (creator, unknown, occupied) = (address(0x101), address(0x202), address(0x303));
    let names = catalogue(&[creator, unknown, occupied, address(3)]);
    let (_, native) = initial(Fork::Cancun, creator, unknown);
    let mut budget = WorkBudget::new(100_000_000);
    let mut state = prepared(&native, &names, &d, &mut budget);
    state.codes.remove(&unknown);
    state.nonces.remove(&unknown);
    state.codes.insert(
        occupied,
        Code::Runtime(Program::from_hex("600100").unwrap()),
    );
    state.nonces.insert(occupied, word(0));
    assert_eq!(
        state
            .process_collision(ProcessOwner::Named(unknown), &mut budget)
            .unwrap(),
        Collision::Unknown
    );
    for action in [ProcessAction::Create, ProcessAction::Create2] {
        let alternatives = plan(
            &state,
            &names,
            ProcessOwner::Named(creator),
            action,
            &mut budget,
        );
        assert!(
            alternatives
                .constructors
                .iter()
                .any(|e| e.destination == ProcessOwner::Named(unknown))
        );
        assert!(
            alternatives
                .rejections
                .iter()
                .any(|r| r.destination == Some(ProcessOwner::Named(unknown)) && r.increments_nonce)
        );
        assert!(
            !alternatives
                .constructors
                .iter()
                .any(|e| e.destination == ProcessOwner::Named(occupied)
                    || e.destination == ProcessOwner::Named(creator))
        );
        let post = state
            .process_increment_nonce(ProcessOwner::Named(creator), d.borrowed(), &mut budget)
            .unwrap();
        assert_eq!(post.codes.get(&occupied), state.codes.get(&occupied));
        assert_eq!(
            post.process_nonce(ProcessOwner::Named(creator), &mut budget)
                .unwrap(),
            Some(8)
        );
    }
}

#[test]
fn max_nonce_and_underfunding_keep_pre_nonce_state() {
    let d = domain();
    let (creator, destination) = (address(0x101), address(0x202));
    let names = catalogue(&[creator, destination, address(3)]);
    let (_, native) = initial(Fork::Cancun, creator, destination);
    let mut budget = WorkBudget::new(100_000_000);
    let mut state = prepared(&native, &names, &d, &mut budget);
    state
        .nonces
        .insert(creator, EvmValue::constant(U256::from(u64::MAX)));
    let p = plan(
        &state,
        &names,
        ProcessOwner::Named(creator),
        ProcessAction::Create2,
        &mut budget,
    );
    assert!(p.constructors.is_empty());
    assert!(p.rejections.iter().all(|r| !r.increments_nonce));
    assert_eq!(
        state
            .process_nonce(ProcessOwner::Named(creator), &mut budget)
            .unwrap(),
        Some(u64::MAX)
    );
    // Frozen central callsite uses Unknown funding, so both a rejection with
    // unchanged state and an affordable conditional entry must remain present.
    state.nonces.insert(creator, word(7));
    state.balances.insert(creator, word(0));
    let p = plan(
        &state,
        &names,
        ProcessOwner::Named(creator),
        ProcessAction::Create,
        &mut budget,
    );
    assert!(p.rejections.iter().any(|r| r.reason
        == creation::RejectionReason::InsufficientBalance
        && !r.increments_nonce));
    assert!(!p.constructors.is_empty());
    assert_eq!(
        state.nonces.get(&creator).unwrap().singleton(),
        Some(U256::from(7))
    );
}

#[test]
fn residual_same_class_aliases_cover_distinct_and_same_accounts_without_key_growth() {
    let d = domain();
    let creator = address(0x101);
    let named = address(0x202);
    let outside_a = address(0xaaa);
    let outside_b = address(0xbbb);
    let names = catalogue(&[creator, named, address(3)]);
    let (_, native) = initial(Fork::Cancun, creator, named);
    let mut budget = WorkBudget::new(100_000_000);
    let state = prepared(&native, &names, &d, &mut budget);
    let post = state
        .process_increment_nonce(ProcessOwner::Residual, d.borrowed(), &mut budget)
        .unwrap();
    let child = post
        .process_begin_constructor(
            ProcessOwner::Residual,
            ProcessOwner::Residual,
            d.borrowed(),
            &mut budget,
        )
        .unwrap();
    assert_eq!(child, post); // Canonical independent Top is already a weak cover.
    let keys = child
        .instantiate_with_payment(d.borrowed(), &mut budget)
        .unwrap()
        .addresses();
    assert!(!keys.contains(&outside_a) && !keys.contains(&outside_b));
    for (from, to) in [(outside_a, outside_b), (outside_b, outside_a)] {
        let mut actual = native.clone();
        actual.write_nonce(from, word(3));
        actual.write_balance(from, word(93));
        actual.write_balance(to, word(30));
        actual.begin_creation(to);
        assert!(covers(&child, &actual, &names, &d));
    }
    // Same instance is collision-only after nonce increment; the plan keeps it
    // among residual collision possibilities rather than proving class aliases.
    let p = plan(
        &post,
        &names,
        ProcessOwner::Residual,
        ProcessAction::Create,
        &mut budget,
    );
    assert!(
        p.rejections
            .iter()
            .any(|r| r.destination == Some(ProcessOwner::Residual) && r.increments_nonce)
    );
    assert!(
        p.constructors
            .iter()
            .any(|e| e.destination == ProcessOwner::Residual)
    );
    let deployed = child
        .process_deploy(ProcessOwner::Residual, d.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(deployed, child);
}

#[test]
fn all_named_residual_creator_destination_pairs_cover_native_success_guard() {
    let d = domain();
    let (creator, named) = (address(0x101), address(0x202));
    let names = catalogue(&[creator, named, address(3)]);
    let (_, native) = initial(Fork::Cancun, creator, named);
    let mut budget = WorkBudget::new(100_000_000);
    let state = prepared(&native, &names, &d, &mut budget);
    for (from, to) in [
        (creator, named),
        (creator, address(0xaaa)),
        (address(0xaaa), named),
        (address(0xaaa), address(0xbbb)),
    ] {
        let src = names.classify(from);
        let dst = names.classify(to);
        let post = state
            .process_increment_nonce(src, d.borrowed(), &mut budget)
            .unwrap();
        let child = post
            .process_begin_constructor(src, dst, d.borrowed(), &mut budget)
            .unwrap();
        let mut actual = native.clone();
        actual.write_nonce(from, word(if from == creator { 8 } else { 3 }));
        actual.write_balance(from, word(93));
        actual.write_balance(to, word(30));
        actual.begin_creation(to);
        assert!(covers(&child, &actual, &names, &d), "{src:?} -> {dst:?}");
        assert_eq!(child.codes.get(&creator), state.codes.get(&creator));
    }
}

#[test]
fn full_catalogue_epoch_changes_for_code_digest_and_fork() {
    let owner = address(0x101);
    let mut epochs = Vec::new();
    for (hash, fork) in [
        (B256::ZERO, Fork::Cancun),
        (B256::repeat_byte(0x80), Fork::Cancun),
        (B256::ZERO, Fork::Prague),
    ] {
        let frozen = OwnerCatalogue::freeze(
            &[owner],
            &[CapturedProcessCode {
                source: owner,
                hash,
                fork,
            }],
            OwnerCatalogueLimits { names: 1, codes: 1 },
            &mut WorkBudget::new(1_000_000),
        )
        .unwrap();
        let key = frozen.key(ProcessOwner::Residual).unwrap();
        assert_eq!(key.epoch, frozen.epoch());
        epochs.push(key.epoch);
    }
    assert!(epochs[0] != epochs[1] && epochs[1] != epochs[2] && epochs[0] != epochs[2]);
}

#[test]
fn collision_and_nonce_fast_returns_honor_both_cancellation_owners() {
    let d = domain();
    let (creator, destination) = (address(0x101), address(0x202));
    let names = catalogue(&[creator, destination, address(3)]);
    let (_, native) = initial(Fork::Cancun, creator, destination);
    let state = prepared(&native, &names, &d, &mut WorkBudget::new(100_000_000));
    let captured = analysis_control::Cancellation::new();
    let mut budget = analysis_control::with_cancellation(&captured, || WorkBudget::new(1_000_000));
    captured.cancel();
    for owner in [ProcessOwner::Residual, ProcessOwner::Named(creator)] {
        assert!(matches!(
            state.process_collision(owner, &mut budget),
            Err(CloneError::Cancelled)
        ));
        assert!(matches!(
            state.process_nonce(owner, &mut budget),
            Err(CloneError::Cancelled)
        ));
    }
    let current = analysis_control::Cancellation::new();
    current.cancel();
    let mut budget = WorkBudget::new(1_000_000);
    analysis_control::with_cancellation(&current, || {
        assert!(matches!(
            state.process_collision(ProcessOwner::Residual, &mut budget),
            Err(CloneError::Cancelled)
        ));
        assert!(matches!(
            state.process_nonce(ProcessOwner::Residual, &mut budget),
            Err(CloneError::Cancelled)
        ));
    });
}

#[test]
fn exhausted_query_fast_paths_return_work_without_any_evm_branch() {
    let d = domain();
    let (creator, destination) = (address(0x101), address(0x202));
    let names = catalogue(&[creator, destination, address(3)]);
    let (_, native) = initial(Fork::Cancun, creator, destination);
    let state = prepared(&native, &names, &d, &mut WorkBudget::new(100_000_000));
    let mut budget = WorkBudget::new(0);
    assert!(!budget.charge(1));
    for owner in [ProcessOwner::Residual, ProcessOwner::Named(creator)] {
        assert!(matches!(
            state.process_nonce(owner, &mut budget),
            Err(CloneError::Work)
        ));
        assert!(matches!(
            state.process_collision(owner, &mut budget),
            Err(CloneError::Work)
        ));
    }
}

#[test]
fn unknown_nonce_increment_covers_every_accepted_protocol_endpoint() {
    let d = domain();
    let (creator, destination) = (address(0x101), address(0x202));
    let names = catalogue(&[creator, destination, address(3)]);
    let (_, native) = initial(Fork::Cancun, creator, destination);
    let mut budget = WorkBudget::new(100_000_000);
    let mut input = prepared(&native, &names, &d, &mut budget);
    input.nonces.remove(&creator);
    let post = input
        .process_increment_nonce(ProcessOwner::Named(creator), d.borrowed(), &mut budget)
        .unwrap();
    let incremented = post.nonces.get(&creator).unwrap();
    assert!(incremented.numeric().contains(U256::ONE));
    assert!(incremented.numeric().contains(U256::from(u64::MAX)));
    assert!(!incremented.numeric().contains(U256::ZERO));
    input
        .nonces
        .insert(creator, EvmValue::constant(U256::from(u64::MAX - 1)));
    let post = input
        .process_increment_nonce(ProcessOwner::Named(creator), d.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(
        post.process_nonce(ProcessOwner::Named(creator), &mut budget)
            .unwrap(),
        Some(u64::MAX)
    );
}

#[test]
fn residual_raw_store_counterexample_is_removed_by_actual_complement_preparation() {
    let d = domain();
    let (creator, outside) = (address(0x101), address(0xaaa));
    let (_, native) = initial(Fork::Cancun, creator, outside);
    let names = catalogue(&[creator, address(3)]);
    let mut budget = WorkBudget::new(100_000_000);
    let raw = native
        .summary_projection_with_payment(d.borrowed(), &mut budget)
        .unwrap();
    let bad = raw
        .process_begin_constructor(
            ProcessOwner::Named(creator),
            ProcessOwner::Residual,
            d.borrowed(),
            &mut budget,
        )
        .unwrap();
    // Existing API is conditional on canonical input. This is an executable
    // counterexample to omitting that documented central caller obligation.
    assert_eq!(
        bad.nonces.get(&outside).unwrap().singleton(),
        Some(U256::ZERO)
    );
    let canonical = raw
        .process_complement(&names, d.borrowed(), &mut budget)
        .unwrap();
    let child = canonical
        .process_begin_constructor(
            ProcessOwner::Named(creator),
            ProcessOwner::Residual,
            d.borrowed(),
            &mut budget,
        )
        .unwrap();
    assert!(child.nonces.get(&outside).is_none());
    assert!(
        child
            .instantiate_with_payment(d.borrowed(), &mut budget)
            .unwrap()
            .nonce(outside)
            .numeric()
            .contains(U256::ONE)
    );
    assert_eq!(names.classify(outside), ProcessOwner::Residual);
}
