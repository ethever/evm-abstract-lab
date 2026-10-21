//! Release-mode comparison of actual transaction Store operations.

use alloy_primitives::{Address, U256, keccak256};
use evm_abstract::{
    Fork,
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, ProductAlgebra, ValueAlgebra,
        relational::RelationLimits,
    },
    world::{EvmAccount, EvmStore, EvmWorld, StoreError, WorldError},
};
use serde::Serialize;
use std::{collections::BTreeSet, hint::black_box, num::ParseIntError, time::Instant};
use thiserror::Error;

const MAX_SPARSE_WRITES: usize = 8;
const REPLACEMENT: u64 = 1_000_000;
const OBSERVATION_SCOPE: &str = "all-listed-slots-plus-unlisted-default-read";

#[derive(Debug, Error)]
enum ExampleError {
    #[error("{0}")]
    Argument(#[from] ParseIntError),
    #[error("{name} must be between 1 and {maximum}")]
    ArgumentRange { name: String, maximum: usize },
    #[error("usage: state-backends [slots: 1..65536] [iterations: 1..10000]")]
    Usage,
    #[error("{workload} changed its input Store")]
    StoreChanged { workload: &'static str },
    #[error("{workload} did not change the tested storage observations")]
    UnchangedEffect { workload: &'static str },
    #[error(
        "{workload} slot {slot:#x}: expected {expected:?}, actual finite candidates {actual:?}, expected candidates admitted={admitted}"
    )]
    StorageObservation {
        workload: &'static str,
        slot: U256,
        expected: BTreeSet<U256>,
        actual: Option<BTreeSet<U256>>,
        admitted: bool,
    },
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    World(#[from] WorldError),
    #[error("transaction Store operation failed: {0}")]
    Store(#[from] StoreError),
}

#[derive(Serialize)]
struct Sample {
    backend: &'static str,
    workload: &'static str,
    slots: usize,
    iterations: usize,
    writes_per_iteration: usize,
    relation_limits: RelationLimits,
    elapsed_ns: u128,
    final_slot_count: usize,
    final_work_size: usize,
    storage_observation_scope: &'static str,
    storage_observation_count: usize,
    unlisted_slot: U256,
    baseline_storage_observation_checksum: String,
    effect_storage_observation_checksum: String,
    restored_storage_observation_checksum: String,
    restore_verified: bool,
}

struct Measurement {
    address: Address,
    slots: usize,
    iterations: usize,
    limits: RelationLimits,
    baseline_checksum: String,
}

#[derive(Clone, Copy)]
enum StorageEffect {
    Baseline,
    SparseWrite,
    UnknownAlias,
    Joined,
}

impl StorageEffect {
    fn expected(self, index: usize, slots: usize) -> BTreeSet<U256> {
        let original = U256::from(if index < slots { index + 1 } else { 0 });
        let replacement = U256::from(REPLACEMENT);
        match self {
            Self::SparseWrite if index < slots.min(MAX_SPARSE_WRITES) => [replacement].into(),
            Self::UnknownAlias => [original, replacement].into(),
            Self::Joined if index < slots.min(MAX_SPARSE_WRITES) => [original, replacement].into(),
            _ => [original].into(),
        }
    }
}

fn bounded_argument(
    argument: Option<String>,
    name: &str,
    default: usize,
    maximum: usize,
) -> Result<usize, ExampleError> {
    let value = argument.map_or(Ok(default), |value| value.parse::<usize>())?;
    if !(1..=maximum).contains(&value) {
        return Err(ExampleError::ArgumentRange {
            name: name.to_owned(),
            maximum,
        });
    }
    Ok(value)
}

fn relation_limits(slots: usize) -> RelationLimits {
    let defaults = RelationLimits::default();
    let writes = slots.min(MAX_SPARSE_WRITES);
    // SlotState::zero has one node. Each literal-key/literal-value Store adds
    // three nodes and one depth level. A round adds at most eight writes.
    // Joining baseline with that round adds one ITE and a one-node selector;
    // a checked SlotRead reserves its node plus the literal key before folding.
    // Thus the largest read needs 6*S + 3*W + 6 nodes and S + W + 3 depth.
    // Every iteration restores its checkpoint; iterations never multiply this
    // bound. The only join guard fits the unchanged default constraint limit.
    // Argument validation bounds S to 65536, so these finite sums cannot overflow.
    RelationLimits {
        max_nodes: defaults.max_nodes.max(6 * slots + 3 * writes + 6),
        max_depth: defaults.max_depth.max(slots + writes + 3),
        ..defaults
    }
}

fn storage_observation_checksum<D: EvmNumericAlgebra>(
    store: &EvmStore<D::Element>,
    address: Address,
    slots: usize,
    algebra: &ValueAlgebra<D>,
    effect: StorageEffect,
    workload: &'static str,
) -> Result<String, ExampleError> {
    let mut cells = Vec::with_capacity(slots + 1);
    // Slot S is deterministically outside the initialized 0..S range. It reads
    // the default region that an unknown alias write must also affect.
    for index in 0..=slots {
        let key = U256::from(index);
        let value = store.read_checked(address, &EvmValue::constant(key), algebra.borrowed())?;
        let expected = effect.expected(index, slots);
        let candidates = value.constants();
        let admitted = expected.iter().all(|word| value.contains(*word));
        if candidates != Some(&expected) || !admitted {
            return Err(ExampleError::StorageObservation {
                workload,
                slot: key,
                expected,
                actual: candidates.cloned(),
                admitted,
            });
        }
        // Preserve the actual complete numeric carrier, including every product
        // component. Unsupported/non-finite candidate views fail above. This
        // executable selects ProductAlgebra; no arbitrary-N exactness oracle or
        // source-identity normalization is assumed by this generic collector.
        cells.push((key, value.numeric().clone()));
    }
    Ok(format!(
        "{:#x}",
        keccak256(serde_json::to_vec(&(address, cells))?)
    ))
}

fn sample<D: EvmNumericAlgebra>(
    workload: &'static str,
    baseline: &EvmStore<D::Element>,
    algebra: &ValueAlgebra<D>,
    measurement: &Measurement,
    writes_per_iteration: usize,
    effect_checksum: &str,
    mut operation: impl FnMut(&mut EvmStore<D::Element>) -> Result<(), StoreError>,
) -> Result<Sample, ExampleError> {
    let mut store = baseline.clone();
    let start = Instant::now();
    for _ in 0..measurement.iterations {
        operation(black_box(&mut store))?;
    }
    let elapsed_ns = start.elapsed().as_nanos();
    if store != *baseline {
        return Err(ExampleError::StoreChanged { workload });
    }
    let restored_checksum = storage_observation_checksum(
        &store,
        measurement.address,
        measurement.slots,
        algebra,
        StorageEffect::Baseline,
        workload,
    )?;
    Ok(Sample {
        backend: snapshot_state::BACKEND,
        workload,
        slots: measurement.slots,
        iterations: measurement.iterations,
        writes_per_iteration,
        relation_limits: measurement.limits,
        elapsed_ns,
        final_slot_count: store.slots().len(),
        final_work_size: store.work_size(),
        storage_observation_scope: OBSERVATION_SCOPE,
        storage_observation_count: measurement.slots + 1,
        unlisted_slot: U256::from(measurement.slots),
        baseline_storage_observation_checksum: measurement.baseline_checksum.clone(),
        effect_storage_observation_checksum: effect_checksum.to_owned(),
        restored_storage_observation_checksum: restored_checksum,
        restore_verified: true,
    })
}

fn run() -> Result<(), ExampleError> {
    let mut arguments = std::env::args().skip(1);
    let slots = bounded_argument(arguments.next(), "slots", 4096, 65_536)?;
    let iterations = bounded_argument(arguments.next(), "iterations", 100, 10_000)?;
    if arguments.next().is_some() {
        return Err(ExampleError::Usage);
    }
    let spec = DomainSpec::default().with_relations(relation_limits(slots));
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(spec));
    measure(slots, iterations, &algebra)
}

fn measure<D: EvmNumericAlgebra>(
    slots: usize,
    iterations: usize,
    algebra: &ValueAlgebra<D>,
) -> Result<(), ExampleError> {
    let address = Address::from([1_u8; 20]);
    let domain = algebra.borrowed();
    let mut account = EvmAccount::<D::Element>::empty();
    account.storage = (0..slots)
        .map(|slot| (U256::from(slot), EvmValue::constant(U256::from(slot + 1))))
        .collect();
    let mut world = EvmWorld::new(Fork::Osaka, "state-backend-comparison");
    world.insert(address, account)?;
    let baseline = EvmStore::new_checked(&world, domain)?;
    let sparse_slots: Vec<_> = (0..slots.min(MAX_SPARSE_WRITES))
        .map(|slot| EvmValue::constant(U256::from(slot)))
        .collect();
    let first_slot = EvmValue::constant(U256::ZERO);
    let unknown_slot = EvmValue::top();
    let replacement = EvmValue::constant(U256::from(REPLACEMENT));
    let mut alternate = baseline.clone();
    for slot in &sparse_slots {
        alternate.write_checked(address, slot, &replacement, domain)?;
    }
    let mut unknown_effect = baseline.clone();
    unknown_effect.write_checked(address, &unknown_slot, &replacement, domain)?;
    let checksum = |store, effect, workload| {
        storage_observation_checksum(store, address, slots, algebra, effect, workload)
    };
    let baseline_checksum = checksum(&baseline, StorageEffect::Baseline, "baseline")?;
    let sparse_checksum = checksum(
        &alternate,
        StorageEffect::SparseWrite,
        "sparse_write_restore",
    )?;
    let unknown_checksum = checksum(
        &unknown_effect,
        StorageEffect::UnknownAlias,
        "unknown_alias_write_restore",
    )?;
    let joined_checksum = checksum(
        &baseline.join(&alternate, domain),
        StorageEffect::Joined,
        "join_drop",
    )?;
    for (workload, effect) in [
        ("sparse_write_restore", &sparse_checksum),
        ("unknown_alias_write_restore", &unknown_checksum),
        ("join_drop", &joined_checksum),
    ] {
        if effect == &baseline_checksum {
            return Err(ExampleError::UnchangedEffect { workload });
        }
    }
    let measurement = Measurement {
        address,
        slots,
        iterations,
        limits: algebra.spec().relations(),
        baseline_checksum,
    };
    let samples = [
        sample(
            "checkpoint_drop",
            &baseline,
            algebra,
            &measurement,
            0,
            &measurement.baseline_checksum,
            |store| {
                let checkpoint = black_box(store.snapshot());
                drop(checkpoint);
                Ok(())
            },
        )?,
        sample(
            "sparse_write_restore",
            &baseline,
            algebra,
            &measurement,
            sparse_slots.len(),
            &sparse_checksum,
            |store| {
                let checkpoint = store.snapshot();
                let result = (|| {
                    for slot in &sparse_slots {
                        store.write_checked(address, slot, black_box(&replacement), domain)?;
                    }
                    black_box(store.read_checked(address, &first_slot, domain)?);
                    Ok(())
                })();
                store.restore(black_box(checkpoint));
                result
            },
        )?,
        sample(
            "unknown_alias_write_restore",
            &baseline,
            algebra,
            &measurement,
            slots + 1,
            &unknown_checksum,
            |store| {
                let checkpoint = store.snapshot();
                let result = (|| {
                    store.write_checked(address, &unknown_slot, black_box(&replacement), domain)?;
                    black_box(store.read_checked(address, &first_slot, domain)?);
                    Ok(())
                })();
                store.restore(black_box(checkpoint));
                result
            },
        )?,
        sample(
            "join_drop",
            &baseline,
            algebra,
            &measurement,
            0,
            &joined_checksum,
            |store| {
                let joined = black_box(store.join(black_box(&alternate), domain));
                black_box(joined.read_checked(address, &first_slot, domain)?);
                drop(joined);
                Ok(())
            },
        )?,
    ];
    for sample in samples {
        println!("{}", serde_json::to_string(&sample)?);
    }
    Ok(())
}

fn main() -> Result<(), ExampleError> {
    run()
}
