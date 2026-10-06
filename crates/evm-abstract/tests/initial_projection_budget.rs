//! 初始化的完整候选投影必须先经过同一工作预算。

use evm_abstract::{
    Address, Fork, U256,
    analysis::{Config, ExecutionConfig, FrontierReason, Status, analyze_world},
    domain::{
        Domain, DomainSpec, Profile, Value,
        facts::{UnaryPredicate, WordBounds},
    },
    render,
    world::{Account, ByteArray, Entry, World},
};
use std::num::NonZeroUsize;

fn open_range(unknown_bits: usize) -> Value {
    open_interval(U256::ZERO, (U256::from(1) << unknown_bits) - U256::from(1))
}

fn open_interval(lower: U256, upper: U256) -> Value {
    let domain = Domain::from_spec(DomainSpec::new(
        Profile::Product,
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(256).unwrap(),
    ));
    let value = domain
        .from_facts(&[UnaryPredicate::UnsignedBounds(
            WordBounds::new(lower, upper).unwrap(),
        )])
        .unwrap();
    assert!(value.constants().is_none());
    value
}

fn fixture() -> (World, Entry) {
    let address = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "initial projection budget");
    world
        .insert(address, Account::from_hex("00", world.fork()).unwrap())
        .unwrap();
    let entry = Entry {
        address: address,
        environment: evm_abstract::world::EvmEnvironment {
            to: (address).into(),
            caller: (Address::ZERO).into(),
            value: Value::constant(U256::ZERO),
            calldata: ByteArray::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironment::default()
        },
    };
    (world, entry)
}

fn constants_config(capacity: usize, max_work: usize) -> ExecutionConfig {
    ExecutionConfig {
        analysis: Config {
            domain_profile: Profile::ConstantsOnly,
            max_constants: capacity,
            ..Config::default()
        },
        max_work,
        ..ExecutionConfig::default()
    }
}

fn assert_initial_work_frontier(graph: &evm_abstract::analysis::WorldAnalysis) {
    assert_eq!(graph.status(), Status::Incomplete);
    assert!(graph.states().is_empty());
    assert!(graph.outcomes().is_empty());
    assert_eq!(graph.frontiers().len(), 1);
    let frontier = &graph.frontiers()[0];
    assert_eq!(frontier.from, None);
    assert_eq!(frontier.target, None);
    assert_eq!(frontier.pc, None);
    assert_eq!(frontier.reason, FrontierReason::Work);
    assert_eq!(graph.work(), 0);
}

#[test]
fn initial_projection_precharge_covers_open_entry_and_storage_values() {
    for storage in [false, true] {
        let (mut world, mut entry) = fixture();
        if storage {
            let mut account = world.account(entry.address).unwrap().clone();
            account
                .storage
                .insert(U256::ZERO, Value::constant(U256::ZERO));
            world = World::new(Fork::Osaka, "storage projection baseline");
            world.insert(entry.address, account).unwrap();
        }
        let baseline =
            analyze_world(world.clone(), entry.clone(), constants_config(512, 100_000)).unwrap();
        assert_eq!(
            baseline.status(),
            Status::Converged,
            "{:?}",
            baseline.frontiers()
        );
        let value = open_range(9);
        if storage {
            let mut account = world.account(entry.address).unwrap().clone();
            account.storage.insert(U256::ZERO, value);
            world = World::new(Fork::Osaka, "open storage projection budget");
            world.insert(entry.address, account).unwrap();
        } else {
            entry.environment.value = value;
        }
        let graph = analyze_world(world, entry, constants_config(512, 100_000)).unwrap();
        assert_initial_work_frontier(&graph);
    }
}

#[test]
fn initial_projection_precharge_covers_unknown_calldata_bytes() {
    let (world, mut entry) = fixture();
    let baseline =
        analyze_world(world.clone(), entry.clone(), constants_config(256, 100_000)).unwrap();
    assert_eq!(
        baseline.status(),
        Status::Converged,
        "{:?}",
        baseline.frontiers()
    );
    entry.environment.calldata = ByteArray::unknown();
    let graph = analyze_world(world, entry, constants_config(256, 100_000)).unwrap();
    assert_initial_work_frontier(&graph);
}

#[test]
fn initial_projection_precharge_covers_interval_enumeration() {
    let (world, mut entry) = fixture();
    let baseline =
        analyze_world(world.clone(), entry.clone(), constants_config(512, 100_000)).unwrap();
    assert_eq!(
        baseline.status(),
        Status::Converged,
        "{:?}",
        baseline.frontiers()
    );
    let lower = (U256::from(1) << 200usize) - U256::from(1);
    let upper = lower + U256::from(511);
    entry.environment.value = open_interval(lower, upper);
    let bits = entry.environment.value.known_bits();
    assert!((!(bits.zero() | bits.one())).count_ones() >= usize::BITS as usize);
    let graph =
        analyze_world(world.clone(), entry.clone(), constants_config(512, 100_000)).unwrap();
    assert_initial_work_frontier(&graph);
    let graph = analyze_world(world, entry, constants_config(512, 1_000_000)).unwrap();
    assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
    let candidates = graph.states()[0]
        .entry
        .active()
        .call_value
        .constants()
        .unwrap();
    assert_eq!(candidates.len(), 512);
    assert_eq!(candidates.first(), Some(&lower));
    assert_eq!(candidates.last(), Some(&upper));
}

#[test]
fn maximum_capacity_open_projection_stops_even_with_maximum_work() {
    for max_work in [100_000, usize::MAX] {
        for storage in [false, true] {
            let (mut world, mut entry) = fixture();
            let value = open_range(usize::BITS as usize - 1);
            if storage {
                let mut account = world.account(entry.address).unwrap().clone();
                account.storage.insert(U256::ZERO, value);
                world = World::new(Fork::Osaka, "maximum storage projection budget");
                world.insert(entry.address, account).unwrap();
            } else {
                entry.environment.value = value;
            }
            let graph =
                analyze_world(world, entry, constants_config(usize::MAX, max_work)).unwrap();
            assert_initial_work_frontier(&graph);
            assert!(
                render::world::explain(&graph)
                    .unwrap()
                    .contains("SSA unavailable")
            );
        }
    }
}

#[test]
fn maximum_capacity_top_and_small_finite_projection_remain_complete() {
    let finite = Domain::new(NonZeroUsize::new(3).unwrap()).join(
        &Value::constant(U256::ZERO),
        &Value::constant(U256::from(2)),
    );
    for value in [Value::top(), finite] {
        let (world, mut entry) = fixture();
        entry.environment.value = value;
        let graph = analyze_world(world, entry, constants_config(usize::MAX, 100_000)).unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        assert!(!graph.states().is_empty());
    }
}

#[test]
fn open_projection_keeps_every_candidate_when_work_fits() {
    for capacity in [512, usize::MAX] {
        let (world, mut entry) = fixture();
        entry.environment.value = open_range(9);
        let graph = analyze_world(world, entry, constants_config(capacity, 1_000_000)).unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        let candidates = graph.states()[0]
            .entry
            .active()
            .call_value
            .constants()
            .unwrap();
        assert_eq!(candidates.len(), 512);
        assert_eq!(candidates.first(), Some(&U256::ZERO));
        assert_eq!(candidates.last(), Some(&U256::from(511)));
        assert!(graph.work() > 1000 && graph.work() <= 1_000_000);
    }
}
