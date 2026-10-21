//! Mixed finite observations, independent of any language adapter. The bound is
//! always a sound may envelope; completion additionally certifies accounted work
//! within the already declared source universe.

mod atomicity;
mod cycle;
mod guards;

use super::*;
use crate::{Answer, Availability, Change, Closure, SlotMeta, algebra::PublicationPolicy};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Observation {
    bound: u8,
    complete: bool,
}
const OPEN: Observation = Observation {
    bound: 1,
    complete: false,
};
const COMPLETE: Observation = Observation {
    complete: true,
    ..OPEN
};

struct Observations;
impl Capability for Observations {
    type Key = u8;
    type Value = Observation;
    fn key_work(_: &u8) -> usize {
        1
    }
    fn value_work(_: &Observation) -> usize {
        1
    }
}

struct ObservationPolicy;
impl PublicationPolicy for ObservationPolicy {
    type Value = Observation;
    type Error = Infallible;
    const DESCENDING: bool = false;
    fn work(&self, _: &Observation, _: &Observation) -> usize {
        8
    }
    fn combine(
        &self,
        old: &Observation,
        incoming: &Observation,
    ) -> Result<Observation, Infallible> {
        let bound = old.bound | incoming.bound;
        Ok(Observation {
            bound,
            complete: incoming.complete && bound == incoming.bound,
        })
    }
    fn equivalent(&self, left: &Observation, right: &Observation) -> bool {
        left == right
    }
    fn preserves_guarantees(&self, old: &Observation, next: &Observation) -> bool {
        old.bound == next.bound && (!old.complete || next.complete)
    }
}

type Observed<P = ObservationPolicy> = Channel<Observations, P>;

struct Scenario<P = ObservationPolicy>
where
    P: PublicationPolicy<Value = Observation>,
{
    f: Fixture,
    values: Observed<P>,
    anchor: SlotMeta<u8>,
    native: u8,
}
impl<P> Scenario<P>
where
    P: PublicationPolicy<Value = Observation>,
    P::Error: std::fmt::Debug,
{
    fn new(policy: P) -> Self {
        let mut f = Fixture::new();
        let anchor = f.kernel.slot(f.source, &mut f.meter).unwrap();
        let mut values = f.kernel.channel(policy, &mut f.meter).unwrap();
        let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
        run.observe_slot(&anchor).unwrap();
        run.publish(&mut values, Scoped::new(f.source, 0), OPEN)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        let mut this = Self {
            f,
            values,
            anchor,
            native: 1,
        };
        assert_eq!(this.read_b().value(), Some(&OPEN));
        assert_eq!(this.f.kernel.closure(), Closure::Closed);
        this
    }
    fn publish(&mut self, job: JobId, key: u8, value: Observation) -> Change {
        let mut run = self.f.kernel.begin(job, &mut self.f.meter).unwrap();
        let change = run
            .publish(&mut self.values, Scoped::new(self.f.source, key), value)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        change
    }
    fn read_b(&mut self) -> Answer<Observation> {
        let mut run = self
            .f
            .kernel
            .begin(self.f.downstream_reader, &mut self.f.meter)
            .unwrap();
        let answer = run
            .query(&mut self.values, Scoped::new(self.f.source, 0))
            .unwrap();
        if let Some(value) = answer.value() {
            refine(&mut run, &mut self.f.local, value.bound);
        }
        run.finish(LocalClosure::Stable).unwrap();
        answer
    }
    fn read_world(&mut self) -> Answer<Observation> {
        let mut run = self
            .f
            .kernel
            .begin(self.f.writer, &mut self.f.meter)
            .unwrap();
        let answer = run
            .query(&mut self.values, Scoped::new(self.f.source, 0))
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        answer
    }
    fn guarantee(&mut self) -> Answer<u8> {
        let mut run = self
            .f
            .kernel
            .begin(self.f.writer, &mut self.f.meter)
            .unwrap();
        let answer = run
            .query(&mut self.f.local, Scoped::new(self.f.downstream, 0))
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        answer
    }
    fn grow_source(&mut self) {
        let mut run = self
            .f
            .kernel
            .begin(self.f.writer, &mut self.f.meter)
            .unwrap();
        run.publish_prepared(&self.anchor, &mut self.native, 3, Change::Changed, 1)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    fn replace(&mut self, producer: bool) {
        self.f.replace(producer, 3);
    }
}

#[test]
fn finite_observation_classification_preserves_all_old_guarantees() {
    let values: Vec<_> = (0..8)
        .flat_map(|bound| [false, true].map(|complete| Observation { bound, complete }))
        .collect();
    let admits = |observation: Observation, outcomes: u8, done: bool| {
        outcomes & !observation.bound == 0 && (!observation.complete || done)
    };
    for old in &values {
        assert_eq!(ObservationPolicy.combine(old, old).unwrap(), *old);
        for incoming in &values {
            let next = ObservationPolicy.combine(old, incoming).unwrap();
            assert_eq!(old.bound & !next.bound, 0);
            assert_eq!(incoming.bound & !next.bound, 0);
            if ObservationPolicy.preserves_guarantees(old, &next) {
                for outcomes in 0..8 {
                    for done in [false, true] {
                        assert!(!admits(next, outcomes, done) || admits(*old, outcomes, done));
                    }
                }
            }
        }
    }
}

#[test]
fn completion_enhancement_installs_and_wakes_without_staling_an_adopted_guarantee() {
    let mut s = Scenario::new(ObservationPolicy);
    let before = s.read_b();
    let guarantee = s.guarantee();
    assert_eq!(guarantee.value(), Some(&1));
    assert_eq!(s.publish(s.f.reader, 0, COMPLETE), Change::Changed);
    assert!(s.f.kernel.worklist().contains(&s.f.downstream_reader));
    assert!(!s.f.kernel.worklist().contains(&s.f.downstream_owner));
    assert!(!s.f.stale(0) && !s.f.stale(1));
    assert_eq!(s.guarantee(), guarantee);
    let after = s.read_b();
    assert_eq!(after.value(), Some(&COMPLETE));
    assert_eq!(after.version.get(), before.version.get() + 1);
    assert_eq!(s.publish(s.f.reader, 0, COMPLETE), Change::Unchanged);
    assert_eq!(s.read_b(), after);
    assert_eq!(s.f.kernel.closure(), Closure::Closed);
}

#[test]
fn bound_growth_and_completion_reopening_stale_old_guarantees() {
    for next in [
        Observation {
            bound: 3,
            ..COMPLETE
        },
        OPEN,
    ] {
        let mut s = Scenario::new(ObservationPolicy);
        s.publish(s.f.reader, 0, COMPLETE);
        s.read_b();
        assert_eq!(s.publish(s.f.reader, 0, next), Change::Changed);
        assert!(!s.f.stale(0) && s.f.stale(1));
        assert_eq!(s.guarantee().state, Availability::Pending);
        assert!(s.f.kernel.worklist().contains(&s.f.downstream_owner));
    }
}

#[test]
fn classification_uses_prepared_value_rather_than_incoming_completion() {
    let mut s = Scenario::new(ObservationPolicy);
    s.publish(s.f.reader, 0, COMPLETE);
    s.read_b();
    let incoming = Observation {
        bound: 0,
        complete: true,
    };
    s.publish(s.f.reader, 0, incoming);
    assert_eq!(s.read_world().value(), Some(&OPEN));
    assert!(
        s.f.stale(1),
        "the prepared result reopens the retained larger bound"
    );
}
