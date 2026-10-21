//! Real Seed wake/replay over intentionally delayed native source publication.
use super::*;
use crate::analysis::{
    cooperation::{self as public, Input, LocalInput, PointKind, Ports},
    engine::cooperation::{Binding, Stamp},
    worklist::{
        MachineMeter,
        runtime::{Task, TaskKind},
    },
};
use analysis_kernel::{Availability, GenerationEntry, JobId, LocalClosure, ScopeKey};

struct Pending {
    h: Inventory,
    parent: Subject,
    child: Subject,
    parent_seed: JobId,
    child_seed: JobId,
    entry: GenerationEntry,
}
impl Pending {
    fn new() -> Self {
        let mut h = Inventory::new();
        let mut owner = h.begin();
        let mut subjects = Vec::new();
        for (context, namespace) in [(900, 1), (901, 2)] {
            let mut meter = MachineMeter(&mut h.budget);
            let scope = h
                .runtime
                .kernel()
                .scope(
                    ScopeKey {
                        context,
                        generation: 1,
                        policy: 0,
                    },
                    &mut meter,
                )
                .unwrap();
            let subject = Subject {
                point: Point {
                    namespace,
                    kind: PointKind::Body,
                    index: 9,
                },
                scope,
            };
            let seed = h
                .runtime
                .kernel()
                .register(scope, Task(TaskKind::Seed(subject)), &mut meter)
                .unwrap();
            h.runtime.kernel().seal_scope(scope, &mut meter).unwrap();
            let relative = h.runtime.relative;
            let entry = h
                .runtime
                .kernel()
                .generation_entry(relative, scope, &mut meter)
                .unwrap();
            subjects.push((subject, seed, entry));
        }
        let (parent, parent_seed, parent_entry) = subjects.remove(0);
        let (child, child_seed, entry) = subjects.remove(0);
        let epoch = h.cooperation.owner_epochs[&h.owner];
        for (subject, seed, selected, source) in [
            (parent, parent_seed, parent_entry, None),
            (child, child_seed, entry.clone(), Some(parent)),
        ] {
            let mut meter = MachineMeter(&mut h.budget);
            let mut run = h.runtime.kernel().begin(seed, &mut meter).unwrap();
            let anchor = run.observe_slot(&h.anchor).unwrap();
            if source.is_none() {
                run.observe_slot(&h.assumption).unwrap();
            } else {
                assert!(matches!(
                    run.query(
                        &mut h.cooperation.shared.as_mut().unwrap().inputs,
                        Scoped::new(parent.scope, ())
                    )
                    .unwrap()
                    .state,
                    Availability::Pending
                ));
            }
            // A real publisher retains native source premises before its value
            // is available; Pending acquires no parent guarantee/head.
            run.finish(LocalClosure::Stable).unwrap();
            h.cooperation.bindings.insert(
                subject.point,
                Binding {
                    stamp: Stamp {
                        anchor,
                        phase: 0,
                        secondary: None,
                        parent: source,
                    },
                    input: Input::new(subject, h.payload.clone()).in_universe(h.runtime.world),
                    entry: selected,
                    producer: Some((h.owner, epoch)),
                    consumer: h.owner,
                },
            );
            h.cooperation.scope_owners.insert(subject.scope, h.owner);
        }
        let stores = h
            .cooperation
            .assembly
            .create(h.runtime.kernel(), &mut MachineMeter(&mut h.budget))
            .unwrap();
        h.cooperation.stores.insert(child.scope, stores);
        h.cooperation
            .owner_points
            .entry(h.owner)
            .or_default()
            .insert(child.point);
        {
            let mut meter = MachineMeter(&mut h.budget);
            let mut run = h.runtime.kernel().resume(&mut owner, &mut meter).unwrap();
            run.current_generation(&entry).unwrap();
            assert!(matches!(
                run.query(
                    &mut h.cooperation.shared.as_mut().unwrap().inputs,
                    Scoped::new(child.scope, ())
                )
                .unwrap()
                .state,
                Availability::Pending
            ));
            owner = run.suspend();
        }
        h.cooperation
            .defer_owner(h.owner, &mut h.runtime, &mut h.budget)
            .unwrap();
        h.runtime.finish(owner, &mut h.budget).unwrap();
        Self {
            h,
            parent,
            child,
            parent_seed,
            child_seed,
            entry,
        }
    }
    fn publish_parent(&mut self) {
        let mut meter = MachineMeter(&mut self.h.budget);
        let mut run = self
            .h
            .runtime
            .kernel()
            .begin(self.parent_seed, &mut meter)
            .unwrap();
        run.observe_slot(&self.h.assumption).unwrap();
        let input = self.h.cooperation.bindings[&self.parent.point]
            .input
            .clone();
        let mut ports = Ports {
            shared: self.h.cooperation.shared.as_mut().unwrap(),
            local: &mut analysis_kernel::Nil,
            global: self.h.cooperation.global.as_mut().unwrap(),
        };
        public::initialize(&mut run, &mut ports, input).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    fn drain(&mut self) {
        while let Some((task, token)) = self.h.runtime.next_external(&mut self.h.budget).unwrap() {
            assert!(matches!(task.0, TaskKind::Seed(subject) if subject == self.child));
            self.h
                .cooperation
                .evaluate(&mut self.h.runtime, task, token, &mut self.h.budget)
                .unwrap();
        }
    }
    fn child_value(&mut self) -> Availability<Input<NumericValue>> {
        let mut meter = MachineMeter(&mut self.h.budget);
        let mut run = self
            .h
            .runtime
            .kernel()
            .begin(self.h.owner, &mut meter)
            .unwrap();
        let answer = run
            .query(
                &mut self.h.cooperation.shared.as_mut().unwrap().inputs,
                Scoped::new(self.child.scope, ()),
            )
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        answer.state
    }
}

#[test]
fn pending_parent_availability_replays_seed_and_finishes_native_obligation() {
    let mut p = Pending::new();
    assert!(matches!(p.child_value(), Availability::Pending));
    assert_ne!(
        p.h.runtime.view().closure(),
        analysis_kernel::Closure::Closed
    );
    p.publish_parent();
    p.drain();
    assert!(matches!(p.child_value(), Availability::Value(_)));
    let mut meter = MachineMeter(&mut p.h.budget);
    let run = p.h.runtime.kernel().begin(p.h.owner, &mut meter).unwrap();
    let mut owner = run.suspend();
    assert!(
        p.h.cooperation
            .complete_native(&mut p.h.runtime, &mut owner, &mut p.h.budget)
            .unwrap()
    );
    p.h.runtime.finish(owner, &mut p.h.budget).unwrap();
    assert_eq!(
        p.h.runtime.view().closure(),
        analysis_kernel::Closure::Closed
    );
}

#[test]
fn retired_pending_parent_requires_fresh_owner_payload() {
    let mut p = Pending::new();
    let old_child = p.child;
    let weak = p.h.cooperation.bindings[&old_child.point].input.downgrade();
    let mut meter = MachineMeter(&mut p.h.budget);
    let scope =
        p.h.runtime
            .kernel()
            .scope(
                ScopeKey {
                    generation: 2,
                    ..p.parent.scope.key()
                },
                &mut meter,
            )
            .unwrap();
    let parent = Subject {
        point: p.parent.point,
        scope,
    };
    let seed =
        p.h.runtime
            .kernel()
            .register(scope, Task(TaskKind::Seed(parent)), &mut meter)
            .unwrap();
    p.h.runtime.kernel().seal_scope(scope, &mut meter).unwrap();
    let mut run = p.h.runtime.kernel().begin(p.h.owner, &mut meter).unwrap();
    run.replace_generation(
        &mut p
            .h
            .cooperation
            .bindings
            .get_mut(&parent.point)
            .unwrap()
            .entry,
        scope,
    )
    .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut payload = p.h.payload.clone();
    payload.active_mut().stack[0] = crate::analysis::machine::fixtures::word(42);
    let input = Input::new(parent, payload.clone()).in_universe(p.h.runtime.world);
    p.h.cooperation
        .bindings
        .get_mut(&parent.point)
        .unwrap()
        .input = input.clone();
    let mut run = p.h.runtime.kernel().begin(seed, &mut meter).unwrap();
    run.observe_slot(&p.h.assumption).unwrap();
    let mut ports = Ports {
        shared: p.h.cooperation.shared.as_mut().unwrap(),
        local: &mut analysis_kernel::Nil,
        global: p.h.cooperation.global.as_mut().unwrap(),
    };
    public::initialize(&mut run, &mut ports, input).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(
        !p.h.runtime
            .view()
            .scope_stale(old_child.scope, &mut MachineMeter(&mut p.h.budget))
            .unwrap(),
        "Pending adopted no obsolete guarantee"
    );
    p.drain();
    assert!(matches!(p.child_value(), Availability::Pending));
    assert_eq!(p.h.cooperation.subject(old_child.point), Some(old_child));
    assert!(p.h.cooperation.deferred.contains_key(&p.h.owner));
    assert_ne!(
        p.h.runtime.view().closure(),
        analysis_kernel::Closure::Closed
    );
    // Actual native owner executes from its fresh receipt. The anchor version
    // stays equal; carrying the previous derived payload would return 0.
    for _ in 0..2 {
        p.h.runtime
            .kernel()
            .retry(p.h.owner, &mut MachineMeter(&mut p.h.budget))
            .unwrap();
        let (task, mut owner) = p.h.runtime.next(true, &mut p.h.budget).unwrap().unwrap();
        assert!(matches!(task.0, TaskKind::Body(0)));
        p.h.cooperation
            .begin_owner(&mut p.h.runtime, &mut owner, &mut p.h.budget)
            .unwrap();
        let anchor = p.h.cooperation.bindings[&old_child.point].stamp.anchor;
        let view =
            p.h.cooperation
                .input(
                    old_child.point,
                    anchor,
                    0,
                    None,
                    Some(parent),
                    payload.clone(),
                    p.h.algebra.borrowed(),
                    &mut p.h.runtime,
                    &mut owner,
                    &mut p.h.budget,
                    |run| {
                        assert_eq!(run.observe_slot(&p.h.anchor)?, anchor);
                        Ok(())
                    },
                )
                .unwrap();
        let LocalInput::Reachable {
            payload: actual, ..
        } = view
        else {
            panic!("{view:?}")
        };
        assert_eq!(actual.active().stack[0].singleton(), Some(U256::from(42)));
        assert!(
            p.h.cooperation
                .complete_native(&mut p.h.runtime, &mut owner, &mut p.h.budget)
                .unwrap()
        );
        p.h.runtime.finish(owner, &mut p.h.budget).unwrap();
    }
    assert_ne!(p.h.cooperation.subject(old_child.point), Some(old_child));
    assert_eq!(p.h.cooperation.bindings[&old_child.point].entry, p.entry);
    assert_eq!(weak.strong_count(), 0);
    assert_eq!(
        p.h.runtime.view().closure(),
        analysis_kernel::Closure::Closed
    );
}

#[test]
fn seed_replay_work_or_cancel_retains_pending_input_and_obligations() {
    use crate::analysis::{control::Control, progress::Observer};
    for cancel in [false, true] {
        let mut p = Pending::new();
        p.publish_parent();
        let (task, token) = p.h.runtime.next_external(&mut p.h.budget).unwrap().unwrap();
        assert!(matches!(task.0, TaskKind::Seed(subject) if subject == p.child));
        let control = Control::new(Observer::default());
        let result = if cancel {
            control.cancellation().cancel();
            control.scope(|| {
                p.h.cooperation
                    .evaluate(&mut p.h.runtime, task, token, &mut p.h.budget)
            })
        } else {
            let mut empty = crate::resource::WorkBudget::new(0);
            p.h.cooperation
                .evaluate(&mut p.h.runtime, task, token, &mut empty)
        };
        assert!(matches!(
            result,
            Err(public::CooperationError::Protocol(KernelError::Stopped(_)))
        ));
        assert!(matches!(p.child_value(), Availability::Pending));
        assert!(p.h.cooperation.deferred.contains_key(&p.h.owner));
        assert_ne!(
            p.h.runtime.view().closure(),
            analysis_kernel::Closure::Closed
        );
        assert!(
            p.h.runtime
                .view()
                .task::<crate::analysis::MachineError>(p.child_seed)
                .is_ok()
        );
    }
}

struct Narrow;
impl<Cx> analysis_kernel::Reduction<Cx> for Narrow
where
    Cx: analysis_kernel::Accepts<public::Refinements, public::RefinementRoute>,
{
    type Task = Subject;
    type Error = Cx::Error;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.accept(
            Scoped::new(subject.scope, ()),
            public::StackFacts::at(0, crate::domain::WordObservation::constant(U256::ZERO)),
        )?;
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn partial_seed_replay_finishes_feedback_without_resetting_later_narrowing() {
    let mut p = Pending::new();
    p.publish_parent();
    {
        // Interrupt initialization precisely between its two independently
        // atomic publications, with the genuine native/parent premises adopted.
        let mut meter = MachineMeter(&mut p.h.budget);
        let mut run =
            p.h.runtime
                .kernel()
                .begin(p.child_seed, &mut meter)
                .unwrap();
        run.query(
            &mut p.h.cooperation.shared.as_mut().unwrap().inputs,
            Scoped::new(p.parent.scope, ()),
        )
        .unwrap();
        public::initialize_readonly(
            &mut run,
            p.h.cooperation.shared.as_mut().unwrap(),
            &p.h.cooperation.bindings[&p.child.point].input,
        )
        .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    p.h.runtime
        .kernel()
        .retry(p.child_seed, &mut MachineMeter(&mut p.h.budget))
        .unwrap();
    p.drain();
    assert!(matches!(p.child_value(), Availability::Value(_)));
    {
        let mut meter = MachineMeter(&mut p.h.budget);
        let mut run =
            p.h.runtime
                .kernel()
                .begin(p.child_seed, &mut meter)
                .unwrap();
        let mut ports = Ports {
            shared: p.h.cooperation.shared.as_mut().unwrap(),
            local: p.h.cooperation.stores.get_mut(&p.child.scope).unwrap(),
            global: p.h.cooperation.global.as_mut().unwrap(),
        };
        let mut cx = analysis_kernel::Context::new(&mut run, &mut ports);
        analysis_kernel::Analyzer::evaluate(
            &mut analysis_kernel::Registered::new(Narrow),
            &p.child,
            &mut cx,
        )
        .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    // The Seed itself read Refinements; narrowing queues it, and replay must
    // retain that fact rather than republish the initializer's coarse Top.
    assert!(
        p.h.runtime
            .view()
            .worklist()
            .pending()
            .any(|job| job == p.child_seed)
    );
    p.drain();
    let mut meter = MachineMeter(&mut p.h.budget);
    let mut run =
        p.h.runtime
            .kernel()
            .begin(p.child_seed, &mut meter)
            .unwrap();
    let facts = run
        .query(
            &mut p.h.cooperation.shared.as_mut().unwrap().refinements,
            Scoped::new(p.child.scope, ()),
        )
        .unwrap();
    assert_eq!(
        facts.value(),
        Some(&public::StackFacts::at(
            0,
            crate::domain::WordObservation::constant(U256::ZERO)
        ))
    );
    run.finish(LocalClosure::Stable).unwrap();
}
