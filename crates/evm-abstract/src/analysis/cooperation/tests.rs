use super::{
    InputTasks, NoTasks, PairedTasks, PerInput, Point, PointKind, StackFacts, StackMeet, Subject,
};
use crate::{U256, domain::WordObservation};
use analysis_kernel::{Either, Kernel, Meter, RefinePolicy, ScopeKey};
use std::convert::Infallible;

#[test]
fn scoped_stack_facts_accumulate_without_retracting_other_slots() {
    let lower = StackFacts::at(
        0,
        WordObservation::unsigned_range(U256::from(2), U256::from(4)).unwrap(),
    );
    let exact = StackFacts::at(0, WordObservation::constant(U256::from(3)));
    let other = StackFacts::at(1, WordObservation::constant(U256::from(9)));
    let joined = StackMeet.refine(&lower, &other).unwrap();
    let refined = StackMeet.refine(&joined, &exact).unwrap();
    let StackFacts::Reachable(facts) = &refined else {
        panic!("compatible guarantees must remain reachable");
    };
    assert_eq!(facts[&0].singleton(), Some(U256::from(3)));
    assert_eq!(facts[&1].singleton(), Some(U256::from(9)));
    assert!(StackMeet.equivalent(&refined, &StackMeet.refine(&refined, &exact).unwrap()));
    assert_eq!(refined, StackMeet.refine(&exact, &joined).unwrap());
}

#[test]
fn contradictory_guarantees_are_explicit_unreachable_and_top_is_ready() {
    let left = StackFacts::at(0, WordObservation::constant(U256::from(3)));
    let right = StackFacts::at(0, WordObservation::constant(U256::from(4)));
    assert_eq!(
        StackMeet.refine(&left, &right).unwrap(),
        StackFacts::Unreachable
    );
    assert_eq!(StackMeet.refine(&left, &StackFacts::none()).unwrap(), left);
    assert_eq!(
        StackFacts::at(0, WordObservation::top()),
        StackFacts::none()
    );
}

struct TestMeter;

impl Meter for TestMeter {
    type Stop = Infallible;

    fn reserve(&mut self, _: usize) -> Result<(), Infallible> {
        Ok(())
    }

    fn checkpoint(&self) -> Result<(), Infallible> {
        Ok(())
    }
}

#[test]
fn static_pair_routes_create_distinct_component_jobs_for_the_same_universe() {
    let mut kernel = Kernel::<()>::new().unwrap();
    let scope = kernel
        .scope(
            ScopeKey {
                context: 17,
                generation: 2,
                policy: 1,
            },
            &mut TestMeter,
        )
        .unwrap();
    let subject = Subject {
        point: Point {
            namespace: 7,
            kind: PointKind::Body,
            index: 4,
        },
        scope,
    };
    let tasks = PairedTasks::new(PerInput, PerInput);
    assert_eq!(
        tasks.tasks(subject).collect::<Vec<_>>(),
        vec![Either::Left(subject), Either::Right(subject)]
    );
    assert!(tasks.registration_work() >= 2);
    assert_eq!(NoTasks.tasks(subject).count(), 0);
}

#[test]
fn input_ports_do_not_expose_another_generations_premises() {
    use super::{Input, create_ports, initialize};
    use crate::{
        analysis::machine::fixtures,
        domain::{Domain, NumericValue},
    };
    use analysis_kernel::{Availability, LocalClosure, Scoped};

    let mut kernel = Kernel::<u8>::new().unwrap();
    let mut meter = TestMeter;
    let mut inputs = Vec::new();
    for generation in 1..=2 {
        let scope = kernel
            .scope(
                ScopeKey {
                    context: 31,
                    generation,
                    policy: 1,
                },
                &mut meter,
            )
            .unwrap();
        let subject = Subject {
            point: Point {
                namespace: 0,
                kind: PointKind::World,
                index: 0,
            },
            scope,
        };
        let mut ports =
            create_ports::<NumericValue, _, _, _>(&mut kernel, &mut meter, &super::NoStores)
                .unwrap();
        let job = kernel.register(scope, 0, &mut meter).unwrap();
        let mut run = kernel.begin(job, &mut meter).unwrap();
        initialize(
            &mut run,
            &mut ports,
            Input::new(subject, fixtures::payload(Domain::default())),
        )
        .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        kernel.seal_scope(scope, &mut meter).unwrap();
        inputs.push((subject, ports));
    }
    let first = inputs[0].0;
    let second = inputs[1].0;
    let job = kernel.register(first.scope, 1, &mut meter).unwrap();
    let mut run = kernel.begin(job, &mut meter).unwrap();
    // The other generation really is seeded, but it is not a provider route
    // exposed by this fixed universe's public context.
    assert!(matches!(
        run.query(&mut inputs[1].1.head, Scoped::new(second.scope, ()))
            .unwrap()
            .state,
        Availability::Value(_)
    ));
    let foreign = run
        .query(&mut inputs[0].1.head, Scoped::new(second.scope, ()))
        .unwrap();
    assert!(matches!(foreign.state, Availability::Pending));
    run.finish(LocalClosure::Stable).unwrap();
    assert_ne!(kernel.closure(), analysis_kernel::Closure::Closed);
}
