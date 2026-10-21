use super::*;
use crate::{KernelError, Meter, PublishError};
use std::{cell::Cell, rc::Rc};

const PREPARATION: usize = 19;

#[derive(Default)]
struct Hook {
    combined: Cell<usize>,
    classified: Cell<usize>,
    cancelled: Cell<bool>,
    cancel_on_classification: Cell<bool>,
    refuse: Cell<bool>,
    verify_payment: Cell<bool>,
    paid: Cell<usize>,
    minimum_paid: Cell<usize>,
}
impl Hook {
    fn check_payment(&self) {
        if self.verify_payment.get() {
            assert!(self.paid.get() >= self.minimum_paid.get());
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Refused;

struct CheckedPolicy(Rc<Hook>);
impl PublicationPolicy for CheckedPolicy {
    type Value = Observation;
    type Error = Refused;
    const DESCENDING: bool = false;
    fn work(&self, _: &Observation, _: &Observation) -> usize {
        self.0.minimum_paid.set(self.0.paid.get() + PREPARATION);
        PREPARATION
    }
    fn combine(&self, old: &Observation, incoming: &Observation) -> Result<Observation, Refused> {
        self.0.check_payment();
        self.0.combined.set(self.0.combined.get() + 1);
        if self.0.refuse.get() {
            return Err(Refused);
        }
        Ok(ObservationPolicy.combine(old, incoming).unwrap())
    }
    fn equivalent(&self, old: &Observation, next: &Observation) -> bool {
        ObservationPolicy.equivalent(old, next)
    }
    fn preserves_guarantees(&self, old: &Observation, next: &Observation) -> bool {
        self.0.check_payment();
        self.0.classified.set(self.0.classified.get() + 1);
        if self.0.cancel_on_classification.get() {
            self.0.cancelled.set(true);
        }
        ObservationPolicy.preserves_guarantees(old, next)
    }
}

struct CheckedMeter<'a> {
    root: &'a mut Budget,
    hook: &'a Hook,
    refuse_preparation: bool,
}
impl Meter for CheckedMeter<'_> {
    type Stop = Stop;
    fn reserve(&mut self, work: usize) -> Result<(), Stop> {
        if self.refuse_preparation && work == PREPARATION {
            return Err(Stop::Work);
        }
        self.root.reserve(work)?;
        self.hook.paid.set(self.root.used);
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        if self.hook.cancelled.get() {
            Err(Stop::Cancelled)
        } else {
            self.root.checkpoint()
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Update {
    Completion,
    Reopening,
    NewFamily,
    Renewal,
}
const UPDATES: [Update; 4] = [
    Update::Completion,
    Update::Reopening,
    Update::NewFamily,
    Update::Renewal,
];
fn setup(update: Update) -> (Scenario<CheckedPolicy>, Rc<Hook>, JobId, Observation) {
    let hook = Rc::new(Hook::default());
    let mut s = Scenario::new(CheckedPolicy(Rc::clone(&hook)));
    if matches!(update, Update::Reopening | Update::Renewal) {
        s.publish(s.f.reader, 0, COMPLETE);
        s.read_b();
    }
    if matches!(update, Update::Renewal) {
        s.grow_source();
        s.replace(false);
        s.replace(true);
        owner_read(&mut s.f.kernel, &mut s.f.meter, s.f.owner, &s.f.entry);
        owner_read(
            &mut s.f.kernel,
            &mut s.f.meter,
            s.f.downstream_owner,
            &s.f.downstream_entry,
        );
        assert_eq!(s.read_b().state, Availability::Pending);
    }
    let job = if matches!(update, Update::NewFamily) {
        s.f.kernel
            .register(s.f.unrelated, (), &mut s.f.meter)
            .unwrap()
    } else {
        s.f.reader
    };
    hook.combined.set(0);
    hook.classified.set(0);
    let incoming = if matches!(update, Update::Reopening) {
        OPEN
    } else {
        COMPLETE
    };
    (s, hook, job, incoming)
}
fn reservations(update: Update) -> usize {
    let (mut s, _, job, incoming) = setup(update);
    let mut run = s.f.kernel.begin(job, &mut s.f.meter).unwrap();
    let before = run.meter().calls;
    run.publish(&mut s.values, Scoped::new(s.f.source, 0), incoming)
        .unwrap();
    run.meter().calls - before
}

#[test]
fn every_publication_refusal_keeps_value_version_coverage_staleness_and_wakes_atomic() {
    for update in UPDATES {
        for fail_at in 1..=reservations(update) {
            let (mut s, _, job, incoming) = setup(update);
            let before = s.read_world();
            let mut run = s.f.kernel.begin(job, &mut s.f.meter).unwrap();
            let queue = run.kernel().worklist().clone();
            run.meter().fail_on = Some(run.meter().calls + fail_at);
            assert_eq!(
                run.publish(&mut s.values, Scoped::new(s.f.source, 0), incoming),
                Err(PublishError::Kernel(KernelError::Stopped(Stop::Work))),
                "{update:?}, reservation {fail_at}"
            );
            run.meter().fail_on = None;
            assert_eq!(run.kernel().worklist(), &queue);
            run.finish(LocalClosure::Stable).unwrap();
            assert_eq!(s.read_world(), before);
            assert!(!s.f.stale(0) && !s.f.stale(1) && !s.f.stale(2));
            assert_eq!(s.publish(job, 0, incoming), Change::Changed);
        }
    }
}

#[test]
fn classification_and_combine_are_prepaid_by_one_existing_preparation_quote() {
    for refuse in [true, false] {
        let (mut s, hook, job, incoming) = setup(Update::Completion);
        hook.verify_payment.set(true);
        let before = s.read_world();
        let mut meter = CheckedMeter {
            root: &mut s.f.meter,
            hook: &hook,
            refuse_preparation: refuse,
        };
        let mut run = s.f.kernel.begin(job, &mut meter).unwrap();
        let result = run.publish(&mut s.values, Scoped::new(s.f.source, 0), incoming);
        if refuse {
            assert_eq!(
                result,
                Err(PublishError::Kernel(KernelError::Stopped(Stop::Work)))
            );
            assert_eq!((hook.combined.get(), hook.classified.get()), (0, 0));
        } else {
            assert_eq!(result, Ok(Change::Changed));
            assert_eq!((hook.combined.get(), hook.classified.get()), (1, 1));
        }
        run.finish(LocalClosure::Stable).unwrap();
        if refuse {
            assert_eq!(s.read_world(), before);
        } else {
            assert_eq!(s.read_b().value(), Some(&COMPLETE));
            assert!(!s.f.stale(1));
        }
    }
}

#[test]
fn cancellation_set_by_preserving_or_invalidating_classification_commits_nothing() {
    for update in [Update::Completion, Update::Reopening, Update::NewFamily] {
        let (mut s, hook, job, incoming) = setup(update);
        let before = s.read_world();
        hook.verify_payment.set(true);
        hook.cancel_on_classification.set(true);
        let mut meter = CheckedMeter {
            root: &mut s.f.meter,
            hook: &hook,
            refuse_preparation: false,
        };
        let mut run = s.f.kernel.begin(job, &mut meter).unwrap();
        let queue = run.kernel().worklist().clone();
        assert_eq!(
            run.publish(&mut s.values, Scoped::new(s.f.source, 0), incoming),
            Err(PublishError::Kernel(KernelError::Stopped(Stop::Cancelled)))
        );
        assert_eq!(run.kernel().worklist(), &queue);
        assert_eq!(hook.classified.get(), 1);
        hook.cancelled.set(false);
        run.finish(LocalClosure::Stable).unwrap();
        assert_eq!(s.read_world(), before);
        assert!(!s.f.stale(0) && !s.f.stale(1) && !s.f.stale(2));
    }
}

#[test]
fn semantic_refusal_and_equivalence_never_call_classification() {
    let (mut s, hook, job, _) = setup(Update::Completion);
    let before = s.read_world();
    hook.refuse.set(true);
    let mut run = s.f.kernel.begin(job, &mut s.f.meter).unwrap();
    let queue = run.kernel().worklist().clone();
    assert_eq!(
        run.publish(&mut s.values, Scoped::new(s.f.source, 0), COMPLETE),
        Err(PublishError::Policy(Refused))
    );
    assert_eq!(hook.classified.get(), 0);
    assert_eq!(run.kernel().worklist(), &queue);
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(s.read_world(), before);
    assert!(!s.f.stale(1));
    hook.refuse.set(false);
    assert_eq!(s.publish(job, 0, OPEN), Change::Unchanged);
    assert_eq!(hook.classified.get(), 0);

    let (mut s, hook, job, incoming) = setup(Update::Renewal);
    assert_eq!(s.publish(job, 0, incoming), Change::Changed);
    assert_eq!(
        hook.classified.get(),
        0,
        "equivalent coverage renewal skips classification"
    );
    assert_eq!(s.read_b().value(), Some(&COMPLETE));
}

struct LocalPolicy;
impl PublicationPolicy for LocalPolicy {
    type Value = Observation;
    type Error = Infallible;
    const DESCENDING: bool = true;
    fn work(&self, old: &Observation, incoming: &Observation) -> usize {
        ObservationPolicy.work(old, incoming)
    }
    fn combine(
        &self,
        old: &Observation,
        incoming: &Observation,
    ) -> Result<Observation, Infallible> {
        ObservationPolicy.combine(old, incoming)
    }
    fn equivalent(&self, old: &Observation, next: &Observation) -> bool {
        old == next
    }
    fn preserves_guarantees(&self, old: &Observation, next: &Observation) -> bool {
        ObservationPolicy.preserves_guarantees(old, next)
    }
}

#[test]
fn preserving_policy_does_not_grant_registration_or_foreign_scope_write_permission() {
    use crate::{Cons, Nil};

    struct Initialize;
    impl<Cx: Accepts<Observations>> Reduction<Cx> for Initialize {
        type Task = Scope;
        type Error = <Cx as Accepts<Observations>>::Error;
        fn reduce(&mut self, scope: &Scope, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
            cx.accept(Scoped::new(*scope, 0), COMPLETE)?;
            Ok(LocalClosure::Stable)
        }
    }

    let mut f = Fixture::new();
    let mut stores = Cons::new(
        f.kernel
            .channel::<Observations, _, _>(LocalPolicy, &mut f.meter)
            .unwrap(),
        Nil,
    );
    let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
    assert_eq!(
        run.publish(&mut stores.head, Scoped::new(f.target, 0), COMPLETE),
        Err(PublishError::Kernel(KernelError::UnregisteredReduction))
    );
    assert!(stores.head.is_empty());
    let mut rule = Registered::new(Initialize);
    assert_eq!(
        rule.evaluate(&f.source, &mut Context::new(&mut run, &mut stores)),
        Err(PublishError::Kernel(KernelError::WrongScope))
    );
    assert!(stores.head.is_empty());
    rule.evaluate(&f.target, &mut Context::new(&mut run, &mut stores))
        .unwrap();
    // Even after an initialized value exists, classification grants no authority
    // to an ordinary analyzer (and must never run before that permission check).
    assert_eq!(
        run.publish(&mut stores.head, Scoped::new(f.target, 0), OPEN),
        Err(PublishError::Kernel(KernelError::UnregisteredReduction))
    );
    run.finish(LocalClosure::Stable).unwrap();
}
