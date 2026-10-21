//! Real-generation ownership witnesses, without retaining public Input handles.
use crate::{
    Fork,
    analysis::{
        EnginePolicies, ExecutionConfig, FrontierReason, Status,
        cooperation::{
            Extension, ExtraStores, InputRoute, Inputs, Point, Subject, analyze_world_cooperating,
        },
    },
    domain::{AbstractValue, Domain, NumericValue, ValueAlgebra},
    world::{Account, ByteArray, Entry, World},
};
use alloy_primitives::{Address, U256};
use analysis_kernel::{Analyzer, Kernel, KernelError, LocalClosure, Meter, Read, Scoped};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::Weak,
};

#[derive(Default)]
struct Counts {
    created: Cell<usize>,
    dropped: Cell<usize>,
    peak: Cell<usize>,
    quoted: Cell<usize>,
}
struct Store(Rc<Counts>);
impl Drop for Store {
    fn drop(&mut self) {
        self.0.dropped.set(self.0.dropped.get() + 1);
    }
}
struct Factory {
    counts: Rc<Counts>,
    refuse: bool,
}
impl ExtraStores for Factory {
    type Stores = Store;
    fn retirement_work(&self, _: &Store) -> usize {
        self.counts.quoted.set(self.counts.quoted.get() + 1);
        if self.refuse { usize::MAX } else { 4 }
    }
    fn create<J, M: Meter>(
        &self,
        _: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Store, KernelError<M::Stop>> {
        meter.reserve(4).map_err(KernelError::Stopped)?;
        self.counts.created.set(self.counts.created.get() + 1);
        self.counts.peak.set(
            self.counts
                .peak
                .get()
                .max(self.counts.created.get() - self.counts.dropped.get()),
        );
        Ok(Store(Rc::clone(&self.counts)))
    }
}
struct Watch {
    counts: Rc<Counts>,
    generations: BTreeMap<Point, BTreeSet<analysis_kernel::Scope>>,
    weak: Vec<Weak<crate::analysis::EvmMachinePayload<NumericValue>>>,
    dead_during_run: usize,
    drops_during_run: usize,
}
impl<Cx: Read<Inputs<NumericValue>, InputRoute>> Analyzer<Cx> for Watch {
    type Task = Subject;
    type Error = Cx::Error;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        let answer = cx.query(Scoped::new(subject.scope, ()))?;
        if let Some(input) = answer.value() {
            if self
                .generations
                .entry(subject.point)
                .or_default()
                .insert(subject.scope)
            {
                self.weak.push(input.downgrade());
            }
            self.dead_during_run = self
                .weak
                .iter()
                .filter(|weak| weak.strong_count() == 0)
                .count();
            self.drops_during_run = self.counts.dropped.get();
        }
        Ok(LocalClosure::Stable)
    }
}
fn run(refuse: bool) -> (crate::analysis::WorldAnalysis, Watch, Rc<Counts>) {
    let owner = Address::from_word(U256::from(0x101).into());
    let mut world = World::new(Fork::Osaka, "generation-lifetime");
    world
        .insert(
            owner,
            Account::from_hex("5f5b600101600156", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = Entry::concrete(
        owner,
        Address::ZERO,
        AbstractValue::constant(U256::ZERO),
        ByteArray::empty(),
    );
    let counts = Rc::new(Counts::default());
    let mut extension = Extension::new(Watch {
        counts: Rc::clone(&counts),
        generations: BTreeMap::new(),
        weak: Vec::new(),
        dead_during_run: 0,
        drops_during_run: 0,
    })
    .with_stores(Factory {
        counts: Rc::clone(&counts),
        refuse,
    });
    let result = analyze_world_cooperating(
        world,
        entry,
        ExecutionConfig {
            max_work: 100_000_000,
            root_terminal_coverage: false,
            ..ExecutionConfig::default()
        },
        &ValueAlgebra::new(Domain::default()),
        &EnginePolicies::default(),
        &mut extension,
    )
    .unwrap();
    (result, extension.analyzer, counts)
}
#[test]
fn old_native_snapshots_and_client_stores_drop_during_many_generations() {
    let (result, watch, counts) = run(false);
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    let generations: usize = watch.generations.values().map(BTreeSet::len).sum();
    assert!(
        generations > watch.generations.len() + 3,
        "exercise repeated replacement"
    );
    assert!(
        watch.dead_during_run > 0,
        "reclaimed Inputs release native snapshots during the run"
    );
    assert!(
        watch.drops_during_run > 0,
        "client stores release before whole-run destruction"
    );
    assert!(
        counts.peak.get() <= watch.generations.len() + 1,
        "retention is bounded by current points plus one replacement"
    );
    assert_eq!(counts.created.get(), counts.dropped.get());
    assert!(watch.weak.iter().all(|weak| weak.strong_count() == 0));
}
#[test]
fn refused_client_disposal_quote_leaves_an_honest_unfinished_run() {
    let (result, _, counts) = run(true);
    assert!(counts.quoted.get() > 0);
    assert_eq!(result.status(), Status::Incomplete);
    assert!(
        result
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::Work)
    );
    assert_eq!(
        counts.created.get(),
        counts.dropped.get(),
        "final ownership cleanup still happens after interruption"
    );
}

type Assembly = Extension<Watch, crate::analysis::cooperation::PerInput, Factory>;
struct Inventory {
    cooperation: super::super::Cooperation<
        NumericValue,
        Assembly,
        KernelError<crate::analysis::MachineError>,
    >,
    runtime: crate::analysis::worklist::runtime::Runtime<Subject>,
    anchor: analysis_kernel::SlotMeta<crate::analysis::EvmMachinePayload<NumericValue>>,
    assumption: analysis_kernel::SlotMeta<bool>,
    owner: analysis_kernel::JobId,
    payload: crate::analysis::EvmMachinePayload<NumericValue>,
    algebra: ValueAlgebra<Domain>,
    budget: crate::resource::WorkBudget,
    counts: Rc<Counts>,
}
impl Inventory {
    fn new() -> Self {
        use crate::analysis::{
            machine::fixtures,
            worklist::{
                MachineMeter,
                runtime::{Runtime, TaskKind},
            },
        };
        let domain = Domain::default();
        let mut payload = fixtures::payload(domain);
        payload.active_mut().stack.push(fixtures::word(0));
        payload.normalize();
        let mut budget = crate::resource::WorkBudget::new(1_000_000_000);
        let mut runtime = Runtime::new(&mut budget).unwrap();
        let relative = runtime.relative;
        let anchor = runtime
            .kernel()
            .slot(relative, &mut MachineMeter(&mut budget))
            .unwrap();
        let assumption = runtime
            .kernel()
            .slot(relative, &mut MachineMeter(&mut budget))
            .unwrap();
        let owner = runtime
            .register_relative(TaskKind::Body(0), &mut budget)
            .unwrap();
        let counts = Rc::new(Counts::default());
        let extension = Extension::new(Watch {
            counts: Rc::clone(&counts),
            generations: BTreeMap::new(),
            weak: Vec::new(),
            dead_during_run: 0,
            drops_during_run: 0,
        })
        .with_stores(Factory {
            counts: Rc::clone(&counts),
            refuse: false,
        });
        Self {
            cooperation: super::super::Cooperation::new(extension),
            runtime,
            anchor,
            assumption,
            owner,
            payload,
            algebra: ValueAlgebra::new(domain),
            budget,
            counts,
        }
    }
    fn point(namespace: u64) -> Point {
        Point {
            namespace,
            kind: crate::analysis::cooperation::PointKind::Body,
            index: 0,
        }
    }
    fn view(&mut self, namespace: u64, token: &mut analysis_kernel::RunningJob) {
        use crate::analysis::worklist::MachineMeter;
        let version = {
            let mut meter = MachineMeter(&mut self.budget);
            let run = self.runtime.kernel().resume(token, &mut meter).unwrap();
            let version = run.slot_version(&self.anchor).unwrap();
            *token = run.suspend();
            version
        };
        let anchor = &self.anchor;
        let assumption = (namespace == 1).then_some(&self.assumption);
        let view = self
            .cooperation
            .input(
                Self::point(namespace),
                version,
                0,
                None,
                if namespace == 2 {
                    self.cooperation.subject(Self::point(1))
                } else {
                    None
                },
                self.payload.clone(),
                self.algebra.borrowed(),
                &mut self.runtime,
                token,
                &mut self.budget,
                |run| {
                    assert_eq!(run.observe_slot(anchor)?, version);
                    if let Some(assumption) = assumption {
                        run.observe_slot(assumption)?;
                    }
                    Ok(())
                },
            )
            .unwrap();
        assert!(matches!(
            view,
            crate::analysis::cooperation::LocalInput::Reachable { .. }
        ));
    }
    fn begin(&mut self) -> analysis_kernel::RunningJob {
        use crate::analysis::worklist::MachineMeter;
        self.runtime
            .kernel()
            .retry(self.owner, &mut MachineMeter(&mut self.budget))
            .unwrap();
        let (_, mut token) = self.runtime.next(true, &mut self.budget).unwrap().unwrap();
        self.cooperation
            .begin_owner(&mut self.runtime, &mut token, &mut self.budget)
            .unwrap();
        self.view(1, &mut token);
        token
    }
    fn finish(&mut self, mut token: analysis_kernel::RunningJob) {
        self.cooperation
            .finish_owner(
                &mut self.runtime,
                &mut token,
                &mut self.budget,
                self.cooperation.subject(Self::point(1)),
            )
            .unwrap();
        self.runtime.finish(token, &mut self.budget).unwrap();
    }
    fn selection(
        &mut self,
        token: &mut analysis_kernel::RunningJob,
        entry: &analysis_kernel::GenerationEntry,
    ) -> analysis_kernel::Availability<analysis_kernel::Scope> {
        use crate::analysis::worklist::MachineMeter;
        let mut meter = MachineMeter(&mut self.budget);
        let mut run = self.runtime.kernel().resume(token, &mut meter).unwrap();
        let answer = run.current_generation(entry).unwrap();
        *token = run.suspend();
        answer.state
    }
}

#[test]
fn complete_contribution_disappears_and_reappears_through_the_same_public_entry() {
    let mut h = Inventory::new();
    let point = Inventory::point(2);
    let mut first = h.begin();
    h.view(2, &mut first);
    let old_scope = h.cooperation.bindings[&point].input.subject().scope;
    let weak = h.cooperation.bindings[&point].input.downgrade();
    let entry = h.cooperation.bindings[&point].entry.clone();
    h.finish(first);

    let mut absent = h.begin();
    h.cooperation
        .finish_owner(
            &mut h.runtime,
            &mut absent,
            &mut h.budget,
            h.cooperation.subject(Inventory::point(1)),
        )
        .unwrap();
    assert!(matches!(
        h.selection(&mut absent, &entry),
        analysis_kernel::Availability::Absent
    ));
    assert!(!h.cooperation.bindings.contains_key(&point));
    assert!(h.cooperation.dormant[&point].entry == entry);
    assert_eq!(
        weak.strong_count(),
        0,
        "withdrawal releases the native snapshot immediately"
    );
    assert!(h.counts.dropped.get() > 0);
    h.runtime.finish(absent, &mut h.budget).unwrap();

    let mut reappeared = h.begin();
    h.view(2, &mut reappeared); // Same raw anchor version, genuinely fresh scope.
    let current = h.cooperation.bindings[&point].input.subject().scope;
    assert_ne!(current, old_scope);
    assert!(h.cooperation.bindings[&point].entry == entry);
    assert!(
        matches!(h.selection(&mut reappeared, &entry), analysis_kernel::Availability::Value(scope) if scope == current)
    );
    h.finish(reappeared);
}

#[test]
fn self_loop_native_growth_during_evaluation_cannot_withdraw_old_contributions() {
    use crate::analysis::{
        machine::fixtures,
        worklist::{MachineMeter, kernel_failure},
    };
    let mut h = Inventory::new();
    let point = Inventory::point(2);
    let mut first = h.begin();
    h.view(2, &mut first);
    let entry = h.cooperation.bindings[&point].entry.clone();
    h.finish(first);

    let mut current = h.begin();
    let work = h.payload.work_size().saturating_mul(8);
    MachineMeter(&mut h.budget).reserve(work).unwrap();
    let mut grown = h.payload.clone();
    grown.active_mut().stack[0] = Domain::default().join(&fixtures::word(0), &fixtures::word(1));
    let payload = &mut h.payload;
    h.runtime
        .with_run(&mut current, &mut h.budget, |run| {
            run.publish_prepared_with(&h.anchor, analysis_kernel::Change::Changed, work, || {
                *payload = grown
            })
            .map_err(kernel_failure)
        })
        .unwrap();
    h.cooperation
        .finish_owner(
            &mut h.runtime,
            &mut current,
            &mut h.budget,
            h.cooperation.subject(Inventory::point(1)),
        )
        .unwrap();
    assert!(h.cooperation.bindings.contains_key(&point));
    assert!(!h.cooperation.dormant.contains_key(&point));
    assert!(
        h.runtime
            .view()
            .generation_stale(&entry, &mut MachineMeter(&mut h.budget))
            .unwrap()
    );
    h.runtime.finish(current, &mut h.budget).unwrap();

    let mut refreshed = h.begin();
    h.cooperation
        .finish_owner(
            &mut h.runtime,
            &mut refreshed,
            &mut h.budget,
            h.cooperation.subject(Inventory::point(1)),
        )
        .unwrap();
    assert!(matches!(
        h.selection(&mut refreshed, &entry),
        analysis_kernel::Availability::Absent
    ));
    h.runtime.finish(refreshed, &mut h.budget).unwrap();
}

#[test]
fn adopted_parent_invalidation_withholds_child_and_cannot_launder_complete_empty_receipt() {
    use crate::analysis::worklist::{MachineMeter, kernel_failure};
    let mut h = Inventory::new();
    let point = Inventory::point(2);
    let mut first = h.begin();
    h.view(2, &mut first);
    let entry = h.cooperation.bindings[&point].entry.clone();
    let old_child = h.cooperation.subject(point).unwrap();
    h.finish(first);

    let mut current = h.begin();
    let consumed = h.cooperation.subject(Inventory::point(1)).unwrap();
    let raw_version = h.cooperation.bindings[&point].stamp.anchor;
    h.runtime
        .with_run(&mut current, &mut h.budget, |run| {
            run.publish_prepared_with(&h.assumption, analysis_kernel::Change::Changed, 1, || {})
                .map_err(kernel_failure)
        })
        .unwrap();
    assert!(
        h.runtime
            .view()
            .generation_stale(&entry, &mut MachineMeter(&mut h.budget))
            .unwrap(),
        "child Inputs must adopt the parent's full validity head before republication"
    );
    assert!(matches!(
        h.selection(&mut current, &entry),
        analysis_kernel::Availability::Pending
    ));
    let view = h
        .cooperation
        .input(
            point,
            raw_version,
            0,
            None,
            Some(consumed),
            h.payload.clone(),
            h.algebra.borrowed(),
            &mut h.runtime,
            &mut current,
            &mut h.budget,
            |_| Ok(()),
        )
        .unwrap();
    assert!(matches!(
        view,
        crate::analysis::cooperation::LocalInput::Deferred
    ));
    assert_eq!(
        h.cooperation.subject(point),
        Some(old_child),
        "an obsolete source cannot install a new child universe"
    );

    // Even a replacement parent selected during accept cannot certify the
    // complete-empty conclusion computed using the captured previous parent.
    h.view(1, &mut current);
    assert_ne!(h.cooperation.subject(Inventory::point(1)), Some(consumed));
    h.cooperation
        .finish_owner(&mut h.runtime, &mut current, &mut h.budget, Some(consumed))
        .unwrap();
    assert!(h.cooperation.bindings.contains_key(&point));
    assert!(!h.cooperation.dormant.contains_key(&point));
    h.runtime.finish(current, &mut h.budget).unwrap();

    let mut complete = h.begin();
    let fresh = h.cooperation.subject(Inventory::point(1));
    h.cooperation
        .finish_owner(&mut h.runtime, &mut complete, &mut h.budget, fresh)
        .unwrap();
    assert!(matches!(
        h.selection(&mut complete, &entry),
        analysis_kernel::Availability::Absent
    ));
    h.runtime.finish(complete, &mut h.budget).unwrap();
}

#[test]
fn budget_or_cancellation_refusal_does_not_withdraw_an_incomplete_source_inventory() {
    use crate::analysis::{control::Control, progress::Observer};
    for cancel in [false, true] {
        let mut h = Inventory::new();
        let point = Inventory::point(2);
        let mut first = h.begin();
        h.view(2, &mut first);
        h.finish(first);
        let weak = h.cooperation.bindings[&point].input.downgrade();
        let mut current = h.begin();
        let control = Control::new(Observer::default());
        let result = if cancel {
            control.cancellation().cancel();
            control.scope(|| {
                h.cooperation.finish_owner(
                    &mut h.runtime,
                    &mut current,
                    &mut h.budget,
                    h.cooperation.subject(Inventory::point(1)),
                )
            })
        } else {
            assert!(!h.budget.charge(usize::MAX));
            h.cooperation.finish_owner(
                &mut h.runtime,
                &mut current,
                &mut h.budget,
                h.cooperation.subject(Inventory::point(1)),
            )
        };
        assert!(result.is_err());
        assert!(h.cooperation.bindings.contains_key(&point));
        assert!(!h.cooperation.dormant.contains_key(&point));
        assert!(weak.strong_count() > 0);
        assert_ne!(h.runtime.view().closure(), analysis_kernel::Closure::Closed);
    }
}

#[test]
fn stop_between_client_readiness_and_retry_keeps_the_native_deferred_obligation() {
    use crate::analysis::{control::Control, progress::Observer, worklist::MachineMeter};
    for cancel in [false, true] {
        let mut h = Inventory::new();
        let current = h.begin();
        h.cooperation
            .defer_owner(h.owner, &mut h.runtime, &mut h.budget)
            .unwrap();
        // The native transfer yielded with a represented obligation. Its
        // clients subsequently become ready without completing that transfer.
        h.runtime.finish(current, &mut h.budget).unwrap();
        let subject = h.cooperation.subject(Inventory::point(1)).unwrap();
        assert_eq!(
            h.runtime
                .view()
                .closure_in_metered(subject.scope, &mut MachineMeter(&mut h.budget))
                .unwrap(),
            analysis_kernel::Closure::Closed
        );
        let obligation = h.cooperation.deferred[&h.owner];
        let control = Control::new(Observer::default());
        let result = if cancel {
            control.cancellation().cancel();
            control.scope(|| {
                h.runtime
                    .kernel()
                    .retry(h.owner, &mut MachineMeter(&mut h.budget))
            })
        } else {
            assert!(h.budget.charge(1_000_000_000 - h.budget.used()));
            h.runtime
                .kernel()
                .retry(h.owner, &mut MachineMeter(&mut h.budget))
        };
        assert!(matches!(result, Err(KernelError::Stopped(_))), "{result:?}");
        assert_eq!(h.cooperation.deferred[&h.owner], obligation);
        assert!(
            matches!(h.runtime.view().closure(), analysis_kernel::Closure::Open(open) if open.obligations > 0)
        );
    }
}

#[path = "seed.rs"]
mod seed;
