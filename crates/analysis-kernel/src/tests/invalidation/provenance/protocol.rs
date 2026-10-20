use super::*;

#[test]
fn declared_producer_invalidation_and_replacement_withhold_x_until_equivalent_republication() {
    for producer_first in [false, true] {
        let mut s = Scenario::new();
        let before = s.read_b();
        s.grow();
        assert!(s.f.kernel.worklist().contains(&s.f.owner));
        assert!(s.f.kernel.worklist().contains(&s.f.downstream_owner));
        if producer_first {
            s.replace(true);
        }
        s.replace(false);
        let pending = s.read_b();
        assert_eq!(pending.state, Availability::Pending);
        assert_eq!(pending.version, before.version, "X itself did not grow");
        assert!(
            !s.f.stale(1),
            "Pending coverage is not an adopted old guarantee"
        );
        if !producer_first {
            s.replace(true);
        }
        s.drain_owners();
        assert_eq!(
            s.read_b().state,
            Availability::Pending,
            "retiring A0 must retain the stable family guard"
        );
        assert!(!s.f.stale(0) && !s.f.stale(1));
        assert!(s.f.kernel.worklist().is_empty());
        // Exact counterexample: no stale scopes or queued/running jobs remain,
        // but A1 finished without republishing the old numerically ready X.
        assert!(
            matches!(s.f.kernel.closure(), Closure::Open(o) if o.missing == 1 && o.obligations == 0)
        );
        let report = solve(&mut s.f.kernel, &mut NoOp, &mut Nil, &mut s.f.meter, 0).unwrap();
        assert!(matches!(report.closure, Closure::Open(o) if o.missing == 1));
        assert_eq!(
            s.renew(),
            Change::Changed,
            "equivalent value restores visible coverage"
        );
        assert!(!s.f.stale(0) && !s.f.stale(1));
        assert!(s.f.kernel.worklist().contains(&s.f.downstream_reader));
        let restored = s.read_b();
        assert_eq!(restored.value(), Some(&1));
        assert!(restored.version > before.version);
        assert_eq!(s.f.kernel.closure(), Closure::Closed);
        assert_eq!(s.renew(), Change::Unchanged);
        assert_eq!(s.read_b().version, restored.version);
        assert_eq!(s.f.kernel.closure(), Closure::Closed);
    }
}

#[test]
fn multiple_declared_families_require_every_current_producer_and_new_family_invalidates_once() {
    let mut s = Scenario::new();
    let c =
        s.f.kernel
            .register(s.f.unrelated, (), &mut s.f.meter)
            .unwrap();
    let anchor_c =
        s.f.kernel
            .slot::<u8, _>(s.f.source, &mut s.f.meter)
            .unwrap();
    let mut input_c = 1;
    let mut run = s.f.kernel.begin(c, &mut s.f.meter).unwrap();
    run.observe_slot(&anchor_c).unwrap();
    assert_eq!(
        run.publish(&mut s.shared, Scoped::new(s.f.source, 0), 1)
            .unwrap(),
        Change::Changed
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(
        s.f.stale(1),
        "B used X before its newly declared C premise existed"
    );
    assert!(!s.f.stale(0));
    s.replace(false);
    assert_eq!(s.read_b().value(), Some(&1));
    s.drain_owners();
    assert_eq!(s.f.kernel.closure(), Closure::Closed);
    let mut run = s.f.kernel.begin(s.f.writer, &mut s.f.meter).unwrap();
    run.publish_prepared(&anchor_c, &mut input_c, 3, Change::Changed, 1)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!s.f.stale(0), "A's native input stayed valid");
    assert!(s.f.stale(1) && s.f.stale(2));
    s.replace(false);
    assert_eq!(
        s.read_b().state,
        Availability::Pending,
        "A still covers X, but C's unmet declared contribution blocks the aggregate"
    );
}

#[test]
fn explicit_covering_absence_can_renew_a_family_without_erasing_retained_aggregate_value() {
    let mut s = Scenario::new();
    s.grow();
    s.replace(false);
    s.replace(true);
    s.drain_owners();
    assert_eq!(s.read_b().state, Availability::Pending);
    let mut run = s.f.kernel.begin(s.f.reader, &mut s.f.meter).unwrap();
    run.observe_slot(&s.anchor).unwrap();
    assert_eq!(
        run.resolve_absent(&mut s.shared, Scoped::new(s.f.source, 0))
            .unwrap(),
        Change::Changed
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(
        s.read_b().value(),
        Some(&1),
        "covering absence retains the old safe may bound"
    );
    assert!(!s.f.stale(0) && !s.f.stale(1));
    assert_eq!(s.f.kernel.closure(), Closure::Closed);
}

#[test]
fn family_identity_is_stable_across_tasks_and_retains_one_generation_quote() {
    let mut s = Scenario::new();
    let another = s.f.kernel.register(s.f.target, (), &mut s.f.meter).unwrap();
    let mut run = s.f.kernel.begin(another, &mut s.f.meter).unwrap();
    assert_eq!(
        run.publish(&mut s.shared, Scoped::new(s.f.source, 0), 1)
            .unwrap(),
        Change::Unchanged,
        "a second job in the same source scope is not a new producer family"
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!s.f.stale(1));
    let mut quote = None;
    for generation in 1..=32 {
        s.replace(true);
        assert!(s.f.stale(1));
        s.replace(false);
        s.drain_owners();
        assert_eq!(s.read_b().state, Availability::Pending);
        let before = s.f.meter.used;
        assert_eq!(s.renew(), Change::Changed);
        let work = s.f.meter.used - before;
        assert_eq!(
            *quote.get_or_insert(work),
            work,
            "coverage does not accumulate historical source generations"
        );
        assert_eq!(s.read_b().value(), Some(&1));
        assert!(!s.f.stale(0) && !s.f.stale(1));
        assert_eq!(s.f.target.key().generation, generation);
        assert_eq!(s.f.downstream.key().generation, generation);
        assert_eq!(s.f.kernel.closure(), Closure::Closed);
    }
}

#[test]
fn topology_enumeration_tracks_producer_validity_and_withholds_uncovered_membership() {
    let mut s = Scenario::new();
    let directory_job =
        s.f.kernel
            .register(s.f.unrelated, (), &mut s.f.meter)
            .unwrap();
    owner_read(
        &mut s.f.kernel,
        &mut s.f.meter,
        s.f.owner,
        &s.f.unrelated_entry,
    );
    let mut run = s.f.kernel.begin(directory_job, &mut s.f.meter).unwrap();
    assert_eq!(run.keys(&mut s.shared, s.f.source).unwrap(), [0]);
    run.finish(LocalClosure::Stable).unwrap();
    s.grow();
    assert!(s.f.stale(2));
    let next = scope(
        &mut s.f.kernel,
        &mut s.f.meter,
        s.f.unrelated.key().context,
        1,
    );
    let job = s.f.kernel.register(next, (), &mut s.f.meter).unwrap();
    s.f.kernel.seal_scope(next, &mut s.f.meter).unwrap();
    let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
    run.replace_generation(&mut s.f.unrelated_entry, next)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = s.f.kernel.begin(job, &mut s.f.meter).unwrap();
    assert_eq!(
        run.keys(&mut s.shared, s.f.source),
        Err(KernelError::PendingCoverage)
    );
    run.finish(LocalClosure::Stable).unwrap();
    s.replace(false);
    s.replace(true);
    s.drain_owners();
    assert_eq!(s.renew(), Change::Changed);
    assert!(s.f.kernel.worklist().contains(&job));
    let mut run = s.f.kernel.begin(job, &mut s.f.meter).unwrap();
    assert_eq!(run.keys(&mut s.shared, s.f.source).unwrap(), [0]);
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!s.f.stale(2));
    s.read_b();
    assert_eq!(s.f.kernel.closure(), Closure::Closed);
}

#[test]
fn absent_coverage_is_pending_until_explicitly_renewed_without_adopting_a_negative_proof() {
    let mut s = Scenario::new();
    let mut run = s.f.kernel.begin(s.f.reader, &mut s.f.meter).unwrap();
    run.resolve_absent(&mut s.shared, Scoped::new(s.f.source, 2))
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let observer =
        s.f.kernel
            .register(s.f.unrelated, (), &mut s.f.meter)
            .unwrap();
    let mut run = s.f.kernel.begin(observer, &mut s.f.meter).unwrap();
    assert_eq!(
        run.query(&mut s.shared, Scoped::new(s.f.source, 2))
            .unwrap()
            .state,
        Availability::Absent
    );
    run.finish(LocalClosure::Stable).unwrap();
    s.grow();
    assert!(
        !s.f.stale(2),
        "Absent was a wake subscription, not a negative semantic proof"
    );
    s.replace(true);
    let mut run = s.f.kernel.begin(observer, &mut s.f.meter).unwrap();
    assert_eq!(
        run.query(&mut s.shared, Scoped::new(s.f.source, 2))
            .unwrap()
            .state,
        Availability::Pending
    );
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = s.f.kernel.begin(s.f.reader, &mut s.f.meter).unwrap();
    assert_eq!(
        run.resolve_absent(&mut s.shared, Scoped::new(s.f.source, 2))
            .unwrap(),
        Change::Changed
    );
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = s.f.kernel.begin(observer, &mut s.f.meter).unwrap();
    assert_eq!(
        run.query(&mut s.shared, Scoped::new(s.f.source, 2))
            .unwrap()
            .state,
        Availability::Absent
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!s.f.stale(2));
}

#[test]
fn foreign_fact_stored_in_consumers_own_scope_adopts_growth_and_new_family_premises() {
    for new_family in [false, true] {
        let mut f = Fixture::new();
        let mut channel: Inputs = f.kernel.channel(Ascending(Union), &mut f.meter).unwrap();
        let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
        run.publish(&mut channel, Scoped::new(f.downstream, 0), 1)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        let mut run = f.kernel.begin(f.downstream_reader, &mut f.meter).unwrap();
        assert_eq!(
            run.query(&mut channel, Scoped::new(f.downstream, 0))
                .unwrap()
                .value(),
            Some(&1)
        );
        run.finish(LocalClosure::Stable).unwrap();
        let producer = if new_family {
            f.kernel.register(f.unrelated, (), &mut f.meter).unwrap()
        } else {
            f.reader
        };
        let mut run = f.kernel.begin(producer, &mut f.meter).unwrap();
        run.publish(
            &mut channel,
            Scoped::new(f.downstream, 0),
            if new_family { 1 } else { 3 },
        )
        .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        assert!(
            f.stale(1),
            "foreign fact growth/family addition is a premise even in the reader's storage scope"
        );
        assert!(!f.stale(0));
    }
}
