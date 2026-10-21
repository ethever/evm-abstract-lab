use super::support::{Budget, Stop};
use analysis_kernel::{
    Accepts, Ascending, Capability, Closure, Cons, Context, Descending, Either, Here, JoinPolicy,
    Kernel, KernelError, LocalClosure, Nil, Pair, PublishError, Read, Reduction, RefinePolicy,
    Registered, Scope, ScopeKey, Scoped, There, solve,
};
use std::{cell::RefCell, fmt, rc::Rc};

#[path = "bounds.rs"]
mod bounds;
#[path = "masks.rs"]
mod masks;

type Trace = Rc<RefCell<Vec<(&'static str, u16, u16)>>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mask {
    ones: u8,
    zeros: u8,
}
struct MaskFacts;
impl Capability for MaskFacts {
    type Key = ();
    type Value = Mask;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &Mask) -> usize {
        2
    }
}
struct BoundsFacts;
impl Capability for BoundsFacts {
    type Key = ();
    type Value = (u8, u8);
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &(u8, u8)) -> usize {
        2
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contradiction;
impl fmt::Display for Contradiction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("inconsistent local guarantees")
    }
}
impl std::error::Error for Contradiction {}
struct MaskMeet;
impl RefinePolicy for MaskMeet {
    type Value = Mask;
    type Error = Contradiction;
    fn work(&self, _: &Mask, _: &Mask) -> usize {
        4
    }
    fn refine(&self, a: &Mask, b: &Mask) -> Result<Mask, Contradiction> {
        let mask = Mask {
            ones: a.ones | b.ones,
            zeros: a.zeros | b.zeros,
        };
        if mask.ones & mask.zeros == 0 {
            Ok(mask)
        } else {
            Err(Contradiction)
        }
    }
    fn equivalent(&self, a: &Mask, b: &Mask) -> bool {
        a == b
    }
}
struct BoundsMeet;
impl RefinePolicy for BoundsMeet {
    type Value = (u8, u8);
    type Error = Contradiction;
    fn work(&self, _: &(u8, u8), _: &(u8, u8)) -> usize {
        4
    }
    fn refine(&self, a: &(u8, u8), b: &(u8, u8)) -> Result<Self::Value, Contradiction> {
        let result = (a.0.max(b.0), a.1.min(b.1));
        if result.0 <= result.1 {
            Ok(result)
        } else {
            Err(Contradiction)
        }
    }
    fn equivalent(&self, a: &Self::Value, b: &Self::Value) -> bool {
        a == b
    }
}
struct Seed;
struct SeedTask(Scope);
impl<Cx> Reduction<Cx> for Seed
where
    Cx: Accepts<MaskFacts, Here, Error = PublishError<Contradiction, Stop>>
        + Accepts<BoundsFacts, There<Here>, Error = PublishError<Contradiction, Stop>>,
{
    type Task = SeedTask;
    type Error = PublishError<Contradiction, Stop>;
    fn reduce(&mut self, task: &SeedTask, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        <Cx as Accepts<MaskFacts, Here>>::accept(
            cx,
            Scoped::new(task.0, ()),
            Mask { ones: 1, zeros: 0 },
        )?;
        <Cx as Accepts<BoundsFacts, There<Here>>>::accept(cx, Scoped::new(task.0, ()), (2, 4))?;
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn independent_typed_analyzers_jointly_refine_a_cycle_in_every_initial_order() {
    for order in [[0, 1, 2], [1, 2, 0], [2, 0, 1]] {
        let mut meter = Budget::default();
        let mut kernel = Kernel::new().unwrap();
        let scope = kernel
            .scope(
                ScopeKey {
                    context: 9,
                    generation: 4,
                    policy: 2,
                },
                &mut meter,
            )
            .unwrap();
        let mut stores = Cons::new(
            kernel
                .channel::<MaskFacts, _, _>(Descending(MaskMeet), &mut meter)
                .unwrap(),
            Cons::new(
                kernel
                    .channel::<BoundsFacts, _, _>(Descending(BoundsMeet), &mut meter)
                    .unwrap(),
                Nil,
            ),
        );
        let trace = Trace::default();
        let mut components = Pair::new(
            Registered::new(Seed),
            Pair::new(
                Registered::new(masks::Masks {
                    trace: Rc::clone(&trace),
                }),
                Registered::new(bounds::Bounds {
                    trace: Rc::clone(&trace),
                }),
            ),
        );
        for task in order {
            let task = match task {
                0 => Either::Left(SeedTask(scope)),
                1 => Either::Right(Either::Left(masks::Task(scope))),
                _ => Either::Right(Either::Right(bounds::Task(scope))),
            };
            kernel.register(scope, task, &mut meter).unwrap();
        }
        kernel.seal_scope(scope, &mut meter).unwrap();
        let report = solve(&mut kernel, &mut components, &mut stores, &mut meter, 100).unwrap();
        assert_eq!(report.closure, Closure::Closed);
        assert!(
            report.steps > 3,
            "a single pass cannot close the dependency cycle"
        );
        assert!(
            trace
                .borrow()
                .iter()
                .filter(|row| row.0 == "bounds")
                .count()
                >= 2
        );
        assert!(trace.borrow().contains(&("masks", 3, 252)));
        let observer = kernel
            .register(scope, Either::Left(SeedTask(scope)), &mut meter)
            .unwrap();
        let mut run = kernel.begin(observer, &mut meter).unwrap();
        assert_eq!(
            run.query(&mut stores.head, Scoped::new(scope, ()))
                .unwrap()
                .value(),
            Some(&Mask {
                ones: 3,
                zeros: 252
            })
        );
        assert_eq!(
            run.query(&mut stores.tail.head, Scoped::new(scope, ()))
                .unwrap()
                .value(),
            Some(&(3, 3))
        );
        run.finish(LocalClosure::Stable).unwrap();
        assert_eq!(
            solve(&mut kernel, &mut components, &mut stores, &mut meter, 100)
                .unwrap()
                .steps,
            0
        );
        assert!(meter.used > kernel.dependency_count());
    }
}

// A second domain and partition choice: finite sets keyed by semantic sign class.
struct Sets;
impl Capability for Sets {
    type Key = (u8, bool);
    type Value = std::collections::BTreeSet<i16>;
    fn key_work(_: &Self::Key) -> usize {
        2
    }
    fn value_work(v: &Self::Value) -> usize {
        v.len()
    }
}
struct Union;
impl JoinPolicy for Union {
    type Value = std::collections::BTreeSet<i16>;
    type Error = std::convert::Infallible;
    fn work(&self, a: &Self::Value, b: &Self::Value) -> usize {
        (a.len() + b.len()) * 16 + 1
    }
    fn join(&self, a: &Self::Value, b: &Self::Value) -> Result<Self::Value, Self::Error> {
        Ok(a.union(b).copied().collect())
    }
    fn equivalent(&self, a: &Self::Value, b: &Self::Value) -> bool {
        a == b
    }
}

#[test]
fn another_carrier_and_partition_and_explicit_provider_routes_need_no_core_changes() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = kernel
        .scope(
            ScopeKey {
                context: 13,
                generation: 0,
                policy: 8,
            },
            &mut meter,
        )
        .unwrap();
    let mut stores = Cons::new(
        kernel
            .channel::<Sets, _, _>(Ascending(Union), &mut meter)
            .unwrap(),
        Cons::new(
            kernel
                .channel::<Sets, _, _>(Ascending(Union), &mut meter)
                .unwrap(),
            Nil,
        ),
    );
    let job = kernel.register(scope, (), &mut meter).unwrap();
    let mut run = kernel.begin(job, &mut meter).unwrap();
    let mut cx = Context::new(&mut run, &mut stores);
    <_ as Accepts<Sets, Here>>::accept(&mut cx, Scoped::new(scope, (2, false)), [-2, -1].into())
        .unwrap();
    <_ as Accepts<Sets, There<Here>>>::accept(&mut cx, Scoped::new(scope, (2, false)), [-7].into())
        .unwrap();
    let left = <_ as Read<Sets, Here>>::query(&mut cx, Scoped::new(scope, (2, false))).unwrap();
    let right =
        <_ as Read<Sets, There<Here>>>::query(&mut cx, Scoped::new(scope, (2, false))).unwrap();
    assert_eq!(left.value().unwrap(), &[-2, -1].into());
    assert_eq!(right.value().unwrap(), &[-7].into());
    run.finish(LocalClosure::Stable).unwrap();
}

#[test]
fn step_limits_and_analyzer_errors_retain_joint_obligations() {
    use analysis_kernel::{Analyzer, SolveError};
    struct Fails;
    impl<Cx> Analyzer<Cx> for Fails {
        type Task = ();
        type Error = Stop;
        fn evaluate(&mut self, _: &(), _: &mut Cx) -> Result<LocalClosure, Stop> {
            Err(Stop::Work)
        }
    }
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = kernel
        .scope(
            ScopeKey {
                context: 1,
                generation: 0,
                policy: 1,
            },
            &mut meter,
        )
        .unwrap();
    kernel.register(scope, (), &mut meter).unwrap();
    kernel.seal_scope(scope, &mut meter).unwrap();
    let bounded = solve(&mut kernel, &mut Fails, &mut Nil, &mut meter, 0).unwrap();
    assert_eq!(bounded.steps, 0);
    assert!(matches!(bounded.closure, Closure::Open(open) if open.pending == 1));
    assert_eq!(
        solve(&mut kernel, &mut Fails, &mut Nil, &mut meter, 1),
        Err(SolveError::Analyzer(Stop::Work))
    );
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.interrupted == 1));
}
