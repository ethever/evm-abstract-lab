//! Summary readers must inherit the true validity of native Body assumptions.
use super::{
    deferred::{GuardBound, Widen},
    *,
};
use analysis_kernel::{Availability, Change, There};
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::cooperation::{
    PointKind, ProcedureId, ProcedureRoute, Procedures, Summaries, SummaryClosureRoute,
    SummaryClosures, SummaryRoute, closed_summary,
};
use evm_abstract::analysis::{AnalysisEngine, AnalysisInput, ExecutionConfig};
use std::{cell::RefCell, collections::BTreeMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Wake {
    procedure: ProcedureId,
    generation: usize,
    revision: usize,
    body_evaluations: usize,
    invalidated: bool,
}
/// Scheduling witness only: discovery must not import the publisher's premises.
struct Trigger;
impl Capability for Trigger {
    type Key = ();
    type Value = Wake;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &Wake) -> usize {
        8
    }
}
struct First;
impl JoinPolicy for First {
    type Value = Wake;
    type Error = Infallible;
    fn work(&self, _: &Wake, _: &Wake) -> usize {
        16
    }
    fn join(&self, old: &Wake, _: &Wake) -> Result<Wake, Infallible> {
        Ok(*old)
    }
    fn equivalent(&self, left: &Wake, right: &Wake) -> bool {
        left == right
    }
}
struct Factory;
impl ExtraStores for Factory {
    type Stores =
        Cons<Channel<GuardBound, Ascending<Widen>>, Cons<Channel<Trigger, Ascending<First>>, Nil>>;
    fn retirement_work(&self, stores: &Self::Stores) -> usize {
        stores
            .head
            .len()
            .saturating_add(stores.tail.head.len())
            .saturating_mul(16)
    }
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        Ok(Cons::new(
            kernel.channel(Ascending(Widen), meter)?,
            Cons::new(kernel.channel(Ascending(First), meter)?, Nil),
        ))
    }
}
type BoundRoute = SharedRoute<Here>;
type TriggerRoute = SharedRoute<There<Here>>;

#[derive(Default)]
struct Audit {
    latest_bodies: BTreeMap<Point, Subject>,
    body_evaluations: usize,
    triggered: bool,
    probes: usize,
}
type Trace = Rc<RefCell<Audit>>;

struct Source {
    reader_point: Option<Point>,
    fired: bool,
    invalidate: bool,
    trace: Trace,
}
impl<Cx> Analyzer<Cx> for Source
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Discover<Procedures, ProcedureRoute, Error = Infrastructure>
        + Discover<Summaries<ExcludingWord>, SummaryRoute, Error = Infrastructure>
        + Discover<SummaryClosures, SummaryClosureRoute, Error = Infrastructure>
        + Discover<Generations, GenerationRoute, Error = Infrastructure>
        + ReadGeneration<Error = Infrastructure>
        + Accepts<GuardBound, BoundRoute, Error = PublishError<Infallible, MachineError>>
        + Accepts<Trigger, TriggerRoute, Error = PublishError<Infallible, MachineError>>
        + Charges<Error = Infrastructure>,
{
    type Task = Subject;
    type Error = Failure;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Failure> {
        if subject.point.kind != PointKind::World {
            return Ok(LocalClosure::Stable);
        }
        // Keep the Summary reader in a different input scope from every X
        // publisher. Reading Summary must not make X's producer adopt Body.
        let reader = *self.reader_point.get_or_insert(subject.point);
        if subject.point == reader {
            return Ok(LocalClosure::Stable);
        }
        let input = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        let Some(input) = input.value() else {
            return Ok(LocalClosure::Stable);
        };
        cx.charge(input.work_size())?;
        let initial_valid = input
            .payload()
            .store
            .slots()
            .get(&(fixtures::address(fixtures::OWNER), U256::ZERO))
            .is_some_and(|word| !word.numeric().contains(U256::from(42)));
        assert!(
            initial_valid,
            "X's initial guard follows from the real immutable store"
        );
        if !self.fired {
            let ids = <Cx as Discover<Procedures, ProcedureRoute>>::discover(
                cx,
                Scoped::new(input.universe(), ()),
            )?;
            if let Some(ids) = ids.value() {
                for id in ids {
                    let answer =
                        <Cx as Discover<Summaries<ExcludingWord>, SummaryRoute>>::discover(
                            cx,
                            Scoped::new(input.universe(), *id),
                        )?;
                    let proof = <Cx as Discover<SummaryClosures, SummaryClosureRoute>>::discover(
                        cx,
                        Scoped::new(input.universe(), *id),
                    )?;
                    let Some(summary) = answer.value().filter(|summary| {
                        !summary.outputs.is_empty()
                            && proof
                                .value()
                                .is_some_and(|proof| proof.generation == summary.generation)
                    }) else {
                        continue;
                    };
                    let (body, body_evaluations) = {
                        let audit = self.trace.borrow();
                        (
                            *audit
                                .latest_bodies
                                .values()
                                .next()
                                .expect("actual Body must have consumed X"),
                            audit.body_evaluations,
                        )
                    };
                    // Routing observations do not adopt a Body guarantee. This
                    // proves actual invalidation, not may-discovery absence.
                    let entry = <Cx as Discover<Generations, GenerationRoute>>::discover(
                        cx,
                        Scoped::new(input.universe(), body.point),
                    )?;
                    let entry = entry.value().unwrap();
                    assert_eq!(cx.current_generation(entry)?.value(), Some(&body.scope));
                    let changed = <Cx as Accepts<GuardBound, BoundRoute>>::accept(
                        cx,
                        Scoped::new(input.universe(), ()),
                        self.invalidate,
                    )
                    .map_err(Failure::Bound)?;
                    if self.invalidate {
                        assert_eq!(changed, Change::Changed);
                        assert!(
                            matches!(cx.current_generation(entry)?.state, Availability::Pending),
                            "X growth must actually stale the selected Body"
                        );
                    } else {
                        assert_eq!(changed, Change::Unchanged);
                        assert_eq!(cx.current_generation(entry)?.value(), Some(&body.scope));
                    }
                    self.fired = true;
                    self.trace.borrow_mut().triggered = true;
                    let wake = Wake {
                        procedure: *id,
                        generation: summary.generation,
                        revision: summary.revision,
                        body_evaluations,
                        invalidated: self.invalidate,
                    };
                    eprintln!(
                        "SUMMARY_VALIDITY X_PUBLICATION body={body:?} changed={changed:?} baseline={wake:?}"
                    );
                    <Cx as Accepts<Trigger, TriggerRoute>>::accept(
                        cx,
                        Scoped::new(input.universe(), ()),
                        wake,
                    )
                    .map_err(Failure::Bound)?;
                    return Ok(LocalClosure::Stable);
                }
            }
        }
        <Cx as Accepts<GuardBound, BoundRoute>>::accept(
            cx,
            Scoped::new(input.universe(), ()),
            self.fired && self.invalidate,
        )
        .map_err(Failure::Bound)?;
        Ok(LocalClosure::Stable)
    }
}

struct Body {
    rule: super::super::bridge::rule::Rule,
    trace: Trace,
}
impl<Cx> Reduction<Cx> for Body
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Read<GuardBound, BoundRoute, Error = Infrastructure>
        + Read<Refinements, RefinementRoute, Error = Infrastructure>
        + Accepts<Refinements, RefinementRoute, Error = PublishError<FactError, MachineError>>
        + Charges<Error = Infrastructure>,
{
    type Task = Subject;
    type Error =
        super::super::bridge::rule::Failure<Infrastructure, PublishError<FactError, MachineError>>;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        use super::super::bridge::rule::Failure as RuleFailure;
        if subject.point.kind == PointKind::Body {
            let input = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
                cx,
                Scoped::new(subject.scope, ()),
            )
            .map_err(RuleFailure::Infrastructure)?;
            let Some(input) = input.value() else {
                return Ok(LocalClosure::Stable);
            };
            let bound =
                <Cx as Read<GuardBound, BoundRoute>>::query(cx, Scoped::new(input.universe(), ()))
                    .map_err(RuleFailure::Infrastructure)?;
            let Some(widen) = bound.value() else {
                return Ok(LocalClosure::Stable);
            };
            {
                let mut audit = self.trace.borrow_mut();
                audit.body_evaluations += 1;
                if subject.point.namespace == 1 {
                    audit.latest_bodies.insert(subject.point, *subject);
                }
            }
            if *widen {
                return Ok(LocalClosure::Stable);
            }
        }
        self.rule.reduce(subject, cx)
    }
}

#[derive(Debug, thiserror::Error)]
enum ReaderError {
    #[error("{0}")]
    Summary(evm_abstract::analysis::cooperation::SummaryReadError<Infrastructure>),
    #[error("capability operation failed: {0}")]
    Infrastructure(Infrastructure),
    #[error("old closed Summary is still available: generation={generation}, revision={revision}")]
    StaleClosed { generation: usize, revision: usize },
}
impl From<evm_abstract::analysis::cooperation::SummaryReadError<Infrastructure>> for ReaderError {
    fn from(error: evm_abstract::analysis::cooperation::SummaryReadError<Infrastructure>) -> Self {
        Self::Summary(error)
    }
}
impl From<Infrastructure> for ReaderError {
    fn from(error: Infrastructure) -> Self {
        Self::Infrastructure(error)
    }
}
struct Reader {
    point: Option<Point>,
    trace: Trace,
}
impl<Cx> Analyzer<Cx> for Reader
where
    // Deliberately no X, Generation, foreign Input or Body capability access.
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Discover<Trigger, TriggerRoute, Error = Infrastructure>
        + Read<Summaries<ExcludingWord>, SummaryRoute, Error = Infrastructure>
        + Read<SummaryClosures, SummaryClosureRoute, Error = Infrastructure>
        + Charges<Error = Infrastructure>,
{
    type Task = Subject;
    type Error = ReaderError;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, ReaderError> {
        if subject.point.kind != PointKind::World {
            return Ok(LocalClosure::Stable);
        }
        let reader = *self.point.get_or_insert(subject.point);
        if subject.point != reader {
            return Ok(LocalClosure::Stable);
        }
        let input = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        let Some(input) = input.value() else {
            return Ok(LocalClosure::Stable);
        };
        let wake = <Cx as Discover<Trigger, TriggerRoute>>::discover(
            cx,
            Scoped::new(input.universe(), ()),
        )?;
        let Some(wake) = wake.value() else {
            return Ok(LocalClosure::Stable);
        };
        {
            let mut audit = self.trace.borrow_mut();
            if audit.probes != 0 {
                return Ok(LocalClosure::Stable);
            }
            assert!(audit.triggered);
            assert_eq!(
                audit.body_evaluations, wake.body_evaluations,
                "query must precede native Body recomputation"
            );
            audit.probes += 1;
        }
        let answer = closed_summary::<ExcludingWord, _>(cx, input.universe(), wake.procedure)?;
        match answer.state {
            Availability::Pending | Availability::Absent => {
                assert!(
                    wake.invalidated,
                    "unchanged provider must preserve a ready closed Summary"
                );
                eprintln!("SUMMARY_VALIDITY reader=unavailable baseline={wake:?}")
            }
            Availability::Value(summary) => {
                assert_eq!(
                    summary.generation, wake.generation,
                    "native output bound must not grow before this query"
                );
                eprintln!(
                    "SUMMARY_VALIDITY reader=complete generation={} revision={} body_evaluations={}",
                    summary.generation, summary.revision, wake.body_evaluations
                );
                if !wake.invalidated {
                    assert_eq!(summary.revision, wake.revision);
                }
                if wake.invalidated {
                    assert_eq!(
                        summary.revision, wake.revision,
                        "old completion snapshot is unchanged"
                    );
                    return Err(ReaderError::StaleClosed {
                        generation: summary.generation,
                        revision: summary.revision,
                    });
                }
            }
        }
        Ok(LocalClosure::Stable)
    }
}
struct Tasks;
impl InputTasks for Tasks {
    type Task = Either<Either<Subject, Subject>, Subject>;
    const ENABLED: bool = true;
    fn registration_work(&self) -> usize {
        8
    }
    fn tasks(&self, subject: Subject) -> impl Iterator<Item = Self::Task> {
        [
            Either::Left(Either::Left(subject)),
            Either::Left(Either::Right(subject)),
            Either::Right(subject),
        ]
        .into_iter()
    }
}
fn run(use_summaries: bool, invalidate: bool) {
    let fixture = super::super::bridge::fixture::guarded(true);
    let mut code = fixture.code;
    code.pop();
    code.extend([
        opcode::POP,
        opcode::PUSH1,
        42,
        opcode::PUSH0,
        opcode::MSTORE,
        opcode::PUSH1,
        32,
        opcode::PUSH0,
        opcode::RETURN,
    ]);
    let trace = Trace::default();
    let calls = Calls::default();
    let algebra = fixtures::feedback_algebra(&calls);
    let mut extension = Extension::with_tasks(
        Pair::new(
            Pair::new(
                Source {
                    reader_point: None,
                    fired: false,
                    invalidate,
                    trace: Rc::clone(&trace),
                },
                Registered::new(Body {
                    rule: super::super::bridge::rule::Rule {
                        enabled: true,
                        ..Default::default()
                    },
                    trace: Rc::clone(&trace),
                }),
            ),
            Reader {
                point: None,
                trace: Rc::clone(&trace),
            },
        ),
        Tasks,
    )
    .with_shared_stores(Factory);
    let result = AnalysisEngine::new(&algebra, &EnginePolicies::default(), &mut extension).run(
        AnalysisInput::new(fixtures::world(&code), fixtures::entry()).unwrap(),
        ExecutionConfig {
            use_summaries,
            ..fixtures::config(5_000_000_000)
        },
        &Control::default(),
    );
    let audit = trace.borrow();
    assert!(
        audit.triggered && audit.probes > 0,
        "the real invalidation window must be observed"
    );
    eprintln!(
        "SUMMARY_VALIDITY result={:?}",
        result.as_ref().map(|r| r.status())
    );
    let result = result.expect("Summary-only reader must reject an invalid closed guarantee");
    fixtures::complete(&result);
}
#[test]
fn summary_only_reader_tracks_provider_validity_in_each_cache_mode() {
    for (case, use_summaries, invalidate) in [
        (
            "summary_only_reader_rejects_stale_closed_without_summary_cache",
            false,
            true,
        ),
        (
            "summary_only_reader_rejects_stale_closed_with_summary_cache",
            true,
            true,
        ),
        (
            "unchanged_provider_preserves_a_valid_closed_summary",
            true,
            false,
        ),
        (
            "unchanged_provider_preserves_a_valid_closed_summary_without_cache",
            false,
            false,
        ),
    ] {
        eprintln!("case={case}");
        run(use_summaries, invalidate);
    }
}
