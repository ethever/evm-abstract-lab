use super::{Budget, Stop, scope};
use crate::{
    Accepts, Analyzer, Availability, Capability, Change, Closure, Cons, Context, Descending,
    Kernel, KernelError, LocalClosure, Nil, PublishError, Reduction, RefinePolicy, Registered,
    Scope, Scoped,
};
use std::convert::Infallible;

struct Members;
impl Capability for Members {
    type Key = ();
    type Value = u8;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &u8) -> usize {
        1
    }
}
struct Intersection;
impl RefinePolicy for Intersection {
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
struct Task {
    scope: Scope,
    universe: u8,
}
struct Initialize;
impl<Cx: Accepts<Members>> Reduction<Cx> for Initialize {
    type Task = Task;
    type Error = <Cx as Accepts<Members>>::Error;
    fn reduce(&mut self, task: &Task, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.accept(Scoped::new(task.scope, ()), task.universe)?;
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn enlarged_input_is_independently_seeded_in_a_fresh_descending_generation() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 1, 0);
    let old = scope(&mut kernel, &mut meter, 2, 0);
    let new = scope(&mut kernel, &mut meter, 2, 1);
    let mut entry = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let mut stores = Cons::new(
        kernel
            .channel::<Members, _, _>(Descending(Intersection), &mut meter)
            .unwrap(),
        Nil,
    );
    let coordinator = kernel
        .register(
            owner,
            Task {
                scope: owner,
                universe: 0,
            },
            &mut meter,
        )
        .unwrap();
    let reader = kernel
        .register(
            owner,
            Task {
                scope: owner,
                universe: 0,
            },
            &mut meter,
        )
        .unwrap();
    let a = kernel
        .register(
            old,
            Task {
                scope: old,
                universe: 0b01,
            },
            &mut meter,
        )
        .unwrap();
    let b = kernel
        .register(
            new,
            Task {
                scope: new,
                universe: 0b11,
            },
            &mut meter,
        )
        .unwrap();
    let mut init = Registered::new(Initialize);
    for (job, generation, universe) in [(a, old, 0b01), (b, new, 0b11)] {
        let mut run = kernel.begin(job, &mut meter).unwrap();
        let task = Task {
            scope: generation,
            universe,
        };
        init.evaluate(&task, &mut Context::new(&mut run, &mut stores))
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        kernel.seal_scope(generation, &mut meter).unwrap();
    }
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let generation = *run.current_generation(&entry).unwrap().value().unwrap();
    assert_eq!(
        run.query(&mut stores.head, Scoped::new(generation, ()))
            .unwrap()
            .value(),
        Some(&0b01)
    );
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(coordinator, &mut meter).unwrap();
    assert_eq!(
        run.replace_generation(&mut entry, new).unwrap(),
        Change::Changed
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(kernel.worklist().contains(&reader));
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let generation = *run.current_generation(&entry).unwrap().value().unwrap();
    assert_eq!(generation, new);
    assert_eq!(
        run.query(&mut stores.head, Scoped::new(generation, ()))
            .unwrap()
            .value(),
        Some(&0b11),
        "the old narrowed guarantee is not met into the enlarged input"
    );
    run.finish(LocalClosure::Stable).unwrap();
    kernel.seal_scope(owner, &mut meter).unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
}

#[test]
fn failed_registered_initialization_leaves_pending_facts_and_interruption() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let s = scope(&mut kernel, &mut meter, 1, 0);
    let mut stores = Cons::new(
        kernel
            .channel::<Members, _, _>(Descending(Intersection), &mut meter)
            .unwrap(),
        Nil,
    );
    let init_job = kernel
        .register(
            s,
            Task {
                scope: s,
                universe: 0b11,
            },
            &mut meter,
        )
        .unwrap();
    let reader = kernel
        .register(
            s,
            Task {
                scope: s,
                universe: 0,
            },
            &mut meter,
        )
        .unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut stores.head, Scoped::new(s, ()))
            .unwrap()
            .state,
        Availability::Pending
    );
    run.finish(LocalClosure::Stable).unwrap();
    kernel.seal_scope(s, &mut meter).unwrap();
    let mut run = kernel.begin(init_job, &mut meter).unwrap();
    assert_eq!(
        run.publish(&mut stores.head, Scoped::new(s, ()), 0b01),
        Err(PublishError::Kernel(KernelError::UnregisteredReduction))
    );
    run.meter().fail_on = Some(run.meter().calls + 2); // lookup, then first-publication fanout
    let result = Registered::new(Initialize).evaluate(
        &Task {
            scope: s,
            universe: 0b11,
        },
        &mut Context::new(&mut run, &mut stores),
    );
    assert_eq!(
        result,
        Err(PublishError::Kernel(KernelError::Stopped(Stop::Work)))
    );
    drop(run);
    assert!(matches!(kernel.closure(), Closure::Open(o) if o.interrupted == 1 && o.missing == 1));
    assert!(!kernel.worklist().contains(&reader));
}
