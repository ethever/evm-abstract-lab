use super::*;
mod atomicity;

fn withdraw(s: &mut Scenario) -> Answer<Scope> {
    let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
    assert_eq!(
        run.withdraw_generation(&mut s.f.entry).unwrap(),
        Change::Changed
    );
    let absent = run.current_generation(&s.f.entry).unwrap();
    assert_eq!(absent.state, Availability::Absent);
    run.finish(LocalClosure::Stable).unwrap();
    absent
}
fn empty() -> Scenario {
    let mut s = Scenario::new();
    s.grow();
    withdraw(&mut s);
    s.replace(false);
    s.drain_owners();
    assert_eq!(
        s.read_b().value(),
        Some(&1),
        "the old aggregate can remain a coarse may upper bound"
    );
    assert_eq!(s.f.kernel.closure(), Closure::Closed);
    s
}
fn next_source(s: &mut Scenario, top: u8) -> (Scope, JobId) {
    let old = s.f.target;
    let next = scope(
        &mut s.f.kernel,
        &mut s.f.meter,
        old.key().context,
        old.key().generation + 1,
    );
    let job = s.f.kernel.register(next, (), &mut s.f.meter).unwrap();
    seed(&mut s.f.kernel, &mut s.f.meter, job, &mut s.f.local, top);
    s.f.kernel.seal_scope(next, &mut s.f.meter).unwrap();
    (next, job)
}

#[test]
fn confirmed_empty_family_closes_and_reactivation_invalidates_its_empty_premise() {
    let mut s = empty();
    assert_eq!(
        s.f.kernel
            .closure_in_metered(s.f.downstream, &mut s.f.meter)
            .unwrap(),
        Closure::Closed
    );
    let identity = s.f.entry.clone();
    let old_scope = s.f.target;
    let before = {
        let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
        let answer = run.current_generation(&s.f.entry).unwrap();
        assert_eq!(
            run.withdraw_generation(&mut s.f.entry).unwrap(),
            Change::Unchanged
        );
        assert_eq!(run.current_generation(&s.f.entry).unwrap(), answer);
        run.finish(LocalClosure::Stable).unwrap();
        answer
    };
    assert_eq!(before.state, Availability::Absent);
    assert!(
        !s.f.kernel
            .generation_stale(&identity, &mut s.f.meter)
            .unwrap()
    );
    assert!(s.f.kernel.scope_stale(old_scope, &mut s.f.meter).unwrap());
    assert!(matches!(
        s.f.kernel.begin(s.f.reader, &mut s.f.meter),
        Err(KernelError::InvalidJob)
    ));
    assert_eq!(s.f.kernel.closure(), Closure::Closed);
    let old = s.read_b();
    s.replace(true);
    assert_eq!(identity, s.f.entry);
    assert!(
        s.f.stale(1),
        "B adopted the empty source premise, so addition invalidates B before X grows"
    );
    s.replace(false);
    s.drain_owners();
    let pending = s.read_b();
    assert_eq!(pending.state, Availability::Pending);
    assert_eq!(pending.version, old.version);
    assert_eq!(
        s.f.kernel
            .closure_in_metered(s.f.downstream, &mut s.f.meter)
            .unwrap(),
        s.f.kernel.closure_in::<Stop>(s.f.downstream).unwrap()
    );
    assert!(matches!(s.f.kernel.closure(), Closure::Open(o) if o.missing == 1));
    let mut run = s.f.kernel.begin(s.f.reader, &mut s.f.meter).unwrap();
    run.observe_slot(&s.anchor).unwrap();
    run.publish(&mut s.shared, Scoped::new(s.f.source, 0), 3)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(s.read_b().value(), Some(&3));
    assert!(!s.f.stale(0) && !s.f.stale(1));
    assert_eq!(s.f.kernel.closure(), Closure::Closed);
}

#[test]
fn withdrawing_one_of_multiple_families_preserves_other_sources_and_reactivates_safely() {
    let mut s = Scenario::new();
    let c =
        s.f.kernel
            .register(s.f.unrelated, (), &mut s.f.meter)
            .unwrap();
    let mut run = s.f.kernel.begin(c, &mut s.f.meter).unwrap();
    run.publish(&mut s.shared, Scoped::new(s.f.source, 0), 2)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    s.replace(false);
    s.drain_owners();
    assert_eq!(s.read_b().value(), Some(&3));
    withdraw(&mut s);
    s.replace(false);
    s.drain_owners();
    assert_eq!(s.read_b().value(), Some(&3));
    assert!(s.f.kernel.task::<Stop>(c).is_ok());
    assert!(!s.f.stale(2));
    assert_eq!(s.f.kernel.closure(), Closure::Closed);
    let mut run = s.f.kernel.begin(s.f.writer, &mut s.f.meter).unwrap();
    run.publish_prepared(&s.anchor, &mut s.native, 5, Change::Changed, 1)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(
        !s.f.stale(1),
        "the confirmed empty family remains authoritative until reactivation"
    );
    s.replace_with_top(true, 0b111);
    assert!(s.f.stale(1));
    s.replace_with_top(false, 0b111);
    s.drain_owners();
    assert_eq!(s.read_b().state, Availability::Pending);
    let mut run = s.f.kernel.begin(s.f.reader, &mut s.f.meter).unwrap();
    run.observe_slot(&s.anchor).unwrap();
    run.publish(&mut s.shared, Scoped::new(s.f.source, 0), 5)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(s.read_b().value(), Some(&7));
    assert!(!s.f.stale(2));
    assert_eq!(s.f.kernel.closure(), Closure::Closed);
}

#[test]
fn withdrawal_can_complete_its_own_run_and_retired_tokens_cannot_resume() {
    let mut f = Fixture::new();
    let mut shared = f.entry.clone();
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    assert_eq!(
        run.withdraw_generation(&mut shared).unwrap(),
        Change::Changed
    );
    assert!(matches!(
        run.publish(&mut f.inputs, Scoped::new(f.source, 0), 3),
        Err(crate::PublishError::Kernel(KernelError::InvalidJob))
    ));
    run.finish(LocalClosure::Stable).unwrap();
    owner_read(&mut f.kernel, &mut f.meter, f.owner, &f.entry);
    assert_eq!(f.kernel.closure(), Closure::Closed);
    let mut token = f
        .kernel
        .begin(f.downstream_reader, &mut f.meter)
        .unwrap()
        .suspend();
    let mut run = f.kernel.begin(f.downstream_owner, &mut f.meter).unwrap();
    run.withdraw_generation(&mut f.downstream_entry).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(matches!(
        f.kernel.resume(&mut token, &mut f.meter),
        Err(KernelError::InvalidJob)
    ));
    owner_read(
        &mut f.kernel,
        &mut f.meter,
        f.downstream_owner,
        &f.downstream_entry,
    );
    assert_eq!(f.kernel.closure(), Closure::Closed);
}

#[test]
fn empty_selector_preserves_lineage_and_fork_selection_is_isolated() {
    for commit in [false, true] {
        let mut s = Scenario::new();
        let old_scope = s.f.target;
        let mut fork = s.f.kernel.fork(&mut s.f.meter).unwrap();
        let mut handle = s.f.entry.clone();
        let mut run = fork.kernel().begin(s.f.owner, &mut s.f.meter).unwrap();
        run.withdraw_generation(&mut handle).unwrap();
        assert_eq!(
            run.current_generation(&handle).unwrap().state,
            Availability::Absent
        );
        run.finish(LocalClosure::Stable).unwrap();
        assert!(!s.f.kernel.scope_stale(old_scope, &mut s.f.meter).unwrap());
        if commit {
            s.f.kernel.commit(fork, &mut s.f.meter).unwrap();
            s.replace(false);
            s.drain_owners();
            assert_eq!(s.read_b().value(), Some(&1));
            assert!(s.f.kernel.scope_stale(old_scope, &mut s.f.meter).unwrap());
        } else {
            drop(fork);
            assert_eq!(s.read_b().value(), Some(&1));
        }
        assert_eq!(s.f.kernel.closure(), Closure::Closed);
    }
    let mut s = empty();
    let bad = scope(&mut s.f.kernel, &mut s.f.meter, 99, 1);
    s.f.kernel.seal_scope(bad, &mut s.f.meter).unwrap();
    let good = scope(&mut s.f.kernel, &mut s.f.meter, s.f.target.key().context, 1);
    let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
    let before = run.current_generation(&s.f.entry).unwrap();
    assert_eq!(
        run.replace_generation(&mut s.f.entry, bad),
        Err(KernelError::WrongScope)
    );
    assert_eq!(
        run.replace_generation(&mut s.f.entry, good),
        Err(KernelError::UnsealedScope)
    );
    assert_eq!(run.current_generation(&s.f.entry).unwrap(), before);
    run.finish(LocalClosure::Stable).unwrap();
    s.f.kernel.seal_scope(good, &mut s.f.meter).unwrap();
    let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
    run.replace_generation(&mut s.f.entry, good).unwrap();
    assert_eq!(
        run.current_generation(&s.f.entry).unwrap().value(),
        Some(&good)
    );
    run.finish(LocalClosure::Stable).unwrap();
}
