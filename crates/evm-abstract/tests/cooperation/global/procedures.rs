//! Discover native procedure observations without implementation table indices.
use super::*;
use evm_abstract::analysis::cooperation::{
    CalleeRoute, Callees, ContinuationRoute, Continuations, PointKind, ProcedureEntries,
    ProcedureEntryRoute, ProcedureId, ProcedureRoute, Procedures, SubscriptionRoute, Subscriptions,
    Summaries, SummaryRoute,
};
use std::collections::BTreeMap;

#[derive(Default)]
struct ProcedureReader {
    revisions: BTreeMap<ProcedureId, BTreeSet<usize>>,
    closed: BTreeSet<ProcedureId>,
    continuations: BTreeSet<evm_abstract::analysis::cooperation::ContinuationId>,
    body_generations: BTreeMap<Point, BTreeSet<analysis_kernel::Scope>>,
    payloads: usize,
    navigation: usize,
}
impl<Cx> Analyzer<Cx> for ProcedureReader
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Read<Procedures, ProcedureRoute, Error = Infrastructure>
        + Read<Callees, CalleeRoute, Error = Infrastructure>
        + Read<ProcedureEntries, ProcedureEntryRoute, Error = Infrastructure>
        + Read<Generations, GenerationRoute, Error = Infrastructure>
        + ReadGeneration<Error = Infrastructure>
        + Read<Subscriptions, SubscriptionRoute, Error = Infrastructure>
        + Discover<Summaries<ExcludingWord>, SummaryRoute, Error = Infrastructure>
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
                let summary = <Cx as Discover<Summaries<ExcludingWord>, SummaryRoute>>::discover(
                    cx,
                    Scoped::new(input.universe(), *id),
                )?;
                if let Some(summary) = summary.value() {
                    cx.charge(summary.work_size())?;
                    self.revisions
                        .entry(*id)
                        .or_default()
                        .insert(summary.revision);
                    if summary.closed {
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
        let fixture = super::super::bridge::fixture::guarded(true);
        let calls = Calls::default();
        let algebra = evm_abstract::domain::ValueAlgebra::new(Policy::new(&calls));
        let mut extension = Extension::new(ProcedureReader::default());
        let result = analyze_world_cooperating(
            fixtures::world(&fixture.code),
            fixtures::entry(),
            ExecutionConfig {
                max_work: 5_000_000_000,
                root_terminal_coverage: false,
                use_summaries,
                ..ExecutionConfig::default()
            },
            &algebra,
            &EnginePolicies::default(),
            &mut extension,
        )
        .unwrap();
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
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
        assert!(
            reader.revisions.values().any(|versions| versions.len() > 1),
            "output and completion growth must wake real clients"
        );
        assert!(
            reader
                .body_generations
                .values()
                .any(|generations| generations.len() > 1)
        );
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
}
