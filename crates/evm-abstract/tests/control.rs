//! Cancellation crosses the actual execution/SSA boundary without poisoning a worker.
use evm_abstract::analysis::{AnalysisInput, EvmAnalysis};

#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Address, Fork,
    analysis::{
        self, ExecutionConfig,
        control::Control,
        progress::{Event, Observer},
    },
    bytecode::Program,
    ssa,
    world::{EvmAccount, EvmEntry, EvmWorld},
};
use std::{sync::mpsc, thread, time::Duration};

#[test]
fn cancelling_an_executing_block_discards_its_graph_and_releases_the_worker_scope() {
    let address = Address::repeat_byte(0x11);
    let code = format!("{}00", "600160010150".repeat(8000));
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "cancellation worklist regression");
    world
        .insert(
            address,
            EvmAccount::<NumericValue>::from_hex(&code, Fork::Osaka).unwrap(),
        )
        .unwrap();
    let (events, progress) = mpsc::sync_channel(32);
    let control = Control::new(Observer::new(events));
    let (finished, result) = mpsc::sync_channel(1);
    thread::scope(|scope| {
        let worker_control = control.clone();
        scope.spawn(move || {
            let result = native::run(
                &native::product(),
                AnalysisInput::new(world, EvmEntry::<NumericValue>::new(address)).unwrap(),
                ExecutionConfig::default(),
                &worker_control,
            );
            let cancelled = matches!(
                result,
                Err(evm_abstract::analysis::cooperation::CooperationError::Cancelled)
            );
            drop(result);
            // Reuse the same physical worker after leaving its cancelled scope.
            let next = native::run(
                &native::product(),
                AnalysisInput::<NumericValue>::from_program(
                    Program::from_hex("00").unwrap(),
                    Default::default(),
                )
                .unwrap(),
                ExecutionConfig::default(),
                &Control::default(),
            )
            .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
            .unwrap();
            finished.send((cancelled, next.status())).unwrap();
        });
        loop {
            if matches!(
                progress.recv_timeout(Duration::from_secs(5)).unwrap(),
                Event::Execution { .. }
            ) {
                break;
            }
        }
        control.cancellation().cancel();
        let (cancelled, next) = result.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            cancelled,
            "cancelled execution must not publish a partial graph"
        );
        assert_eq!(next, analysis::Status::Converged);
    });
}

#[test]
fn cancellation_prevents_starting_full_or_partial_ssa() {
    let graph = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(
            Program::from_hex("600160020100").unwrap(),
            Default::default(),
        )
        .unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap();
    let control = Control::new(Observer::default());
    control.cancellation().cancel();
    control.scope(|| {
        assert!(matches!(
            ssa::build_world(graph.execution()),
            Err(ssa::SsaError::Cancelled)
        ));
        assert!(matches!(
            ssa::build_partial_world(graph.execution(), &mut native::postprocessing()),
            Err(ssa::SsaError::Cancelled)
        ));
    });
    ssa::build_world(graph.execution()).unwrap();
}
