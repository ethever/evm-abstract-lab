use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Bound {
    meaning: u8,
    representation: u8,
}
struct Sidecar {
    s: Scenario,
    slot: SlotMeta<Bound>,
    bound: Bound,
}
impl Sidecar {
    fn new() -> Self {
        let mut f = Fixture::new();
        let anchor = f.kernel.slot(f.source, &mut f.meter).unwrap();
        let slot = f.kernel.slot(f.source, &mut f.meter).unwrap();
        let shared = f.kernel.channel(Ascending(Union), &mut f.meter).unwrap();
        let mut bound = Bound {
            meaning: 1,
            representation: 0,
        };
        let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
        run.observe_slot(&anchor).unwrap();
        assert_eq!(
            run.publish_prepared(
                &slot,
                &mut bound,
                Bound {
                    meaning: 1,
                    representation: 99
                },
                Change::Unchanged,
                1
            )
            .unwrap(),
            Change::Changed
        );
        run.finish(LocalClosure::Stable).unwrap();
        let mut this = Self {
            s: Scenario {
                f,
                anchor,
                native: 1,
                shared,
            },
            slot,
            bound,
        };
        assert_eq!(this.read().value().unwrap().representation, 0);
        this
    }
    fn read(&mut self) -> Answer<Bound> {
        let f = &mut self.s.f;
        let mut run = f.kernel.begin(f.downstream_reader, &mut f.meter).unwrap();
        let answer = run.read_slot(&self.slot, &self.bound, 2).unwrap();
        if answer.value().is_some() {
            assert_eq!(run.observe_slot(&self.slot).unwrap(), answer.version);
        } else {
            assert_eq!(
                run.observe_slot(&self.slot),
                Err(KernelError::PendingCoverage)
            );
        }
        run.finish(LocalClosure::Stable).unwrap();
        answer
    }
    fn pending(&mut self) {
        self.s.grow();
        self.s.replace(false);
        self.s.replace(true);
        self.s.drain_owners();
        assert_eq!(self.read().state, Availability::Pending);
        assert!(
            matches!(self.s.f.kernel.closure(), Closure::Open(o) if o.missing == 1 && o.obligations == 0)
        );
    }
}

#[test]
fn all_prepared_sidecar_paths_restore_equivalent_coverage_without_identity_churn() {
    for flavor in 0..3 {
        let mut x = Sidecar::new();
        let before = x.read();
        x.pending();
        let f = &mut x.s.f;
        let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
        run.observe_slot(&x.s.anchor).unwrap();
        let mut evidence = 0;
        let changed = match flavor {
            0 => run.publish_prepared(
                &x.slot,
                &mut x.bound,
                Bound {
                    meaning: 1,
                    representation: 99,
                },
                Change::Unchanged,
                1,
            ),
            1 => run.publish_prepared_with(&x.slot, Change::Unchanged, 1, || evidence = 1),
            _ => run.publish_prepared_parts(
                &x.slot,
                &mut x.bound,
                Bound {
                    meaning: 1,
                    representation: 0,
                },
                &mut evidence,
                1,
                Change::Unchanged,
                2,
            ),
        }
        .unwrap();
        assert_eq!(changed, Change::Changed);
        run.finish(LocalClosure::Stable).unwrap();
        assert!(!x.s.f.stale(0) && !x.s.f.stale(1));
        assert_eq!(
            x.bound.representation, 0,
            "the value-only Unchanged path preserves its promised representation"
        );
        let restored = x.read();
        assert_eq!(restored.value(), before.value());
        assert!(restored.version > before.version);
        assert_eq!(x.s.f.kernel.closure(), Closure::Closed);
        let f = &mut x.s.f;
        let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
        assert_eq!(
            run.publish_prepared(
                &x.slot,
                &mut x.bound,
                Bound {
                    meaning: 1,
                    representation: 42
                },
                Change::Unchanged,
                1
            )
            .unwrap(),
            Change::Unchanged
        );
        run.finish(LocalClosure::Stable).unwrap();
        assert_eq!(x.read().version, restored.version);
    }
}

#[test]
fn sidecar_refusal_never_installs_evidence_or_coverage_or_wakes_waiting_consumers() {
    let reservations = {
        let mut x = Sidecar::new();
        x.pending();
        let f = &mut x.s.f;
        let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
        let before = run.meter().calls;
        run.publish_prepared_with(&x.slot, Change::Unchanged, 1, || {})
            .unwrap();
        run.meter().calls - before
    };
    for fail_at in 1..=reservations {
        let mut x = Sidecar::new();
        x.pending();
        let before = x.read();
        let f = &mut x.s.f;
        let queue = f.kernel.worklist().clone();
        let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
        run.meter().fail_on = Some(run.meter().calls + fail_at);
        let mut evidence = 0;
        assert_eq!(
            run.publish_prepared_with(&x.slot, Change::Unchanged, 1, || evidence = 1),
            Err(KernelError::Stopped(Stop::Work))
        );
        assert_eq!(evidence, 0);
        drop(run);
        f.meter.fail_on = None;
        assert_eq!(f.kernel.worklist(), &queue);
        assert_eq!(x.read(), before);
        assert!(!x.s.f.stale(1));
    }
}

#[test]
fn coverage_maps_fork_and_commit_with_detached_sidecar_state() {
    for commit in [false, true] {
        let mut x = Sidecar::new();
        x.pending();
        let f = &mut x.s.f;
        let mut fork = f.kernel.fork(&mut f.meter).unwrap();
        let mut detached = x.bound.clone();
        let mut run = fork.kernel().begin(f.reader, &mut f.meter).unwrap();
        run.publish_prepared(
            &x.slot,
            &mut detached,
            x.bound.clone(),
            Change::Unchanged,
            1,
        )
        .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        let mut run = fork
            .kernel()
            .begin(f.downstream_reader, &mut f.meter)
            .unwrap();
        assert!(
            run.read_slot(&x.slot, &detached, 2)
                .unwrap()
                .value()
                .is_some()
        );
        run.finish(LocalClosure::Stable).unwrap();
        assert_eq!(fork.view().closure(), Closure::Closed);
        assert!(matches!(f.kernel.closure(), Closure::Open(o) if o.missing == 1));
        if commit {
            f.kernel
                .commit_with(fork, &mut f.meter, 1, || x.bound = detached)
                .unwrap();
        } else {
            drop(fork);
        }
        assert_eq!(x.read().value().is_some(), commit);
    }
}

#[test]
fn unchanged_raw_publication_counter_cannot_certify_invalidated_producer_coverage() {
    let mut x = Sidecar::new();
    let before = x.read();
    x.s.grow();
    let f = &mut x.s.f;
    let mut run = f.kernel.begin(f.owner, &mut f.meter).unwrap();
    assert_eq!(run.slot_version(&x.slot).unwrap(), before.version);
    let now = run.read_slot(&x.slot, &x.bound, 2).unwrap();
    assert_eq!(now.version, before.version);
    assert_eq!(now.state, Availability::Pending);
    assert_eq!(run.observe_slot(&x.slot), Err(KernelError::PendingCoverage));
    run.finish(LocalClosure::Stable).unwrap();
}
