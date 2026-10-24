//! A complete-empty relation is observable during saturation, before final reports.

use super::super::{EvmSolver, EvmTable, testing};
use crate::{
    analysis::{
        EvmCallStack, EvmRootFrame, ExecutionConfig, FrontierReason, MachineEdgeKind, MachineError,
        ProcedureSummaryKind,
        cooperation::{
            Extension, InputRoute, Inputs, LocalInput, Point, PointKind, ProcedureId,
            ProcedureRoute, Procedures, RefinementRoute, Refinements, StackFacts, Subject,
            Summaries, SummaryClosureRoute, SummaryClosures, SummaryReadError, SummaryRoute,
            discover_closed_summary,
        },
        engine::{
            CorrelatedJoin, Counters, EnginePolicies, FeedbackAcceleration, RegionalSummaries,
            SemanticPolicies, StructuralPartition, cooperation::Cooperation,
        },
        machine::fixtures::{address, fixture},
        testing::{Entry, product},
        transfer::StorageReadPolicy,
        worklist::{
            MachineMeter,
            runtime::{Runtime, Task, TaskKind},
        },
    },
    domain::NumericValue,
    resource::WorkBudget,
};
use analysis_kernel::{
    Accepts, Analyzer, Discover, KernelError, LocalClosure, PublishError, Read, Reduction,
    Registered, Scoped,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

type Infrastructure = KernelError<MachineError>;

#[derive(Clone, Copy, Debug)]
struct Receipt {
    after_frontier: bool,
    procedure: ProcedureId,
    generation: usize,
    revision: usize,
    complete: bool,
}

#[derive(Debug, thiserror::Error)]
enum ProbeError {
    #[error("{0}")]
    Read(Infrastructure),
    #[error("{0}")]
    Summary(SummaryReadError<Infrastructure>),
    #[error("{0}")]
    Bottom(PublishError<crate::domain::facts::FactError, MachineError>),
}

impl From<Infrastructure> for ProbeError {
    fn from(error: Infrastructure) -> Self {
        Self::Read(error)
    }
}

impl From<SummaryReadError<Infrastructure>> for ProbeError {
    fn from(error: SummaryReadError<Infrastructure>) -> Self {
        Self::Summary(error)
    }
}

impl From<PublishError<crate::domain::facts::FactError, MachineError>> for ProbeError {
    fn from(error: PublishError<crate::domain::facts::FactError, MachineError>) -> Self {
        Self::Bottom(error)
    }
}

/// An explicit Bottom premise supplied through the real registered proof port.
/// This is a unit control for the empty input universe, not a claim that the
/// independently tested live loop is unreachable because it has no outputs.
struct EmptyUniverse;

impl<Cx> Reduction<Cx> for EmptyUniverse
where
    Cx: Accepts<
            Refinements,
            RefinementRoute,
            Error = PublishError<crate::domain::facts::FactError, MachineError>,
        > + analysis_kernel::cooperation::ReductionContext,
{
    type Task = Subject;
    type Error = PublishError<crate::domain::facts::FactError, MachineError>;

    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.accept(Scoped::new(subject.scope, ()), StackFacts::Unreachable)?;
        Ok(LocalClosure::Stable)
    }
}

struct Probe {
    after_frontier: Rc<Cell<bool>>,
    receipts: Rc<RefCell<Vec<Receipt>>>,
    empty_universe: bool,
}

impl<Cx> Analyzer<Cx> for Probe
where
    Cx: analysis_kernel::cooperation::ReductionContext
        + Read<Inputs<NumericValue>, InputRoute, Error = Infrastructure>
        + Discover<Procedures, ProcedureRoute, Error = Infrastructure>
        + Discover<Summaries<NumericValue>, SummaryRoute, Error = Infrastructure>
        + Read<Summaries<NumericValue>, SummaryRoute, Error = Infrastructure>
        + Discover<SummaryClosures, SummaryClosureRoute, Error = Infrastructure>
        + Read<SummaryClosures, SummaryClosureRoute, Error = Infrastructure>,
    Cx: Accepts<
            Refinements,
            RefinementRoute,
            Error = PublishError<crate::domain::facts::FactError, MachineError>,
        >,
{
    type Task = Subject;
    type Error = ProbeError;

    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, ProbeError> {
        // The observer never shares a native Body input scope and never
        // publishes a refinement. Unavailable completion leaves the original
        // conservative executable input unchanged.
        if subject.point.kind != PointKind::World {
            if self.empty_universe && subject.point.kind == PointKind::Body {
                Registered::new(EmptyUniverse).evaluate(subject, cx)?;
            }
            return Ok(LocalClosure::Stable);
        }
        let input = <Cx as Read<Inputs<NumericValue>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        let Some(input) = input.value() else {
            return Ok(LocalClosure::Stable);
        };
        let ids = <Cx as Discover<Procedures, ProcedureRoute>>::discover(
            cx,
            Scoped::new(input.universe(), ()),
        )?;
        for id in ids.value().into_iter().flatten() {
            let snapshot = <Cx as Discover<Summaries<NumericValue>, SummaryRoute>>::discover(
                cx,
                Scoped::new(input.universe(), *id),
            )?;
            if let Some(summary) = snapshot
                .value()
                .filter(|summary| summary.outputs.is_empty())
            {
                self.receipts.borrow_mut().push(Receipt {
                    after_frontier: self.after_frontier.get(),
                    procedure: *id,
                    generation: summary.generation,
                    revision: summary.revision,
                    complete: false,
                });
            }
            let closed = discover_closed_summary::<NumericValue, _>(cx, input.universe(), *id)?;
            if let Some(summary) = closed.value().filter(|summary| summary.outputs.is_empty()) {
                self.receipts.borrow_mut().push(Receipt {
                    after_frontier: self.after_frontier.get(),
                    procedure: *id,
                    generation: summary.generation,
                    revision: summary.revision,
                    complete: true,
                });
            }
        }
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn live_known_loop_never_publishes_complete_empty_during_or_after_saturation() {
    check_terminal_proof(false, false);
}

#[test]
fn live_known_region_blocks_unknown_root_table_completion() {
    check_terminal_proof(true, false);
}

#[test]
fn registered_bottom_still_publishes_complete_empty_for_excluded_entries() {
    check_terminal_proof(false, true);
}

fn check_terminal_proof(unknown_root: bool, empty_universe: bool) {
    for use_summaries in [false, true] {
        let algebra = product(Default::default());
        let domain = algebra.borrowed();
        let (world, _) = if unknown_root {
            // The first CALL either enters the live loop or rejects and
            // reaches a later STATICCALL to Unknown. Both are real branches
            // of one original input; the shared table must cover both roots.
            fixture(&[
                (0x101, "5f5f5f5f5f6101025af1505f5f5f5f6109995afa"),
                (0x102, "5b5f56"),
            ])
        } else {
            fixture(&[(0x101, "5f5f5f5f5f6101025af1"), (0x102, "5b5f56")])
        };
        let entry = Entry::new(address(0x101));
        assert!(entry.environment.caller.as_concrete().is_none());
        assert!(entry.environment.value.singleton().is_none());
        assert!(entry.environment.calldata.len().singleton().is_none());
        let config = ExecutionConfig {
            use_summaries,
            ..Default::default()
        };
        let policies = EnginePolicies::default().with_state(SemanticPolicies::new(
            StructuralPartition,
            CorrelatedJoin,
            FeedbackAcceleration,
            RegionalSummaries::with_early_known_admission(),
        ));
        let mut budget = WorkBudget::new(config.max_work);
        let original = testing::initial(&world, &entry, domain).unwrap();
        let mut first = testing::execute_inner(
            &world,
            &entry,
            original.clone(),
            &config,
            domain,
            &mut budget,
            StorageReadPolicy::Abstract,
            false,
        );
        assert!(first.frontiers.is_empty());
        let at = first
            .successors
            .iter()
            .position(|successor| successor.kind == MachineEdgeKind::Call)
            .unwrap();
        let known = first.successors.swap_remove(at).payload;
        assert!(super::super::is_known_call_entry(&known));
        let (child, pending_known) = if unknown_root {
            let rejected = first
                .successors
                .into_iter()
                .find(|successor| successor.kind == MachineEdgeKind::Failure)
                .unwrap()
                .payload;
            let second = testing::execute_inner(
                &world,
                &entry,
                rejected,
                &config,
                domain,
                &mut budget,
                StorageReadPolicy::Abstract,
                false,
            );
            assert!(second.frontiers.is_empty());
            let unknown = second
                .successors
                .into_iter()
                .find(|successor| successor.kind == MachineEdgeKind::Call)
                .unwrap()
                .payload;
            assert_eq!(unknown.active().code, crate::analysis::FrameCode::Unknown);
            assert!(unknown.active().key.is_static);
            let mut detached = known.clone();
            detached.call_stack = EvmCallStack::new(EvmRootFrame {
                state: known.active().clone(),
            });
            (unknown, Some(detached))
        } else {
            (known, None)
        };
        let receipts = Rc::new(RefCell::new(Vec::new()));
        let after_frontier = Rc::new(Cell::new(false));
        let mut cooperation = Cooperation::new(Extension::new(Probe {
            receipts: Rc::clone(&receipts),
            after_frontier: Rc::clone(&after_frontier),
            empty_universe,
        }));
        let mut runtime = Runtime::new(&mut budget).unwrap();
        runtime.register_world(0, 1, &mut budget).unwrap();
        let (_, mut caller) = runtime.next(false, &mut budget).unwrap().unwrap();
        let world_scope = runtime.world;
        let anchor: analysis_kernel::SlotMeta<usize> = runtime
            .kernel()
            .slot(world_scope, &mut MachineMeter(&mut budget))
            .unwrap();
        let version = runtime
            .with_run(&mut caller, &mut budget, |run| {
                run.observe_slot(&anchor)
                    .map_err(crate::analysis::worklist::kernel_failure)
            })
            .unwrap();
        let point = Point {
            namespace: 0,
            kind: PointKind::World,
            index: 0,
        };
        let fallback = cooperation
            .input(
                point,
                version,
                0,
                None,
                None,
                original.clone(),
                domain,
                &mut runtime,
                &mut caller,
                &mut budget,
                |run| {
                    run.observe_slot(&anchor)?;
                    Ok(())
                },
            )
            .unwrap();
        let LocalInput::Reachable { payload, .. } = fallback else {
            panic!("a pure observer must preserve the original conservative fallback");
        };
        assert_eq!(*payload, original);
        let mut table = EvmTable::default();
        let mut counters = Counters::default();
        if let Some(pending) = pending_known {
            // Admission uses the actual first CALL child. Delay its Body until
            // the Unknown entry is present to test the shared aggregate proof.
            let mut solver = EvmSolver {
                table: &mut table,
                world: &world,
                entry: &entry,
                config: &config,
                domain,
                policies: &policies,
                budget: &mut budget,
                counters: &mut counters,
                storage_reads: StorageReadPolicy::Abstract,
                runtime: &mut runtime,
                cooperation: &mut cooperation,
                active: None,
                caller: Some(&mut caller),
                namespace: usize::from(!use_summaries),
                producer: None,
                deferred: false,
                consumed: None,
            };
            let known = solver.procedure(pending).unwrap();
            solver.observe_origin(point, known).unwrap();
        }
        let (execution, record) = table.execute_with(
            0,
            &world,
            &entry,
            child,
            &config,
            domain,
            &policies,
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
            &mut runtime,
            &mut caller,
            &mut cooperation,
        );
        assert_eq!(
            record.kind,
            if unknown_root {
                ProcedureSummaryKind::UnknownEnvironment
            } else {
                ProcedureSummaryKind::KnownCall
            }
        );
        assert_eq!(
            record.closed,
            empty_universe,
            "frontiers={:?} procedures={:?}",
            execution.frontiers,
            table
                .procedures
                .iter()
                .map(|procedure| (
                    &procedure.frame,
                    procedure.outputs.keys().collect::<Vec<_>>(),
                    &procedure.entries
                ))
                .collect::<Vec<_>>()
        );
        if empty_universe {
            assert!(execution.frontiers.is_empty());
            assert!(table.nodes.iter().all(|node| node.excluded));
            assert!(record.procedures.iter().all(
                |procedure| procedure.outputs.is_empty() && procedure.covered_blocks.is_empty()
            ));
        } else {
            assert!(
                execution
                    .frontiers
                    .iter()
                    .any(|frontier| frontier.reason == FrontierReason::RecursiveNoTerminal)
            );
            assert!(execution.frontiers.iter().any(|frontier| {
                frontier
                    .payload
                    .as_ref()
                    .is_some_and(|payload| payload.active().key.code_address == address(0x102))
            }));
        }
        after_frontier.set(true);
        // Reuse the real observer's selected input scope after the outer guard.
        // Re-enter input first in case a valid invalidation replaced that scope.
        cooperation
            .input(
                point,
                version,
                0,
                None,
                None,
                original.clone(),
                domain,
                &mut runtime,
                &mut caller,
                &mut budget,
                |run| {
                    run.observe_slot(&anchor)?;
                    Ok(())
                },
            )
            .unwrap();
        let subject = cooperation.subject(point).unwrap();
        let task = Task(TaskKind::Extension(subject));
        let job = runtime
            .kernel()
            .register(subject.scope, task, &mut MachineMeter(&mut budget))
            .unwrap();
        let task = runtime
            .kernel()
            .task_handle(job, &mut MachineMeter(&mut budget))
            .unwrap();
        let token = runtime
            .kernel()
            .begin(job, &mut MachineMeter(&mut budget))
            .unwrap()
            .suspend();
        cooperation
            .evaluate(&mut runtime, task, token, &mut budget)
            .unwrap();
        let observed = receipts.borrow();
        for receipt in observed.iter() {
            eprintln!(
                "F1_RECEIPT reuse={use_summaries} after={} procedure={:?} generation={} revision={} complete={}",
                receipt.after_frontier,
                receipt.procedure,
                receipt.generation,
                receipt.revision,
                receipt.complete
            );
        }
        assert!(
            observed
                .iter()
                .any(|receipt| !receipt.after_frontier && !receipt.complete)
        );
        assert!(
            observed
                .iter()
                .any(|receipt| receipt.after_frontier && !receipt.complete)
        );
        assert_eq!(
            observed.iter().any(|receipt| receipt.complete),
            empty_universe,
            "unexpected complete-empty certificate: {observed:?}"
        );
        drop(observed);
        if empty_universe {
            let root = record.root_procedure.unwrap();
            let node = *table.procedures[root].entries.first().unwrap();
            let native_version = runtime
                .with_run(&mut caller, &mut budget, |run| {
                    run.slot_version(&table.nodes[node].input)
                        .map_err(crate::analysis::worklist::kernel_failure)
                })
                .unwrap();
            let mut solver = EvmSolver {
                table: &mut table,
                world: &world,
                entry: &entry,
                config: &config,
                domain,
                policies: &policies,
                budget: &mut budget,
                counters: &mut counters,
                storage_reads: StorageReadPolicy::Abstract,
                runtime: &mut runtime,
                cooperation: &mut cooperation,
                active: None,
                caller: Some(&mut caller),
                namespace: usize::from(!use_summaries),
                producer: None,
                deferred: false,
                consumed: None,
            };
            let before = receipts.borrow().len();
            // Reopening a proved-excluded entry is a conservative pending
            // obligation. Its old proof must retire without numeric growth.
            solver.set_excluded(node, false).unwrap();
            let next_version = solver
                .runtime
                .with_run(
                    solver.caller.as_deref_mut().unwrap(),
                    solver.budget,
                    |run| {
                        run.slot_version(&solver.table.nodes[node].input)
                            .map_err(crate::analysis::worklist::kernel_failure)
                    },
                )
                .unwrap();
            assert_eq!(native_version, next_version);
            solver
                .cooperation
                .input(
                    point,
                    version,
                    0,
                    None,
                    None,
                    original,
                    domain,
                    solver.runtime,
                    solver.caller.as_deref_mut().unwrap(),
                    solver.budget,
                    |run| {
                        run.observe_slot(&anchor)?;
                        Ok(())
                    },
                )
                .unwrap();
            let after = receipts.borrow().len();
            assert!(
                after > before,
                "a fresh real observer must run before recomputation"
            );
            assert!(
                receipts.borrow()[before..after]
                    .iter()
                    .all(|receipt| !receipt.complete)
            );
            // The same actual Bottom premise can restore exclusion. Complete
            // emptiness then renews without a fabricated MAY output/revision.
            solver.set_excluded(node, true).unwrap();
            solver.runtime.schedule_observation(solver.budget).unwrap();
            solver.run();
            assert!(solver.table.frontiers.is_empty());
            let renewed = receipts.borrow();
            assert!(renewed[after..].iter().any(|receipt| receipt.complete));
            let original_generation = renewed
                .iter()
                .find(|receipt| receipt.complete)
                .unwrap()
                .generation;
            assert!(
                renewed
                    .iter()
                    .filter(|receipt| receipt.complete)
                    .all(|receipt| receipt.generation == original_generation)
            );
            eprintln!(
                "F1_EXCLUSION reuse={use_summaries} numeric_slot_unchanged=true invalidation_seen=true empty_proof_renewed=true"
            );
        }
    }
}
