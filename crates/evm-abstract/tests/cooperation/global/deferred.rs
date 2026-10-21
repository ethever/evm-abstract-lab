//! A later World publication reopens an already-returned relative interpreter.
use super::super::bridge::rule;
use super::*;
use evm_abstract::analysis::{
    FrontierReason, OutcomeKind,
    control::Control,
    cooperation::{
        PointKind, ProcedureRoute, Procedures, Summaries, SummaryRoute,
        analyze_world_cooperating_with_control,
    },
    progress::Observer,
};
use std::collections::BTreeMap;

/// False is a sound EQ(v,42)==0 guarantee; true is the weaker boolean bound.
/// A provider deliberately widens its independently valid invariant after an
/// observed relative solve closes. This legal loss of precision must retire
/// consumers' old must facts even though their raw numeric input is unchanged.
pub(super) struct GuardBound;
impl Capability for GuardBound {
    type Key = ();
    type Value = bool;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &bool) -> usize {
        1
    }
}
pub(super) struct Widen;
impl JoinPolicy for Widen {
    type Value = bool;
    type Error = Infallible;
    fn work(&self, _: &bool, _: &bool) -> usize {
        1
    }
    fn join(&self, old: &bool, incoming: &bool) -> Result<bool, Infallible> {
        Ok(*old || *incoming)
    }
    fn equivalent(&self, left: &bool, right: &bool) -> bool {
        left == right
    }
}
pub(super) struct Factory;
impl ExtraStores for Factory {
    type Stores = Cons<Channel<GuardBound, Ascending<Widen>>, Nil>;
    fn retirement_work(&self, stores: &Self::Stores) -> usize {
        stores.head.len().saturating_mul(8)
    }
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        Ok(Cons::new(kernel.channel(Ascending(Widen), meter)?, Nil))
    }
}
type Route = SharedRoute<Here>;
#[derive(Default)]
pub(super) struct Source {
    pub(super) grown: usize,
    adopt_closed: bool,
}
impl<Cx> Analyzer<Cx> for Source
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Discover<Procedures, ProcedureRoute, Error = Infrastructure>
        + Discover<Summaries<ExcludingWord>, SummaryRoute, Error = Infrastructure>
        + Read<Summaries<ExcludingWord>, SummaryRoute, Error = Infrastructure>
        + Accepts<GuardBound, Route, Error = PublishError<Infallible, MachineError>>
        + Charges<Error = Infrastructure>,
{
    type Task = Subject;
    type Error = Failure;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Failure> {
        if subject.point.kind != PointKind::World {
            return Ok(LocalClosure::Stable);
        }
        let input = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
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
        let mut widen = false;
        if let Some(ids) = ids.value() {
            for id in ids {
                let summary = <Cx as Discover<Summaries<ExcludingWord>, SummaryRoute>>::discover(
                    cx,
                    Scoped::new(input.universe(), *id),
                )?;
                let complete = summary
                    .value()
                    .is_some_and(|summary| summary.closed && !summary.outputs.is_empty());
                if complete && self.adopt_closed {
                    // A real must consumer adopts only an available closed
                    // certificate. Pending has an independent invariant below,
                    // so this feedback cycle does not require its own closure.
                    let adopted = <Cx as Read<Summaries<ExcludingWord>, SummaryRoute>>::query(
                        cx,
                        Scoped::new(input.universe(), *id),
                    )?;
                    if !adopted.value().is_some_and(|summary| summary.closed) {
                        return Ok(LocalClosure::Stable);
                    }
                }
                widen |= complete;
            }
        }
        // Before widening the proof follows from the actual immutable store.
        // The fixture never writes slot0, so it also covers undiscovered calls.
        cx.charge(input.work_size())?;
        widen |= !input
            .payload()
            .store
            .slots()
            .get(&(fixtures::address(fixtures::OWNER), U256::ZERO))
            .is_some_and(|word| !word.numeric().contains(U256::from(42)));
        <Cx as Accepts<GuardBound, Route>>::accept(cx, Scoped::new(input.universe(), ()), widen)
            .map_err(Failure::Bound)?;
        if widen {
            self.grown += 1;
        }
        Ok(LocalClosure::Stable)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    None,
    Work,
    Cancel,
}
struct Target {
    rule: rule::Rule,
    react: bool,
    stop: Stop,
    cancellation: analysis_control::Cancellation,
    after_growth: usize,
    generations: BTreeMap<Point, BTreeSet<analysis_kernel::Scope>>,
}
impl<Cx> Reduction<Cx> for Target
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Read<GuardBound, Route, Error = Infrastructure>
        + Read<Refinements, RefinementRoute, Error = Infrastructure>
        + Accepts<Refinements, RefinementRoute, Error = PublishError<FactError, MachineError>>
        + Charges<Error = Infrastructure>,
{
    type Task = Subject;
    type Error = rule::Failure<Infrastructure, PublishError<FactError, MachineError>>;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        let input = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )
        .map_err(rule::Failure::Infrastructure)?;
        let Some(input) = input.value() else {
            return Ok(LocalClosure::Stable);
        };
        if subject.point.kind == PointKind::Body {
            self.generations
                .entry(subject.point)
                .or_default()
                .insert(subject.scope);
            // This consumer knows only the custom capability. It never reads A's
            // Inputs/selector to simulate the missing provenance/coverage bridge.
            let bound =
                <Cx as Read<GuardBound, Route>>::query(cx, Scoped::new(input.universe(), ()))
                    .map_err(rule::Failure::Infrastructure)?;
            let Some(widen) = bound.value() else {
                return Ok(LocalClosure::Stable);
            };
            if *widen {
                self.after_growth += 1;
                match self.stop {
                    Stop::Work => cx
                        .charge(10_000_000_000)
                        .map_err(rule::Failure::Infrastructure)?,
                    Stop::Cancel => {
                        self.cancellation.cancel();
                        cx.charge(0).map_err(rule::Failure::Infrastructure)?;
                    }
                    Stop::None => {}
                }
                if self.react {
                    return Ok(LocalClosure::Stable);
                }
            }
        }
        self.rule.reduce(subject, cx)
    }
}
fn code() -> (Vec<u8>, usize) {
    let fixture = super::super::bridge::fixture::guarded(true);
    let mut code = fixture.code;
    code.pop(); // Replace the formerly impossible INVALID with a distinct return.
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
    (code, fixture.impossible)
}
#[test]
fn later_world_fact_reenters_body_changes_native_output_and_preserves_failure_causes() {
    for (use_summaries, adopt_closed) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        for (react, stop) in [
            (false, Stop::None),
            (true, Stop::None),
            (true, Stop::Work),
            (true, Stop::Cancel),
        ] {
            let (code, _) = code();
            let calls = Calls::default();
            let mut policy = Policy::new(&calls);
            policy.exclusion_queries = false;
            let algebra = evm_abstract::domain::ValueAlgebra::new(policy);
            let control = Control::new(Observer::default());
            let mut extension = Extension::with_tasks(
                Pair::new(
                    Source {
                        adopt_closed,
                        ..Source::default()
                    },
                    Registered::new(Target {
                        rule: rule::Rule {
                            enabled: true,
                            ..rule::Rule::default()
                        },
                        react,
                        stop,
                        cancellation: control.cancellation().clone(),
                        after_growth: 0,
                        generations: BTreeMap::new(),
                    }),
                ),
                Order(true),
            )
            .with_shared_stores(Factory);
            let result = analyze_world_cooperating_with_control(
                fixtures::world(&code),
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
                &control,
            );
            assert!(
                result.is_ok()
                    || (stop == Stop::Cancel
                        && matches!(
                            result,
                            Err(evm_abstract::analysis::cooperation::CooperationError::Cancelled)
                        )),
                "{result:?}"
            );
            assert!(
                extension.analyzer.left.grown > 0,
                "a later World input must observe a closed relative output then grow the shared bound"
            );
            assert!(
                extension.analyzer.right.rule().after_growth > 0,
                "the existing relative Body must reenter after the later World publication"
            );
            if stop == Stop::Cancel {
                assert!(matches!(
                    result,
                    Err(evm_abstract::analysis::cooperation::CooperationError::Cancelled)
                ));
                continue;
            }
            let result = result.unwrap();
            if stop == Stop::Work {
                assert_eq!(result.status(), Status::Incomplete);
                assert!(
                    result
                        .frontiers()
                        .iter()
                        .any(|frontier| frontier.reason == FrontierReason::Work
                            && (frontier.payload.is_some() || frontier.target.is_some()))
                );
                assert!(result.frontiers().iter().all(|frontier| frontier.reason
                    != FrontierReason::StateJoin(evm_abstract::analysis::JoinBoundary::Shape)));
                continue;
            }
            assert_eq!(
                result.status(),
                Status::Converged,
                "{:?}",
                result.frontiers()
            );
            let callee_can_return_42 = result
                .procedure_summaries()
                .iter()
                .flat_map(|record| &record.procedures)
                .flat_map(|procedure| &procedure.outputs)
                .filter(|output| output.kind == OutcomeKind::Return)
                .any(|output| {
                    output
                        .data
                        .read_word_with(&fixtures::word(0), algebra.borrowed())
                        .contains(U256::from(42))
                });
            assert_eq!(
                callee_can_return_42, react,
                "late invalidation must remove the old guarantee and change actual native callee returns"
            );
            assert!(
                extension
                    .analyzer
                    .right
                    .rule()
                    .generations
                    .values()
                    .any(|generations| generations.len() > 1)
            );
            assert!(
                extension
                    .analyzer
                    .right
                    .rule()
                    .generations
                    .values()
                    .all(|generations| generations.len() < 100)
            );
        }
    }
}
