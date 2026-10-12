use super::{Event, Observer, Phase, PhaseTimings, Timing, TimingSnapshot};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

#[test]
fn repeated_discovery_phases_accumulate_without_resetting_the_session() {
    let start = Instant::now();
    let at = |milliseconds| start + Duration::from_millis(milliseconds);
    let mut timing = Timing::default();
    timing.start(start);
    timing.phase(Phase::Pinning, at(2));
    timing.phase(Phase::Acquiring, at(5));
    timing.phase(Phase::Analyzing, at(9));
    timing.phase(Phase::Acquiring, at(20));
    timing.phase(Phase::Analyzing, at(25));
    timing.phase(Phase::Analyzing, at(30));
    timing.phase(Phase::BuildingSsa, at(40));
    timing.phase(Phase::Projecting, at(47));

    let live = timing.snapshot(at(50));
    assert_eq!(live.started_at, Some(start));
    assert_eq!(live.elapsed_ns, 50_000_000);
    assert_eq!(
        live.phases,
        PhaseTimings {
            validating_ns: 2_000_000,
            pinning_ns: 3_000_000,
            acquiring_ns: 9_000_000,
            analyzing_ns: 26_000_000,
            building_ssa_ns: 7_000_000,
            projecting_ns: 3_000_000,
        }
    );
    // Reading a live snapshot must not commit or double-charge the active phase.
    assert_eq!(timing.snapshot(at(50)), live);
    timing.finish(at(52));
    let frozen = timing.snapshot(at(100));
    assert_eq!(frozen.elapsed_ns, 52_000_000);
    assert_eq!(frozen.phases.projecting_ns, 5_000_000);
}

#[test]
fn timing_is_opt_in_and_finish_is_frozen_until_a_new_session_starts() {
    let start = Instant::now();
    let at = |milliseconds| start + Duration::from_millis(milliseconds);
    let mut timing = Timing::default();
    timing.phase(Phase::Analyzing, at(1));
    timing.finish(at(2));
    assert_eq!(timing.snapshot(at(3)), TimingSnapshot::default());

    assert!(timing.start(at(4)));
    timing.phase(Phase::Analyzing, at(6));
    assert!(!timing.start(at(7)));
    timing.finish(at(9));
    let frozen = timing.snapshot(at(10));
    assert_eq!(frozen.started_at, Some(at(4)));
    assert_eq!(frozen.elapsed_ns, 5_000_000);
    assert_eq!(frozen.phases.validating_ns, 2_000_000);
    assert_eq!(frozen.phases.analyzing_ns, 3_000_000);
    timing.phase(Phase::Projecting, at(20));
    timing.finish(at(30));
    assert_eq!(timing.snapshot(at(40)), frozen);

    assert!(timing.start(at(50)));
    timing.finish(at(54));
    let second = timing.snapshot(at(70));
    assert_eq!(second.started_at, Some(at(50)));
    assert_eq!(second.elapsed_ns, 4_000_000);
    assert_eq!(second.phases.validating_ns, 4_000_000);
    assert_eq!(second.phases.analyzing_ns, 0);
}

#[test]
fn observer_clones_share_authoritative_timing_when_phase_events_are_dropped() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let observer = Observer::new(sender);
    let worker = observer.clone();
    let start = Instant::now() - Duration::from_secs(5);
    // Deterministic setup supplies meaningful elapsed time without sleeping.
    observer.timing_state().start(start);
    observer.phase(Phase::Pinning);
    worker.phase(Phase::Acquiring);
    worker.phase(Phase::Analyzing);
    assert_eq!(
        receiver.try_iter().collect::<Vec<_>>(),
        [Event::Phase(Phase::Pinning)]
    );
    assert_eq!(observer.timing_state().active.unwrap().0, Phase::Analyzing);
    assert!(observer.timing().phases.validating_ns >= 5_000_000_000);

    // Disconnecting the consumer likewise cannot prevent timing or freeze.
    drop(receiver);
    worker.phase(Phase::BuildingSsa);
    assert_eq!(
        observer.timing_state().active.unwrap().0,
        Phase::BuildingSsa
    );
    worker.finish_timing();
    let frozen = observer.timing();
    observer.finish_timing();
    assert_eq!(worker.timing(), frozen);
    assert_eq!(observer.timing(), frozen);
}

#[test]
fn default_observer_does_not_implicitly_start_timing() {
    let observer = Observer::default();
    observer.phase(Phase::Analyzing);
    observer.finish_timing();
    assert_eq!(observer.timing(), TimingSnapshot::default());
    assert!(observer.start_timing());
    let start = observer.timing().started_at;
    assert!(!observer.clone().start_timing());
    assert_eq!(observer.timing().started_at, start);
    observer.finish_timing();
    assert!(observer.clone().start_timing());
    assert!(!observer.start_timing());
    observer.finish_timing();
}

#[test]
fn concurrent_observer_clones_grant_session_ownership_to_exactly_one_caller() {
    let observer = Observer::default();
    let ready = std::sync::Barrier::new(3);
    let owners = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let first = observer.clone();
        let second = observer.clone();
        let ready = &ready;
        let owners = &owners;
        scope.spawn(move || {
            ready.wait();
            if first.start_timing() {
                owners.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        scope.spawn(move || {
            ready.wait();
            if second.start_timing() {
                owners.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        ready.wait();
    });
    assert_eq!(owners.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(observer.timing().started_at.is_some());
    assert!(!observer.start_timing());
    observer.finish_timing();
    assert!(observer.start_timing());
    observer.finish_timing();
}
