//! 初始化的完整候选投影必须先经过同一工作预算。
use evm_abstract::analysis::AnalysisInput;
use evm_abstract::analysis::control::Control;

#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Address, Fork, U256,
    analysis::{ExecutionConfig, FrontierReason, Status},
    domain::{
        DomainSpec, EvmValue,
        facts::{UnaryPredicate, WordBounds},
    },
    render,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use std::num::NonZeroUsize;

fn open_range(unknown_bits: usize) -> EvmValue<NumericValue> {
    open_interval(U256::ZERO, (U256::from(1) << unknown_bits) - U256::from(1))
}

fn open_interval(lower: U256, upper: U256) -> EvmValue<NumericValue> {
    let domain = native::product_with(DomainSpec::new(
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

fn fixture() -> (EvmWorld<NumericValue>, EvmEntry<NumericValue>) {
    let address = Address::repeat_byte(0x11);
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "initial projection budget");
    world
        .insert(
            address,
            EvmAccount::<NumericValue>::from_hex("00", world.fork()).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry {
        address,
        environment: evm_abstract::world::EvmEnvironmentValues {
            to: (address).into(),
            caller: (Address::ZERO).into(),
            value: EvmValue::<NumericValue>::constant(U256::ZERO),
            calldata: EvmByteArray::<NumericValue>::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironmentValues::<NumericValue>::default()
        },
    };
    (world, entry)
}

fn execution_policy(max_work: usize) -> ExecutionConfig {
    ExecutionConfig {
        max_work,
        ..ExecutionConfig::default()
    }
}

fn assert_initial_work_frontier(graph: &evm_abstract::analysis::EvmWorldAnalysis<NumericValue>) {
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
                .insert(U256::ZERO, EvmValue::<NumericValue>::constant(U256::ZERO));
            world = EvmWorld::<NumericValue>::new(Fork::Osaka, "storage projection baseline");
            world.insert(entry.address, account).unwrap();
        }
        let baseline = native::run(
            &native::product_with(native::spec(512, 4, 256)),
            AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
            execution_policy(100_000),
            &Control::default(),
        )
        .unwrap();
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
            world = EvmWorld::<NumericValue>::new(Fork::Osaka, "open storage projection budget");
            world.insert(entry.address, account).unwrap();
        } else {
            entry.environment.value = value;
        }
        let graph = native::run(
            &native::product_with(native::spec(512, 4, 256)),
            AnalysisInput::new(world, entry).unwrap(),
            execution_policy(100_000),
            &Control::default(),
        )
        .unwrap();
        assert_initial_work_frontier(&graph);
    }
}

#[test]
fn initial_projection_precharge_covers_unknown_calldata_bytes() {
    let (world, mut entry) = fixture();
    let baseline = native::run(
        &native::product_with(native::spec(256, 4, 256)),
        AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
        execution_policy(100_000),
        &Control::default(),
    )
    .unwrap();
    assert_eq!(
        baseline.status(),
        Status::Converged,
        "{:?}",
        baseline.frontiers()
    );
    entry.environment.calldata = EvmByteArray::<NumericValue>::unknown();
    let graph = native::run(
        &native::product_with(native::spec(256, 4, 256)),
        AnalysisInput::new(world, entry).unwrap(),
        execution_policy(100_000),
        &Control::default(),
    )
    .unwrap();
    assert_initial_work_frontier(&graph);
}

#[test]
fn initial_projection_precharge_covers_interval_enumeration() {
    let (world, mut entry) = fixture();
    let baseline = native::run(
        &native::product_with(native::spec(512, 4, 256)),
        AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
        execution_policy(100_000),
        &Control::default(),
    )
    .unwrap();
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
    let graph = native::run(
        &native::product_with(native::spec(512, 4, 256)),
        AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
        execution_policy(100_000),
        &Control::default(),
    )
    .unwrap();
    assert_initial_work_frontier(&graph);
    let graph = native::run(
        &native::product_with(native::spec(512, 4, 256)),
        AnalysisInput::new(world, entry).unwrap(),
        execution_policy(1_000_000),
        &Control::default(),
    )
    .unwrap();
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
                world =
                    EvmWorld::<NumericValue>::new(Fork::Osaka, "maximum storage projection budget");
                world.insert(entry.address, account).unwrap();
            } else {
                entry.environment.value = value;
            }
            let graph = native::run(
                &native::product_with(native::spec(usize::MAX, 4, 256)),
                AnalysisInput::new(world, entry).unwrap(),
                execution_policy(max_work),
                &Control::default(),
            )
            .unwrap();
            assert_initial_work_frontier(&graph);
            assert!(
                render::world::explain(
                    &graph,
                    &native::product_with(graph.domain_spec()),
                    &mut native::postprocessing()
                )
                .unwrap()
                .contains("SSA unavailable")
            );
        }
    }
}

#[test]
fn maximum_capacity_top_and_small_finite_projection_remain_complete() {
    let finite = native::product_with(native::spec(3, 4, 256)).join(
        &EvmValue::<NumericValue>::constant(U256::ZERO),
        &EvmValue::<NumericValue>::constant(U256::from(2)),
    );
    for value in [EvmValue::<NumericValue>::top(), finite] {
        let (world, mut entry) = fixture();
        entry.environment.value = value;
        let graph = native::run(
            &native::product_with(native::spec(usize::MAX, 4, 256)),
            AnalysisInput::new(world, entry).unwrap(),
            execution_policy(100_000),
            &Control::default(),
        )
        .unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        assert!(!graph.states().is_empty());
    }
}

#[test]
fn open_projection_keeps_every_candidate_when_work_fits() {
    for capacity in [512, usize::MAX] {
        let (world, mut entry) = fixture();
        entry.environment.value = open_range(9);
        let graph = native::run(
            &native::product_with(native::spec(capacity, 4, 256)),
            AnalysisInput::new(world, entry).unwrap(),
            execution_policy(1_000_000),
            &Control::default(),
        )
        .unwrap();
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
