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

#[test]
fn reachable_view_allocation_is_prepaid_and_keeps_the_anchor_detached() {
    use super::{LocalInput, RefinementError, apply_refinements};
    use crate::{
        analysis::{MachineError, machine::fixtures},
        domain::{Domain, NumericValue, ValueAlgebra},
        resource::WorkBudget,
    };

    let domain = Domain::default();
    let algebra = ValueAlgebra::new(domain);
    let anchor = fixtures::payload(domain);
    let mut budget = WorkBudget::new(usize::MAX);
    let result = apply_refinements(
        anchor.clone(),
        &StackFacts::none(),
        algebra.borrowed(),
        &mut budget,
    )
    .unwrap();
    let LocalInput::Reachable { payload, limited } = result else {
        panic!("a ready Top guarantee must preserve the reachable input");
    };
    assert_eq!(*payload, anchor);
    assert!(limited.is_empty());
    assert!(std::mem::size_of::<LocalInput<NumericValue>>() <= 5 * std::mem::size_of::<usize>());
    let paid = budget.used();
    assert!(paid > 0, "the added allocation must be reserved first");
    let mut refused = WorkBudget::new(paid - 1);
    assert!(matches!(
        apply_refinements(
            anchor.clone(),
            &StackFacts::none(),
            algebra.borrowed(),
            &mut refused,
        ),
        Err(RefinementError::Stopped(MachineError::Work))
    ));
    assert_eq!(refused.used(), 0, "the new reservation is all-or-nothing");
    assert!(refused.exhausted());
    // Owning the executable view does not grant access to the ascending anchor.
    let mut moved = *payload;
    moved.active_mut().stack.push(fixtures::word(7));
    assert!(anchor.active().stack.is_empty());
}

#[test]
fn unreachable_needs_no_view_allocation_and_cancelled_reachable_stays_an_error() {
    use super::{LocalInput, RefinementError, apply_refinements};
    use crate::{
        analysis::{MachineError, control::Control, machine::fixtures},
        domain::{Domain, ValueAlgebra, relational::QueryReason},
        resource::WorkBudget,
    };

    let domain = Domain::default();
    let algebra = ValueAlgebra::new(domain);
    let mut budget = WorkBudget::new(0);
    assert!(matches!(
        apply_refinements(
            fixtures::payload(domain),
            &StackFacts::Unreachable,
            algebra.borrowed(),
            &mut budget,
        ),
        Ok(LocalInput::Unreachable)
    ));
    assert_eq!(budget.used(), 0);
    assert!(!budget.exhausted());

    let payload = fixtures::payload(domain);
    let control = Control::new(crate::analysis::progress::Observer::default());
    control.cancellation().cancel();
    control.scope(|| {
        let mut budget = WorkBudget::new(usize::MAX);
        assert!(matches!(
            apply_refinements(
                payload,
                &StackFacts::none(),
                algebra.borrowed(),
                &mut budget
            ),
            Err(RefinementError::Stopped(MachineError::Relations(
                QueryReason::Cancelled
            )))
        ));
        assert_eq!(budget.used(), 0);
    });
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
