//! Generation assumptions, publication atomicity and convergence through the real
//! scheduler. Helpers use registered reductions and public routing capabilities.

mod api;
mod atomicity;
mod convergence;
mod discovery;
mod lifecycle;
mod provenance;
mod publication;
mod readiness;
mod self_callback;

use super::{Budget, Noop, Stop, scope};
use crate::{
    Accepts, Analyzer, Ascending, Capability, Channel, Context, Descending, GenerationEntry, JobId,
    JoinPolicy, Kernel, LocalClosure, ReadGeneration, Reduction, RefinePolicy, Registered, Run,
    Scope, Scoped,
};
use std::convert::Infallible;

struct Bits;
impl Capability for Bits {
    type Key = u8;
    type Value = u8;
    fn key_work(_: &u8) -> usize {
        1
    }
    fn value_work(_: &u8) -> usize {
        1
    }
}
struct Union;
impl JoinPolicy for Union {
    type Value = u8;
    type Error = Infallible;
    fn work(&self, _: &u8, _: &u8) -> usize {
        1
    }
    fn join(&self, a: &u8, b: &u8) -> Result<u8, Infallible> {
        Ok(a | b)
    }
    fn equivalent(&self, a: &u8, b: &u8) -> bool {
        a == b
    }
}
struct Meet;
impl RefinePolicy for Meet {
    type Value = u8;
    type Error = Infallible;
    fn work(&self, _: &u8, _: &u8) -> usize {
        1
    }
    fn refine(&self, a: &u8, b: &u8) -> Result<u8, Infallible> {
        Ok(a & b)
    }
    fn equivalent(&self, a: &u8, b: &u8) -> bool {
        a == b
    }
}
type Inputs = Channel<Bits, Ascending<Union>>;
type Guarantees = Channel<Bits, Descending<Meet>>;

struct Seed;
impl<Cx: Accepts<Bits>> Reduction<Cx> for Seed {
    type Task = (Scope, u8);
    type Error = <Cx as Accepts<Bits>>::Error;
    fn reduce(
        &mut self,
        (scope, value): &Self::Task,
        cx: &mut Cx,
    ) -> Result<LocalClosure, Self::Error> {
        cx.accept(Scoped::new(*scope, 0), *value)?;
        Ok(LocalClosure::Stable)
    }
}
fn refine(run: &mut Run<'_, (), Budget>, values: &mut Guarantees, value: u8) {
    let target = run.scope();
    // Context needs an owned Channel at its route. A small provider facade keeps
    // this helper on the actual registered Reduction path without moving stores.
    struct Route<'a>(&'a mut Guarantees);
    impl crate::Select<crate::Here> for Route<'_> {
        type Output = Guarantees;
        fn select(&self) -> &Guarantees {
            self.0
        }
        fn select_mut(&mut self) -> &mut Guarantees {
            self.0
        }
    }
    Registered::new(Seed)
        .evaluate(&(target, value), &mut Context::new(run, &mut Route(values)))
        .unwrap();
}
fn route<Cx: ReadGeneration>(
    cx: &mut Cx,
    entry: &GenerationEntry,
) -> Result<crate::Answer<Scope>, Cx::Error> {
    cx.current_generation(entry)
}
fn owner_read(kernel: &mut Kernel<()>, meter: &mut Budget, job: JobId, entry: &GenerationEntry) {
    let mut run = kernel.begin(job, meter).unwrap();
    route(&mut Context::new(&mut run, &mut crate::Nil), entry).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
}
fn seed(
    kernel: &mut Kernel<()>,
    meter: &mut Budget,
    job: JobId,
    values: &mut Guarantees,
    value: u8,
) {
    let mut run = kernel.begin(job, meter).unwrap();
    refine(&mut run, values, value);
    run.finish(LocalClosure::Stable).unwrap();
}
fn publish(
    kernel: &mut Kernel<()>,
    meter: &mut Budget,
    writer: JobId,
    values: &mut Inputs,
    source: Scope,
    key: u8,
    value: u8,
) {
    let mut run = kernel.begin(writer, meter).unwrap();
    run.publish(values, Scoped::new(source, key), value)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
}

struct Fixture {
    kernel: Kernel<()>,
    meter: Budget,
    inputs: Inputs,
    local: Guarantees,
    source: Scope,
    target: Scope,
    downstream: Scope,
    unrelated: Scope,
    writer: JobId,
    reader: JobId,
    downstream_reader: JobId,
    owner: JobId,
    downstream_owner: JobId,
    entry: GenerationEntry,
    downstream_entry: GenerationEntry,
    unrelated_entry: GenerationEntry,
}
impl Fixture {
    fn new() -> Self {
        Self::configured(1, false)
    }
    fn configured(universe: u8, reverse: bool) -> Self {
        let mut kernel = Kernel::new().unwrap();
        let mut meter = Budget::default();
        let stable = scope(&mut kernel, &mut meter, 0, 0);
        let source = scope(&mut kernel, &mut meter, 1, 0);
        let target = scope(&mut kernel, &mut meter, 2, 0);
        let downstream = scope(&mut kernel, &mut meter, 3, 0);
        let unrelated = scope(&mut kernel, &mut meter, 4, 0);
        let writer = kernel.register(source, (), &mut meter).unwrap();
        let reader = kernel.register(target, (), &mut meter).unwrap();
        let downstream_reader = kernel.register(downstream, (), &mut meter).unwrap();
        let owner = kernel.register(stable, (), &mut meter).unwrap();
        let downstream_owner = kernel.register(stable, (), &mut meter).unwrap();
        let mut inputs = kernel.channel(Ascending(Union), &mut meter).unwrap();
        let mut local = kernel.channel(Descending(Meet), &mut meter).unwrap();
        publish(
            &mut kernel,
            &mut meter,
            writer,
            &mut inputs,
            source,
            0,
            universe,
        );
        // Seed/register/seal, publish stable entry, register owner, then drain.
        seed(&mut kernel, &mut meter, reader, &mut local, universe);
        seed(
            &mut kernel,
            &mut meter,
            downstream_reader,
            &mut local,
            universe,
        );
        for scope in [stable, source, target, downstream, unrelated] {
            kernel.seal_scope(scope, &mut meter).unwrap();
        }
        let (entry, downstream_entry) = if reverse {
            let b = kernel
                .generation_entry(stable, downstream, &mut meter)
                .unwrap();
            let a = kernel.generation_entry(stable, target, &mut meter).unwrap();
            (a, b)
        } else {
            let a = kernel.generation_entry(stable, target, &mut meter).unwrap();
            let b = kernel
                .generation_entry(stable, downstream, &mut meter)
                .unwrap();
            (a, b)
        };
        let unrelated_entry = kernel
            .generation_entry(stable, unrelated, &mut meter)
            .unwrap();
        owner_read(&mut kernel, &mut meter, owner, &entry);
        owner_read(&mut kernel, &mut meter, downstream_owner, &downstream_entry);
        Self {
            kernel,
            meter,
            inputs,
            local,
            source,
            target,
            downstream,
            unrelated,
            writer,
            reader,
            downstream_reader,
            owner,
            downstream_owner,
            entry,
            downstream_entry,
            unrelated_entry,
        }
    }
    fn next_generation(&mut self, old: Scope, top: u8) -> (Scope, JobId) {
        let next = scope(
            &mut self.kernel,
            &mut self.meter,
            old.key().context,
            old.key().generation + 1,
        );
        let job = self.kernel.register(next, (), &mut self.meter).unwrap();
        seed(&mut self.kernel, &mut self.meter, job, &mut self.local, top);
        self.kernel.seal_scope(next, &mut self.meter).unwrap();
        (next, job)
    }
    // Seed and seal before selecting; each scenario owns its sidecar cleanup.
    fn replace(&mut self, producer: bool, top: u8) -> Scope {
        let old = if producer {
            self.target
        } else {
            self.downstream
        };
        let (next, job) = self.next_generation(old, top);
        let (owner, entry) = if producer {
            (self.owner, &mut self.entry)
        } else {
            (self.downstream_owner, &mut self.downstream_entry)
        };
        let mut run = self.kernel.begin(owner, &mut self.meter).unwrap();
        run.replace_generation(entry, next).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        if producer {
            self.target = next;
            self.reader = job;
        } else {
            self.downstream = next;
            self.downstream_reader = job;
        }
        old
    }
    fn adopt(&mut self) {
        let mut run = self.kernel.begin(self.reader, &mut self.meter).unwrap();
        assert_eq!(
            run.query(&mut self.inputs, Scoped::new(self.source, 0))
                .unwrap()
                .value(),
            Some(&1)
        );
        run.finish(LocalClosure::Stable).unwrap();
        let mut run = self
            .kernel
            .begin(self.downstream_reader, &mut self.meter)
            .unwrap();
        assert_eq!(
            run.query(&mut self.local, Scoped::new(self.target, 0))
                .unwrap()
                .value(),
            Some(&1)
        );
        run.finish(LocalClosure::Stable).unwrap();
    }
    fn stale(&mut self, which: usize) -> bool {
        let entry = [&self.entry, &self.downstream_entry, &self.unrelated_entry][which];
        self.kernel
            .generation_stale(entry, &mut self.meter)
            .unwrap()
    }
}
