//! Independent prerequisite witnesses, using a real observation owner and a
//! fake native revision slot. Actual EVM entry navigation is tested at hookup.
use super::super::Cooperation;
use crate::{
    analysis::{
        EvmMachinePayload,
        cooperation::{
            Extension, ExtraStores, Input, InputTasks, PointKind, ProcedureId, Subject,
            current_input,
        },
        machine::fixtures,
        worklist::{
            MachineMeter,
            runtime::{Runtime, Task, TaskKind},
        },
    },
    domain::{NumericValue, ProductAlgebra},
    resource::WorkBudget,
};
use analysis_kernel::{
    Analyzer, Ascending, Capability, Change, Channel, Context, GenerationEntry, Here, JoinPolicy,
    Kernel, KernelError, LocalClosure, Meter, Nil, ReclaimScope, RunningJob, Scope, ScopeKey,
    Scoped, Select, SlotMeta,
};
use std::{cell::Cell, convert::Infallible, rc::Rc};

use crate::domain::EvmNumericAlgebra as _;
struct NoClient;
impl<Cx> Analyzer<Cx> for NoClient {
    type Task = Subject;
    type Error = Infallible;
    fn evaluate(&mut self, _: &Subject, _: &mut Cx) -> Result<LocalClosure, Infallible> {
        panic!("read-only entry must not register a component task")
    }
}
struct Tasks(Rc<Cell<usize>>);
impl InputTasks for Tasks {
    type Task = Subject;
    const ENABLED: bool = true;
    fn registration_work(&self) -> usize {
        1
    }
    fn tasks(&self, subject: Subject) -> impl Iterator<Item = Subject> {
        self.0.set(self.0.get() + 1);
        std::iter::once(subject)
    }
}
struct Local(Rc<Cell<usize>>);
impl ExtraStores for Local {
    type Stores = Nil;
    fn retirement_work(&self, _: &Nil) -> usize {
        0
    }
    fn create<J, M: Meter>(
        &self,
        _: &mut Kernel<J>,
        _: &mut M,
    ) -> Result<Nil, KernelError<M::Stop>> {
        self.0.set(self.0.get() + 1);
        Ok(Nil)
    }
}
struct Held;
impl Capability for Held {
    type Key = ();
    type Value = Input<NumericValue>;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &Self::Value) -> usize {
        8
    }
    fn drop_work(value: &Self::Value) -> usize {
        value.work_size()
    }
}
struct Keep;
impl JoinPolicy for Keep {
    type Value = Input<NumericValue>;
    type Error = Infallible;
    fn work(&self, _: &Self::Value, _: &Self::Value) -> usize {
        8
    }
    fn join(&self, old: &Self::Value, _: &Self::Value) -> Result<Self::Value, Infallible> {
        Ok(old.clone())
    }
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool {
        left.subject() == right.subject()
    }
}
struct Shared {
    held: Channel<Held, Ascending<Keep>>,
    refuse: Rc<Cell<bool>>,
}
impl Select<Here> for Shared {
    type Output = Channel<Held, Ascending<Keep>>;
    fn select(&self) -> &Self::Output {
        &self.held
    }
    fn select_mut(&mut self) -> &mut Self::Output {
        &mut self.held
    }
}
impl ReclaimScope for Shared {
    fn reclaim_scope<J, M: Meter>(
        &mut self,
        kernel: &mut Kernel<J>,
        retired: Scope,
        meter: &mut M,
    ) -> Result<usize, KernelError<M::Stop>> {
        if self.refuse.get() {
            meter.reserve(usize::MAX).map_err(KernelError::Stopped)?;
        }
        self.held.reclaim_scope(kernel, retired, meter)
    }
}
struct Global(Rc<Cell<bool>>);
impl ExtraStores for Global {
    type Stores = Shared;
    fn retirement_work(&self, _: &Shared) -> usize {
        0
    }
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Shared, KernelError<M::Stop>> {
        Ok(Shared {
            held: kernel.channel(Ascending(Keep), meter)?,
            refuse: Rc::clone(&self.0),
        })
    }
}
type Assembly = Extension<NoClient, Tasks, Local, Global>;
struct Harness {
    cooperation: Cooperation<NumericValue, Assembly, Infallible>,
    runtime: Runtime<Subject>,
    budget: WorkBudget,
    source: SlotMeta<usize>,
    writer: analysis_kernel::JobId,
    reader: analysis_kernel::JobId,
    payload: EvmMachinePayload<NumericValue>,
    revision: usize,
    materializations: Rc<Cell<usize>>,
    tasks: Rc<Cell<usize>>,
    locals: Rc<Cell<usize>>,
    refuse: Rc<Cell<bool>>,
}
impl Harness {
    const ID: ProcedureId = ProcedureId {
        namespace: 1,
        index: 0,
    };
    fn new() -> Self {
        let mut budget = WorkBudget::new(1_000_000_000);
        let mut runtime = Runtime::new(&mut budget).unwrap();
        let relative = runtime.relative;
        let source = runtime
            .kernel()
            .slot(relative, &mut MachineMeter(&mut budget))
            .unwrap();
        let writer = runtime
            .register_relative(TaskKind::Body(0), &mut budget)
            .unwrap();
        runtime
            .kernel()
            .begin(writer, &mut MachineMeter(&mut budget))
            .unwrap()
            .finish(LocalClosure::Stable)
            .unwrap();
        let world = runtime.world;
        let reader = runtime
            .kernel()
            .register(
                world,
                Task(TaskKind::World {
                    node: usize::MAX,
                    depth: 0,
                }),
                &mut MachineMeter(&mut budget),
            )
            .unwrap();
        runtime
            .kernel()
            .begin(reader, &mut MachineMeter(&mut budget))
            .unwrap()
            .finish(LocalClosure::Stable)
            .unwrap();
        let tasks = Rc::new(Cell::new(0));
        let locals = Rc::new(Cell::new(0));
        let refuse = Rc::new(Cell::new(false));
        let extension = Extension::with_tasks(NoClient, Tasks(Rc::clone(&tasks)))
            .with_stores(Local(Rc::clone(&locals)))
            .with_shared_stores(Global(Rc::clone(&refuse)));
        let mut payload = fixtures::payload(ProductAlgebra::default());
        payload.active_mut().stack.push(fixtures::word(1));
        Self {
            cooperation: Cooperation::new(extension),
            runtime,
            budget,
            source,
            writer,
            reader,
            payload,
            revision: 1,
            materializations: Rc::new(Cell::new(0)),
            tasks,
            locals,
            refuse,
        }
    }
    fn producer(&mut self) -> RunningJob {
        self.runtime.schedule_observation(&mut self.budget).unwrap();
        let (task, token) = self.runtime.next(false, &mut self.budget).unwrap().unwrap();
        assert!(matches!(task.0, TaskKind::SummaryObservation));
        token
    }
    fn publish(
        &mut self,
    ) -> Result<Subject, crate::analysis::cooperation::CooperationError<Infallible>> {
        let mut producer = self.producer();
        let payload = &self.payload;
        let materializations = &self.materializations;
        let result = self.cooperation.publish_entry_input(
            Self::ID,
            self.revision,
            &self.source,
            &mut self.runtime,
            &mut producer,
            &mut self.budget,
            |budget| {
                MachineMeter(budget).reserve(payload.work_size().saturating_mul(4))?;
                materializations.set(materializations.get() + 1);
                Ok(payload.clone())
            },
        );
        if result.is_ok() {
            self.runtime.finish(producer, &mut self.budget).unwrap();
        }
        result
    }
    fn grow(&mut self) {
        let revision = self.revision + 1;
        self.runtime
            .kernel()
            .retry(self.writer, &mut MachineMeter(&mut self.budget))
            .unwrap();
        let mut meter = MachineMeter(&mut self.budget);
        let mut run = self
            .runtime
            .kernel()
            .begin(self.writer, &mut meter)
            .unwrap();
        let work = self.payload.work_size().saturating_mul(4);
        run.publish_prepared_with(&self.source, Change::Changed, work, || {
            self.revision = revision;
            self.payload.active_mut().stack[0] = fixtures::word(revision as u64);
        })
        .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    fn certificate(&mut self, closed: bool) {
        let mut producer = self.producer();
        let source = &self.source;
        self.cooperation
            .publish_summaries(
                vec![(
                    Self::ID,
                    crate::analysis::cooperation::Summary::new(
                        7,
                        std::collections::BTreeMap::new(),
                        8,
                    ),
                )],
                closed,
                &mut self.runtime,
                &mut producer,
                &mut self.budget,
                |run| {
                    run.observe_slot(source)?;
                    Ok(())
                },
            )
            .unwrap();
        self.runtime.finish(producer, &mut self.budget).unwrap();
    }
    fn summary(
        &mut self,
    ) -> analysis_kernel::Availability<
        std::sync::Arc<crate::analysis::cooperation::Summary<NumericValue>>,
    > {
        self.summary_answer().unwrap().state
    }
    fn summary_answer(
        &mut self,
    ) -> crate::analysis::cooperation::ClosedSummaryResult<
        NumericValue,
        KernelError<crate::analysis::MachineError>,
    > {
        self.runtime
            .kernel()
            .retry(self.reader, &mut MachineMeter(&mut self.budget))
            .unwrap();
        let mut meter = MachineMeter(&mut self.budget);
        let world = self.runtime.world;
        let mut run = self
            .runtime
            .kernel()
            .begin(self.reader, &mut meter)
            .unwrap();
        let mut ports = crate::analysis::cooperation::Ports {
            shared: self.cooperation.shared.as_mut().unwrap(),
            local: &mut Nil,
            global: self.cooperation.global.as_mut().unwrap(),
        };
        let mut cx = Context::new(&mut run, &mut ports);
        let answer = crate::analysis::cooperation::closed_summary::<NumericValue, _>(
            &mut cx,
            world,
            Self::ID,
        );
        run.finish(LocalClosure::Stable).unwrap();
        answer
    }
    fn snapshot(
        &mut self,
    ) -> analysis_kernel::Availability<
        std::sync::Arc<crate::analysis::cooperation::Summary<NumericValue>>,
    > {
        let world = self.runtime.world;
        let mut meter = MachineMeter(&mut self.budget);
        let mut run = self
            .runtime
            .kernel()
            .begin(self.reader, &mut meter)
            .unwrap();
        let answer = run
            .query(
                &mut self.cooperation.shared.as_mut().unwrap().summaries,
                Scoped::new(world, Self::ID),
            )
            .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        answer.state
    }
    fn reader(&mut self, subject: Subject) -> GenerationEntry {
        let scope = self
            .runtime
            .kernel()
            .scope(
                ScopeKey {
                    context: 900,
                    generation: self.revision as u64,
                    policy: 0,
                },
                &mut MachineMeter(&mut self.budget),
            )
            .unwrap();
        let job = self
            .runtime
            .kernel()
            .register(
                scope,
                Task(TaskKind::Extension(Subject {
                    point: crate::analysis::cooperation::Point {
                        namespace: 0,
                        kind: PointKind::World,
                        index: 0,
                    },
                    scope,
                })),
                &mut MachineMeter(&mut self.budget),
            )
            .unwrap();
        self.runtime
            .kernel()
            .seal_scope(scope, &mut MachineMeter(&mut self.budget))
            .unwrap();
        let world = self.runtime.world;
        let entry = self
            .runtime
            .kernel()
            .generation_entry(world, scope, &mut MachineMeter(&mut self.budget))
            .unwrap();
        let mut meter = MachineMeter(&mut self.budget);
        let mut run = self.runtime.kernel().begin(job, &mut meter).unwrap();
        let mut ports = crate::analysis::cooperation::Ports {
            shared: self.cooperation.shared.as_mut().unwrap(),
            local: &mut Nil,
            global: self.cooperation.global.as_mut().unwrap(),
        };
        let input = current_input::<NumericValue, _>(
            &mut Context::new(&mut run, &mut ports),
            world,
            subject.point,
        )
        .unwrap();
        let input = input.value().unwrap();
        assert_eq!(input.subject(), subject);
        run.publish(
            &mut self.cooperation.global.as_mut().unwrap().held,
            Scoped::new(subject.scope, ()),
            input.clone(),
        )
        .unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        entry
    }
}

#[test]
fn readonly_seed_replay_keeps_input_only_and_the_same_source_generation() {
    let mut h = Harness::new();
    let subject = h.publish().unwrap();
    let entry = h.cooperation.readonly[&Harness::ID].entry.clone();
    let materializations = h.materializations.get();
    // Exercise the internal dispatch contract in an existing immutable entry
    // scope. Its cumulative native slot assumptions still guard every job here.
    let job = h
        .runtime
        .kernel()
        .register(
            subject.scope,
            Task(TaskKind::Seed(subject)),
            &mut MachineMeter(&mut h.budget),
        )
        .unwrap();
    let (task, token) = h.runtime.next_external(&mut h.budget).unwrap().unwrap();
    assert!(matches!(task.0, TaskKind::Seed(current) if current == subject));
    h.cooperation
        .evaluate(&mut h.runtime, task, token, &mut h.budget)
        .unwrap();
    assert!(
        h.runtime
            .view()
            .task::<crate::analysis::MachineError>(job)
            .is_ok()
    );
    assert_eq!(
        h.cooperation.readonly[&Harness::ID].input.subject(),
        subject
    );
    assert_eq!(h.cooperation.readonly[&Harness::ID].entry, entry);
    assert_eq!(h.materializations.get(), materializations);
    assert_eq!(h.tasks.get(), 0);
    assert_eq!(h.locals.get(), 0);
    assert!(h.cooperation.stores.is_empty());
    assert_eq!(h.cooperation.shared.as_ref().unwrap().refinements.len(), 0);
}

#[test]
fn readonly_entry_uses_stable_producer_input_only_and_reclaims_each_revision() {
    let mut h = Harness::new();
    let first = h.publish().unwrap();
    assert_eq!(first.point.kind, PointKind::ProcedureEntry);
    let selector = h.cooperation.readonly[&Harness::ID].entry.clone();
    let owner = h.cooperation.readonly[&Harness::ID].owner;
    let mut weak = h.cooperation.readonly[&Harness::ID].input.downgrade();
    let reader = h.reader(first);
    assert_eq!(h.publish().unwrap(), first);
    assert_eq!(h.materializations.get(), 1);
    h.grow();
    assert!(
        h.runtime
            .view()
            .generation_stale(&selector, &mut MachineMeter(&mut h.budget))
            .unwrap()
    );
    assert!(
        h.runtime
            .view()
            .generation_stale(&reader, &mut MachineMeter(&mut h.budget))
            .unwrap(),
        "tracked consumers lose the old entry guarantee before producer republication"
    );
    for revision in 2u64..10 {
        let current = h.publish().unwrap();
        assert_eq!(
            weak.strong_count(),
            0,
            "Inputs and client shared payload must release the retired snapshot during execution"
        );
        assert_ne!(current.scope, first.scope);
        assert_eq!(current.point, first.point);
        assert_eq!(h.cooperation.readonly[&Harness::ID].entry, selector);
        assert_eq!(h.cooperation.readonly[&Harness::ID].owner, owner);
        assert_eq!(
            h.cooperation.readonly[&Harness::ID]
                .input
                .payload()
                .active()
                .stack[0]
                .singleton(),
            Some(alloy_primitives::U256::from(revision))
        );
        assert_eq!(h.publish().unwrap(), current);
        assert_eq!(h.materializations.get(), revision as usize);
        assert_eq!(h.cooperation.shared.as_ref().unwrap().inputs.len(), 1);
        assert_eq!(h.cooperation.shared.as_ref().unwrap().refinements.len(), 0);
        assert!(h.cooperation.stores.is_empty());
        assert!(h.cooperation.bindings.is_empty());
        assert_eq!(h.tasks.get(), 0);
        assert_eq!(h.locals.get(), 0);
        assert_eq!(h.cooperation.global.as_ref().unwrap().held.len(), 0);
        weak = h.cooperation.readonly[&Harness::ID].input.downgrade();
        h.reader(current);
        h.grow();
    }
}

#[test]
fn readonly_entry_rejects_incidental_body_owner_and_retains_refused_reclamation() {
    let mut h = Harness::new();
    h.runtime
        .kernel()
        .retry(h.writer, &mut MachineMeter(&mut h.budget))
        .unwrap();
    let mut meter = MachineMeter(&mut h.budget);
    let mut caller = h
        .runtime
        .kernel()
        .begin(h.writer, &mut meter)
        .unwrap()
        .suspend();
    let result = h.cooperation.publish_entry_input(
        Harness::ID,
        h.revision,
        &h.source,
        &mut h.runtime,
        &mut caller,
        &mut h.budget,
        |_| panic!("wrong owner must not materialize"),
    );
    assert!(matches!(
        result,
        Err(crate::analysis::cooperation::CooperationError::Protocol(
            KernelError::InvalidJob
        ))
    ));
    assert!(h.cooperation.readonly.is_empty());
    h.runtime.finish(caller, &mut h.budget).unwrap();
    let first = h.publish().unwrap();
    h.reader(first);
    let weak = h.cooperation.readonly[&Harness::ID].input.downgrade();
    h.grow();
    h.refuse.set(true);
    let error = h.publish().unwrap_err();
    assert!(
        matches!(
            error,
            crate::analysis::cooperation::CooperationError::Protocol(KernelError::Stopped(
                crate::analysis::MachineError::Work
            ))
        ),
        "{error:?}"
    );
    assert!(matches!(
        h.cooperation.stop(error, &h.budget),
        crate::analysis::MachineError::Work
    ));
    assert!(h.cooperation.interrupted);
    assert_ne!(h.runtime.view().closure(), analysis_kernel::Closure::Closed);
    assert!(
        weak.strong_count() > 0,
        "refused shared retirement retains ownership rather than certifying cleanup"
    );
    drop(h.cooperation);
    assert_eq!(weak.strong_count(), 0);
}

#[test]
fn readonly_entry_root_work_or_cancel_cannot_certify_a_replacement() {
    use crate::analysis::{control::Control, progress::Observer};
    for cancel in [false, true] {
        let mut h = Harness::new();
        let old = h.publish().unwrap();
        h.grow();
        let mut producer = h.producer();
        let control = Control::new(Observer::default());
        if cancel {
            control.cancellation().cancel();
        } else {
            assert!(!h.budget.charge(usize::MAX));
        }
        let result = control.scope(|| {
            h.cooperation.publish_entry_input(
                Harness::ID,
                h.revision,
                &h.source,
                &mut h.runtime,
                &mut producer,
                &mut h.budget,
                |_| panic!("root stop must precede materialization"),
            )
        });
        assert!(
            matches!(
                result,
                Err(crate::analysis::cooperation::CooperationError::Protocol(
                    KernelError::Stopped(_)
                ))
            ),
            "{result:?}"
        );
        assert_eq!(h.cooperation.readonly[&Harness::ID].input.subject(), old);
        assert_ne!(h.runtime.view().closure(), analysis_kernel::Closure::Closed);
        assert_eq!(h.locals.get(), 0);
        assert_eq!(h.tasks.get(), 0);
    }
}

mod proof;
