//! Cancellation crosses the actual execution/SSA boundary without poisoning a worker.

use evm_abstract::{
    Address, Fork,
    analysis::{
        self, Config, ControlledAnalysisError, ExecutionConfig,
        control::Control,
        progress::{Event, Observer},
    },
    bytecode::Program,
    ssa,
    world::{Account, Entry, World},
};
use std::{sync::mpsc, thread, time::Duration};

#[test]
fn cancelling_an_executing_block_discards_its_graph_and_releases_the_worker_scope() {
    let address = Address::repeat_byte(0x11);
    let code = format!("{}00", "600160010150".repeat(8000));
    let mut world = World::new(Fork::Osaka, "cancellation worklist regression");
    world
        .insert(address, Account::from_hex(&code, Fork::Osaka).unwrap())
        .unwrap();
    let (events, progress) = mpsc::sync_channel(32);
    let control = Control::new(Observer::new(events));
    let (finished, result) = mpsc::sync_channel(1);
    thread::scope(|scope| {
        let worker_control = control.clone();
        scope.spawn(move || {
            let result = analysis::analyze_world_with_control(
                world,
                Entry::new(address),
                ExecutionConfig::default(),
                &worker_control,
            );
            let cancelled = matches!(result, Err(ControlledAnalysisError::Cancelled));
            drop(result);
            // Reuse the same physical worker after leaving its cancelled scope.
            let next =
                analysis::analyze(Program::from_hex("00").unwrap(), Config::default()).unwrap();
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
    let graph = analysis::analyze(
        Program::from_hex("600160020100").unwrap(),
        Config::default(),
    )
    .unwrap();
    let control = Control::new(Observer::default());
    control.cancellation().cancel();
    control.scope(|| {
        assert!(matches!(
            ssa::build_world(graph.execution()),
            Err(ssa::SsaError::Cancelled)
        ));
        assert!(matches!(
            ssa::build_partial_world(graph.execution()),
            Err(ssa::SsaError::Cancelled)
        ));
    });
    ssa::build_world(graph.execution()).unwrap();
}
