//! Discover native procedure observations without implementation table indices.
use super::*;
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::cooperation::{
    CalleeRoute, Callees, ContinuationRoute, Continuations, PointKind, ProcedureEntries,
    ProcedureEntryRoute, ProcedureId, ProcedureRoute, Procedures, SubscriptionRoute, Subscriptions,
    Summaries, SummaryClosureRoute, SummaryClosures, SummaryRoute,
};
use evm_abstract::analysis::{AnalysisEngine, AnalysisInput, ExecutionConfig};
use std::collections::BTreeMap;

#[derive(Default)]
struct ProcedureReader<const REQUIRED: bool = false> {
    revisions: BTreeMap<ProcedureId, BTreeSet<usize>>,
    closed: BTreeSet<ProcedureId>,
    continuations: BTreeSet<evm_abstract::analysis::cooperation::ContinuationId>,
    body_generations: BTreeMap<Point, BTreeSet<analysis_kernel::Scope>>,
    payloads: usize,
    navigation: usize,
}
impl<Cx, const REQUIRED: bool> Analyzer<Cx> for ProcedureReader<REQUIRED>
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Read<Procedures, ProcedureRoute, Error = Infrastructure>
        + Read<Callees, CalleeRoute, Error = Infrastructure>
        + Read<ProcedureEntries, ProcedureEntryRoute, Error = Infrastructure>
        + Read<Generations, GenerationRoute, Error = Infrastructure>
        + ReadGeneration<Error = Infrastructure>
        + Read<Subscriptions, SubscriptionRoute, Error = Infrastructure>
        + Discover<Summaries<ExcludingWord>, SummaryRoute, Error = Infrastructure>
        + Read<Summaries<ExcludingWord>, SummaryRoute, Error = Infrastructure>
        + Discover<SummaryClosures, SummaryClosureRoute, Error = Infrastructure>
        + Read<Continuations<ExcludingWord>, ContinuationRoute, Error = Infrastructure>
        + Charges<Error = Infrastructure>,
{
    type Task = Subject;
    type Error = Infrastructure;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        let answer = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        let Some(input) = answer.value() else {
            return Ok(LocalClosure::Stable);
        };
        if subject.point.kind == PointKind::Body {
            self.body_generations
                .entry(subject.point)
                .or_default()
                .insert(subject.scope);
        }
        if subject.point.kind == PointKind::World {
            let callees = <Cx as Read<Callees, CalleeRoute>>::query(
                cx,
                Scoped::new(input.universe(), subject.point),
            )?;
            if let Some(callees) = callees.value() {
                for callee in callees {
                    let info = <Cx as Read<ProcedureEntries, ProcedureEntryRoute>>::query(
                        cx,
                        Scoped::new(input.universe(), *callee),
                    )?;
                    if let Some(info) = info.value() {
                        let entry =
                            current_input::<ExcludingWord, _>(cx, input.universe(), info.entry)?;
                        if let Some(entry) = entry.value() {
                            assert_eq!(
                                entry.payload().active().key.code_address,
                                info.frame.code_address
                            );
                            assert_eq!(
                                entry.payload().active().key.code_hash,
                                info.frame.code_hash
                            );
                            self.navigation += 1;
                        }
                    }
                }
            }
        }
        let ids =
            <Cx as Read<Procedures, ProcedureRoute>>::query(cx, Scoped::new(input.universe(), ()))?;
        if let Some(ids) = ids.value() {
            for id in ids {
                // Both modes accept open MAY outputs and publish no refinement.
                // Required readers must settle without weakening their reads.
                let summary = if REQUIRED {
                    <Cx as Read<Summaries<ExcludingWord>, SummaryRoute>>::query(
                        cx,
                        Scoped::new(input.universe(), *id),
                    )?
                } else {
                    <Cx as Discover<Summaries<ExcludingWord>, SummaryRoute>>::discover(
                        cx,
                        Scoped::new(input.universe(), *id),
                    )?
                };
                if let Some(summary) = summary.value() {
                    cx.charge(summary.work_size())?;
                    let revisions = self.revisions.entry(*id).or_default();
                    assert!(
                        revisions
                            .last()
                            .is_none_or(|previous| *previous <= summary.revision),
                        "native snapshot revisions must not move backwards"
                    );
                    revisions.insert(summary.revision);
                    let proof = <Cx as Discover<SummaryClosures, SummaryClosureRoute>>::discover(
                        cx,
                        Scoped::new(input.universe(), *id),
                    )?;
                    if let Some(proof) = proof.value() {
                        assert_eq!(proof.generation, summary.generation);
                        self.closed.insert(*id);
                    }
                    for output in summary.outputs.values() {
                        // Real native observations, rather than opaque counters.
                        let _ = output.data.len().numeric().observation();
                        self.payloads += 1;
                    }
                }
            }
        }
        let ids = <Cx as Read<Subscriptions, SubscriptionRoute>>::query(
            cx,
            Scoped::new(input.universe(), ()),
        )?;
        if let Some(ids) = ids.value() {
            for id in ids {
                let template =
                    <Cx as Read<Continuations<ExcludingWord>, ContinuationRoute>>::query(
                        cx,
                        Scoped::new(input.universe(), *id),
                    )?;
                if let Some(template) = template.value() {
                    cx.charge(template.work_size())?;
                    assert_eq!(template.caller.kind, PointKind::Body);
                    assert!(template.revision > 0);
                    let _ = template.stack();
                    let _ = template.store();
                    self.continuations.insert(*id);
                }
            }
        }
        Ok(LocalClosure::Stable)
    }
}
#[test]
fn native_summary_and_continuation_keys_are_discovered_tracked_and_repeated_completion_settles() {
    for use_summaries in [false, true] {
        check_procedure_reader::<false>(use_summaries);
    }
}

#[test]
fn required_open_summary_reader_settles_in_each_cache_mode() {
    for (case, use_summaries) in [
        (
            "required_open_summary_reader_settles_without_summaries",
            false,
        ),
        ("required_open_summary_reader_settles_with_summaries", true),
    ] {
        eprintln!("case={case}");
        check_procedure_reader::<true>(use_summaries);
    }
}

fn check_procedure_reader<const REQUIRED: bool>(use_summaries: bool) {
    let fixture = super::super::bridge::fixture::guarded(true);
    let calls = Calls::default();
    let algebra = evm_abstract::domain::ValueAlgebra::new(Policy::new(&calls));
    let mut extension = Extension::new(ProcedureReader::<REQUIRED>::default());
    let result = AnalysisEngine::new(&algebra, &EnginePolicies::default(), &mut extension)
        .run(
            AnalysisInput::new(fixtures::world(&fixture.code), fixtures::entry()).unwrap(),
            ExecutionConfig {
                use_summaries,
                ..fixtures::config(5_000_000_000)
            },
            &Control::default(),
        )
        .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "frontiers={}, first={:?}, work={}",
        result.frontiers().len(),
        result.frontiers().first().map(|frontier| &frontier.reason),
        result.work()
    );
    let reader = &extension.analyzer;
    assert!(!reader.revisions.is_empty());
    assert!(
        reader.navigation > 0,
        "start at an actual World origin and resolve its relative entry Input using only typed public navigation"
    );
    assert!(!reader.continuations.is_empty());
    assert!(reader.payloads > 0);
    assert!(!reader.closed.is_empty());
    assert!(!reader.body_generations.is_empty());
    if REQUIRED {
        // Required first-value reads defer this fixture's native bodies until
        // an initial MAY snapshot arrives. Later native output growth wakes them.
        assert!(
            reader.revisions.values().any(|versions| versions.len() > 1),
            "native output growth must wake the required snapshot readers"
        );
        assert!(
            reader
                .body_generations
                .values()
                .any(|generations| generations.len() > 1)
        );
    }
    // An optional observer can first run after native work settles and coalesce
    // intermediate snapshots. Controlled Read/Discover wakeups and same-value
    // completion are checked by the native_may_growth private regressions.
    assert!(
        reader
            .body_generations
            .values()
            .all(|generations| generations.len() < 100),
        "unchanged completion must not manufacture unbounded universe growth"
    );
    assert!(
        result
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == evm_abstract::analysis::OutcomeKind::Return)
    );
}
