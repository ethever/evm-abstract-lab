//! Release-mode comparison of actual transaction Store operations.

use alloy_primitives::{Address, U256, keccak256};
use evm_abstract::{
    Fork,
    domain::{Domain, Value},
    world::{Account, Store, World, WorldError},
};
use serde::Serialize;
use std::{hint::black_box, num::ParseIntError, time::Instant};
use thiserror::Error;

#[derive(Debug, Error)]
enum ExampleError {
    #[error("{0}")]
    Argument(ParseIntError),
    #[error("{name} must be between 1 and {maximum}")]
    ArgumentRange { name: String, maximum: usize },
    #[error("usage: state-backends [slots: 1..65536] [iterations: 1..10000]")]
    Usage,
    #[error("{workload} changed its input Store")]
    StoreChanged { workload: &'static str },
    #[error("{0}")]
    Json(serde_json::Error),
    #[error("{0}")]
    World(WorldError),
}

impl From<ParseIntError> for ExampleError {
    fn from(error: ParseIntError) -> Self {
        Self::Argument(error)
    }
}

impl From<serde_json::Error> for ExampleError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<WorldError> for ExampleError {
    fn from(error: WorldError) -> Self {
        Self::World(error)
    }
}

#[derive(Serialize)]
struct Sample {
    backend: &'static str,
    workload: &'static str,
    slots: usize,
    iterations: usize,
    writes_per_iteration: usize,
    elapsed_ns: u128,
    observation_checksum: u64,
    final_slot_count: usize,
    final_work_size: usize,
    final_store_checksum: String,
    effect_store_checksum: String,
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

fn observation(value: Value) -> u64 {
    match value.constants() {
        Some(values) => values.iter().fold(values.len() as u64, |checksum, value| {
            checksum.wrapping_add(value.to::<u64>())
        }),
        None => u64::MAX,
    }
}

fn store_checksum(store: &Store) -> Result<String, ExampleError> {
    Ok(format!("{:#x}", keccak256(serde_json::to_vec(store)?)))
}

fn sample(
    workload: &'static str,
    baseline: &Store,
    slots: usize,
    iterations: usize,
    writes_per_iteration: usize,
    effect_store_checksum: &str,
    mut operation: impl FnMut(&mut Store) -> u64,
) -> Result<Sample, ExampleError> {
    // Initial construction and this per-workload clone are outside the timer.
    let mut store = baseline.clone();
    let mut checksum = 0_u64;
    let start = Instant::now();
    for _ in 0..iterations {
        checksum = checksum.wrapping_add(black_box(operation(black_box(&mut store))));
    }
    let elapsed_ns = start.elapsed().as_nanos();
    if store != *baseline {
        return Err(ExampleError::StoreChanged { workload });
    }
    let final_store_checksum = store_checksum(&store)?;
    Ok(Sample {
        backend: snapshot_state::BACKEND,
        workload,
        slots,
        iterations,
        writes_per_iteration,
        elapsed_ns,
        observation_checksum: checksum,
        final_slot_count: store.slots().len(),
        final_work_size: store.work_size(),
        final_store_checksum,
        effect_store_checksum: effect_store_checksum.to_owned(),
    })
}

fn run() -> Result<(), ExampleError> {
    let mut arguments = std::env::args().skip(1);
    let slots = bounded_argument(arguments.next(), "slots", 4096, 65_536)?;
    let iterations = bounded_argument(arguments.next(), "iterations", 100, 10_000)?;
    if arguments.next().is_some() {
        return Err(ExampleError::Usage);
    }

    let address = Address::from([1_u8; 20]);
    let domain = Domain::default();
    let mut account = Account::empty();
    account.storage = (0..slots)
        .map(|slot| {
            (
                U256::from(slot),
                Value::constant(U256::from(slot.saturating_add(1))),
            )
        })
        .collect();
    let mut world = World::new(Fork::Osaka, "state-backend-comparison");
    world.insert(address, account)?;
    let baseline = Store::new(&world);
    let sparse_slots: Vec<_> = (0..slots.min(8))
        .map(|slot| Value::constant(U256::from(slot)))
        .collect();
    let first_slot = Value::constant(U256::ZERO);
    let unknown_slot = Value::top();
    let replacement = Value::constant(U256::from(1_000_000));
    let mut alternate = baseline.clone();
    for slot in &sparse_slots {
        alternate.write(address, slot, &replacement, domain);
    }
    let mut unknown_effect = baseline.clone();
    unknown_effect.write(address, &unknown_slot, &replacement, domain);
    // Preserve whole-map semantic evidence without timing JSON/hash overhead.
    let baseline_checksum = store_checksum(&baseline)?;
    let sparse_checksum = store_checksum(&alternate)?;
    let unknown_checksum = store_checksum(&unknown_effect)?;
    let joined_checksum = store_checksum(&baseline.join(&alternate, domain))?;

    let samples = [
        sample(
            "checkpoint_drop",
            &baseline,
            slots,
            iterations,
            0,
            &baseline_checksum,
            |store| {
                let checkpoint = black_box(store.snapshot());
                drop(checkpoint);
                0
            },
        )?,
        sample(
            "sparse_write_restore",
            &baseline,
            slots,
            iterations,
            sparse_slots.len(),
            &sparse_checksum,
            |store| {
                let checkpoint = store.snapshot();
                for slot in &sparse_slots {
                    store.write(address, slot, black_box(&replacement), domain);
                }
                let observed = observation(black_box(store.read(address, &first_slot, domain)));
                store.restore(black_box(checkpoint));
                observed
            },
        )?,
        sample(
            "unknown_alias_write_restore",
            &baseline,
            slots,
            iterations,
            slots + 1,
            &unknown_checksum,
            |store| {
                let checkpoint = store.snapshot();
                store.write(address, &unknown_slot, black_box(&replacement), domain);
                let observed = observation(black_box(store.read(address, &first_slot, domain)));
                store.restore(black_box(checkpoint));
                observed
            },
        )?,
        sample(
            "join_drop",
            &baseline,
            slots,
            iterations,
            0,
            &joined_checksum,
            |store| {
                let joined = black_box(store.join(black_box(&alternate), domain));
                let observed = observation(black_box(joined.read(address, &first_slot, domain)));
                drop(joined);
                observed
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
