//! A -> world-scoped X -> B. B knows only X's typed route and world key;
//! producer assumptions are acquired automatically by the kernel.
mod atomicity;
mod feedback;
mod protocol;
mod sidecars;
mod topology;
mod withdrawal;

use super::*;
use crate::{Answer, Availability, Change, Closure, KernelError, Nil, SlotMeta, solve};

struct Scenario {
    f: Fixture,
    anchor: SlotMeta<u8>,
    native: u8,
    shared: Inputs,
}
impl Scenario {
    fn new() -> Self {
        let mut f = Fixture::new();
        let anchor = f.kernel.slot(f.source, &mut f.meter).unwrap();
        let mut shared = f.kernel.channel(Ascending(Union), &mut f.meter).unwrap();
        let mut run = f.kernel.begin(f.reader, &mut f.meter).unwrap();
        run.observe_slot(&anchor).unwrap();
        run.publish(&mut shared, Scoped::new(f.source, 0), 1)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        let mut this = Self {
            f,
            anchor,
            native: 1,
            shared,
        };
        assert_eq!(this.read_b().value(), Some(&1));
        assert_eq!(this.f.kernel.closure(), Closure::Closed);
        this
    }
    fn grow(&mut self) {
        self.grow_to(3);
    }
    fn grow_to(&mut self, next: u8) {
        let mut run = self
            .f
            .kernel
            .begin(self.f.writer, &mut self.f.meter)
            .unwrap();
        run.publish_prepared(&self.anchor, &mut self.native, next, Change::Changed, 1)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        assert!(self.f.stale(0) && self.f.stale(1));
    }
    fn replace(&mut self, producer: bool) {
        self.replace_with_top(producer, 3);
    }
    fn replace_with_top(&mut self, producer: bool, top: u8) {
        let old = if producer {
            self.f.target
        } else {
            self.f.downstream
        };
        let next = scope(
            &mut self.f.kernel,
            &mut self.f.meter,
            old.key().context,
            old.key().generation + 1,
        );
        let job = self.f.kernel.register(next, (), &mut self.f.meter).unwrap();
        seed(
            &mut self.f.kernel,
            &mut self.f.meter,
            job,
            &mut self.f.local,
            top,
        );
        self.f.kernel.seal_scope(next, &mut self.f.meter).unwrap();
        let owner = if producer {
            self.f.owner
        } else {
            self.f.downstream_owner
        };
        let entry = if producer {
            &mut self.f.entry
        } else {
            &mut self.f.downstream_entry
        };
        let mut run = self.f.kernel.begin(owner, &mut self.f.meter).unwrap();
        run.replace_generation(entry, next).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        self.f
            .kernel
            .reclaim_channel_scope(&mut self.f.local, old, &mut self.f.meter)
            .unwrap();
        if producer {
            self.f.target = next;
            self.f.reader = job;
        } else {
            self.f.downstream = next;
            self.f.downstream_reader = job;
        }
    }
    fn read_b(&mut self) -> Answer<u8> {
        let mut run = self
            .f
            .kernel
            .begin(self.f.downstream_reader, &mut self.f.meter)
            .unwrap();
        // Deliberately no A entry/input/anchor argument at this consumer boundary.
        let answer = run
            .query(&mut self.shared, Scoped::new(self.f.source, 0))
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        answer
    }
    fn renew(&mut self) -> Change {
        let mut run = self
            .f
            .kernel
            .begin(self.f.reader, &mut self.f.meter)
            .unwrap();
        run.observe_slot(&self.anchor).unwrap();
        let changed = run
            .publish(&mut self.shared, Scoped::new(self.f.source, 0), 1)
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        changed
    }
    fn drain_owners(&mut self) {
        owner_read(
            &mut self.f.kernel,
            &mut self.f.meter,
            self.f.owner,
            &self.f.entry,
        );
        owner_read(
            &mut self.f.kernel,
            &mut self.f.meter,
            self.f.downstream_owner,
            &self.f.downstream_entry,
        );
    }
}

struct NoOp;
impl<Cx> Analyzer<Cx> for NoOp {
    type Task = ();
    type Error = Infallible;
    fn evaluate(&mut self, _: &(), _: &mut Cx) -> Result<LocalClosure, Infallible> {
        Ok(LocalClosure::Stable)
    }
}
