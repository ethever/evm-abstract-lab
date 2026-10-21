//! Reviewer witnesses use a declared finite two-key universe and complete per-key
//! MAY contributions. A/C and both A jobs exist before sealing or enumeration.
use super::{Bits, Budget, Inputs, Stop, Union, refine, scope};
use crate::{
    Answer, Ascending, Availability, Change, Descending, GenerationEntry, JobId, Kernel,
    KernelError, LocalClosure, PublishError, Scope, Scoped,
};

struct Topology {
    kernel: Kernel<()>,
    meter: Budget,
    values: Inputs,
    consumer: Scope,
    owner: JobId,
    first: JobId,
    later: JobId,
    reader: JobId,
    third: JobId,
    entries: [GenerationEntry; 3],
}
impl Topology {
    fn new() -> Self {
        let mut kernel = Kernel::new().unwrap();
        let mut meter = Budget::default();
        let world = scope(&mut kernel, &mut meter, 0, 0);
        let a = scope(&mut kernel, &mut meter, 1, 0);
        let consumer = scope(&mut kernel, &mut meter, 2, 0);
        let c = scope(&mut kernel, &mut meter, 3, 0);
        let owner = kernel.register(world, (), &mut meter).unwrap();
        let first = kernel.register(a, (), &mut meter).unwrap();
        let later = kernel.register(a, (), &mut meter).unwrap();
        let reader = kernel.register(consumer, (), &mut meter).unwrap();
        let third = kernel.register(c, (), &mut meter).unwrap();
        let values = kernel.channel(Ascending(Union), &mut meter).unwrap();
        for s in [world, a, consumer, c] {
            kernel.seal_scope(s, &mut meter).unwrap();
        }
        let entries =
            [a, consumer, c].map(|s| kernel.generation_entry(world, s, &mut meter).unwrap());
        let mut run = kernel.begin(owner, &mut meter).unwrap();
        for entry in &entries {
            run.current_generation(entry).unwrap();
        }
        run.finish(LocalClosure::Stable).unwrap();
        Self {
            kernel,
            meter,
            values,
            consumer,
            owner,
            first,
            later,
            reader,
            third,
            entries,
        }
    }
    fn publish(&mut self, job: JobId, key: u8) -> Change {
        let mut run = self.kernel.begin(job, &mut self.meter).unwrap();
        let change = run
            .publish(&mut self.values, Scoped::new(self.consumer, key), 1)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        change
    }
    fn keys(&mut self, job: JobId) -> Vec<u8> {
        let mut run = self.kernel.begin(job, &mut self.meter).unwrap();
        let keys = run.keys(&mut self.values, self.consumer).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        keys
    }
    fn stale(&mut self, family: usize) -> bool {
        self.kernel
            .generation_stale(&self.entries[family], &mut self.meter)
            .unwrap()
    }
    fn answers(&mut self, new_key: bool) -> Vec<Answer<u8>> {
        let mut run = self.kernel.begin(self.reader, &mut self.meter).unwrap();
        let answers = (0..=u8::from(new_key))
            .map(|key| {
                run.query_discovery(&mut self.values, Scoped::new(self.consumer, key))
                    .unwrap()
            })
            .collect();
        run.finish(LocalClosure::Stable).unwrap();
        answers
    }
    fn observed(new_key: bool) -> Self {
        let mut t = Self::new();
        t.publish(t.first, 0);
        assert_eq!(t.keys(t.reader), [0]);
        if new_key {
            // Preintern Pending separately so publication refusal assertions can
            // compare the complete preexisting cell and queue state.
            let mut run = t.kernel.begin(t.reader, &mut t.meter).unwrap();
            assert_eq!(
                run.query_discovery(&mut t.values, Scoped::new(t.consumer, 1))
                    .unwrap()
                    .state,
                Availability::Pending
            );
            run.finish(LocalClosure::Stable).unwrap();
        }
        t
    }
}

#[test]
fn empty_own_scope_directory_retains_the_first_foreign_membership_premise() {
    for absent in [false, true] {
        let mut t = Topology::new();
        assert!(t.keys(t.reader).is_empty());
        // These finite producer/key inventories are established by the caller;
        // an empty resolved directory does not itself prove an open world closed.
        let mut run = t.kernel.begin(t.first, &mut t.meter).unwrap();
        if absent {
            run.resolve_absent(&mut t.values, Scoped::new(t.consumer, 0))
                .unwrap();
        } else {
            run.publish(&mut t.values, Scoped::new(t.consumer, 0), 1)
                .unwrap();
        }
        run.finish(LocalClosure::Stable).unwrap();
        assert!(t.stale(1));
    }
}

#[test]
fn genuine_local_directory_feedback_wakes_without_reseeding() {
    let mut t = Topology::new();
    assert!(t.keys(t.reader).is_empty());
    for key in [0, 1] {
        t.publish(t.reader, key);
        assert!(!t.stale(1));
        assert!(t.kernel.worklist().contains(&t.reader));
        assert_eq!(t.keys(t.reader), (0..=key).collect::<Vec<_>>());
    }
    assert_eq!(t.publish(t.reader, 1), Change::Unchanged);
    assert!(!t.kernel.worklist().contains(&t.reader));

    let mut local = t
        .kernel
        .channel::<Bits, _, _>(Descending(super::super::Meet), &mut t.meter)
        .unwrap();
    let mut run = t.kernel.begin(t.reader, &mut t.meter).unwrap();
    assert!(run.keys(&mut local, t.consumer).unwrap().is_empty());
    // Coarse {0,1} -> {1} within a fixed input universe; first membership and
    // descending refinement are both local cooperation, not foreign growth.
    refine(&mut run, &mut local, 3);
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!t.stale(1));
    let mut run = t.kernel.begin(t.reader, &mut t.meter).unwrap();
    assert_eq!(run.keys(&mut local, t.consumer).unwrap(), [0]);
    refine(&mut run, &mut local, 1);
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!t.stale(1));
}

#[test]
fn local_feedback_exemption_does_not_hide_growth_from_external_directory_adopters() {
    let mut t = Topology::new();
    assert!(t.keys(t.reader).is_empty());
    assert!(t.keys(t.third).is_empty());
    t.publish(t.reader, 0);
    assert!(!t.stale(1));
    assert!(t.stale(2));
}

#[test]
fn own_scope_multisource_directory_still_adopts_producer_retirement() {
    let mut t = Topology::new();
    t.publish(t.first, 0);
    t.publish(t.third, 0);
    assert_eq!(t.keys(t.reader), [0]);
    let mut run = t.kernel.begin(t.owner, &mut t.meter).unwrap();
    run.withdraw_generation(&mut t.entries[0]).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(t.stale(1));
    assert!(!t.stale(2));
    assert!(
        !t.stale(0),
        "confirmed absence is resolved, not an unrepairable source"
    );
    assert_eq!(
        t.values.len(),
        1,
        "withdrawing one family does not erase the aggregate"
    );
}

fn publication_reservations(new_key: bool) -> usize {
    let mut t = Topology::observed(new_key);
    drop(t.answers(new_key)); // Match the fault cases' discovery subscriptions.
    assert!(!t.stale(1));
    let job = if new_key { t.later } else { t.third };
    let mut run = t.kernel.begin(job, &mut t.meter).unwrap();
    let before = run.meter().calls;
    assert_eq!(
        run.publish(&mut t.values, Scoped::new(t.consumer, u8::from(new_key)), 1)
            .unwrap(),
        Change::Changed
    );
    let phases = run.meter().calls - before;
    run.finish(LocalClosure::Stable).unwrap();
    assert!(
        t.stale(1),
        "foreign membership or a new family changes B's premise"
    );
    assert!(!t.stale(0) && !t.stale(2));
    assert!(t.kernel.worklist().contains(&t.owner));
    assert!(!t.kernel.worklist().contains(&t.reader));
    assert!(phases > 0);
    phases
}

fn unchanged(t: &mut Topology, len: usize, new_key: bool, before: &[Answer<u8>]) {
    assert_eq!(t.values.len(), len);
    assert!(!t.stale(0) && !t.stale(1) && !t.stale(2));
    assert_eq!(t.keys(t.reader), [0]);
    assert_eq!(
        t.answers(new_key),
        before,
        "values, coverage availability and versions are unchanged"
    );
}

#[test]
fn every_foreign_directory_publication_refusal_and_cancellation_is_atomic() {
    for new_key in [false, true] {
        let phases = publication_reservations(new_key);
        for stop in [Stop::Work, Stop::Cancelled] {
            for phase in 1..=phases {
                let mut t = Topology::observed(new_key);
                let len = t.values.len();
                let before = t.answers(new_key);
                let job = if new_key { t.later } else { t.third };
                let mut token = t.kernel.begin(job, &mut t.meter).unwrap().suspend();
                let queue = t.kernel.worklist().clone();
                let mut run = t.kernel.resume(&mut token, &mut t.meter).unwrap();
                run.meter().stop_after(phase, stop);
                assert_eq!(
                    run.publish(&mut t.values, Scoped::new(t.consumer, u8::from(new_key)), 1),
                    Err(PublishError::Kernel(KernelError::Stopped(stop))),
                    "new key {new_key}, {stop:?} at reservation {phase}"
                );
                drop(run);
                t.meter.clear_stop();
                assert_eq!(t.kernel.worklist(), &queue);
                unchanged(&mut t, len, new_key, &before);
                // Refused or cancelled declarations must never become premises.
                let mut run = t.kernel.begin(t.owner, &mut t.meter).unwrap();
                run.withdraw_generation(&mut t.entries[2]).unwrap();
                run.finish(LocalClosure::Stable).unwrap();
                assert!(!t.stale(1));
            }
        }
    }
}
