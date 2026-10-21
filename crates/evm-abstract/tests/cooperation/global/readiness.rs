//! Mandatory producer readiness must resume a native transfer even at Top.
use super::*;
use analysis_kernel::{Availability, Change, Closure, Version};
use evm_abstract::analysis::{
    FrontierReason, OutcomeKind,
    control::Control,
    cooperation::{CooperationError, PointKind, analyze_world_cooperating_with_control},
    progress::Observer,
};
use std::cell::RefCell;

/// A ready guarantee with no additional precision, distinct from Pending.
struct Ready;
impl Capability for Ready {
    type Key = ();
    type Value = StackFacts;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(value: &StackFacts) -> usize {
        value.work_size()
    }
}
struct KeepTop;
impl JoinPolicy for KeepTop {
    type Value = StackFacts;
    type Error = Infallible;
    fn work(&self, old: &StackFacts, incoming: &StackFacts) -> usize {
        old.work_size().saturating_add(incoming.work_size())
    }
    fn join(&self, old: &StackFacts, incoming: &StackFacts) -> Result<StackFacts, Infallible> {
        assert_eq!(old, &StackFacts::none());
        assert_eq!(incoming, &StackFacts::none());
        Ok(StackFacts::none())
    }
    fn equivalent(&self, left: &StackFacts, right: &StackFacts) -> bool {
        left == right
    }
}
struct Factory;
impl ExtraStores for Factory {
    type Stores = Cons<Channel<Ready, Ascending<KeepTop>>, Nil>;
    fn retirement_work(&self, stores: &Self::Stores) -> usize {
        stores.head.len().saturating_mul(8)
    }
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        Ok(Cons::new(kernel.channel(Ascending(KeepTop), meter)?, Nil))
    }
}
type Route = SharedRoute<Here>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Pending(Subject, Version),
    IndependentProducer,
    Published,
    Ready(Subject, Version),
    AfterA,
}
type Trace = Rc<RefCell<Vec<Event>>>;

struct Fixture {
    code: Vec<u8>,
    a: usize,
    b: usize,
    after_a: usize,
    a_jump: usize,
    a_return: usize,
}
fn fixture() -> Fixture {
    // JUMPI registers its true successor A before its independent false
    // successor B. Both native jobs exist before either input is evaluated.
    let mut code = vec![
        opcode::PUSH0,
        opcode::SLOAD,
        opcode::PUSH2,
        0,
        0,
        opcode::JUMPI,
    ];
    let b = code.len();
    code.extend([
        opcode::PUSH1,
        2,
        opcode::PUSH0,
        opcode::MSTORE,
        opcode::PUSH1,
        32,
        opcode::PUSH0,
        opcode::RETURN,
    ]);
    let a = code.len();
    code[3..5].copy_from_slice(&(a as u16).to_be_bytes());
    code.extend([
        opcode::JUMPDEST,
        opcode::PUSH1,
        1,
        opcode::PUSH2,
        0,
        0,
        opcode::JUMP,
    ]);
    let a_jump = code.len() - 1;
    let after_a = code.len();
    code[a + 4..a + 6].copy_from_slice(&(after_a as u16).to_be_bytes());
    code.extend([
        opcode::JUMPDEST,
        opcode::PUSH0,
        opcode::MSTORE,
        opcode::PUSH1,
        32,
        opcode::PUSH0,
        opcode::RETURN,
    ]);
    Fixture {
        code,
        a,
        b,
        after_a,
        a_jump,
        a_return: after_a + 6,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Publish,
    Never,
    Work,
    Cancel,
}
struct Source {
    b: usize,
    mode: Mode,
    trace: Trace,
    cancellation: analysis_control::Cancellation,
    seen: bool,
}
impl<Cx> Analyzer<Cx> for Source
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Discover<Topology, TopologyRoute, Error = Infrastructure>
        + Accepts<Ready, Route, Error = PublishError<Infallible, MachineError>>
        + Charges<Error = Infrastructure>,
{
    type Task = Subject;
    type Error = Failure;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Failure> {
        let answer = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        let Some(input) = answer.value() else {
            return Ok(LocalClosure::Stable);
        };
        cx.charge(input.work_size())?;
        if subject.point.kind != PointKind::World
            || fixtures::input_pc(input).expect("native runtime block") != self.b
        {
            return Ok(LocalClosure::Stable);
        }
        if self.seen {
            return Ok(LocalClosure::Stable);
        }
        self.seen = true;
        // Trace state is an assertion probe only. X is the sole communication
        // channel used to decide readiness; Top is valid for every execution.
        let a = self
            .trace
            .borrow()
            .iter()
            .find_map(|event| match event {
                Event::Pending(subject, _) => Some(*subject),
                _ => None,
            })
            .expect("A must perform its required Pending read before B runs");
        let edges = <Cx as Discover<Topology, TopologyRoute>>::discover(
            cx,
            Scoped::new(
                input.universe(),
                Neighbors {
                    point: a.point,
                    direction: Direction::Successors,
                },
            ),
        )?;
        assert!(
            edges.value().unwrap().is_empty(),
            "native A must not jump while X is Pending"
        );
        self.trace.borrow_mut().push(Event::IndependentProducer);
        match self.mode {
            Mode::Publish => {
                <Cx as Accepts<Ready, Route>>::accept(
                    cx,
                    Scoped::new(input.universe(), ()),
                    StackFacts::none(),
                )
                .map_err(Failure::Bound)?;
                self.trace.borrow_mut().push(Event::Published);
            }
            Mode::Never => {}
            Mode::Work => cx.charge(1_000_000_000).map_err(Failure::Infrastructure)?,
            Mode::Cancel => {
                self.cancellation.cancel();
                cx.charge(0)?;
            }
        }
        Ok(LocalClosure::Stable)
    }
}
struct Target {
    a: usize,
    after_a: usize,
    trace: Trace,
}
impl<Cx> Reduction<Cx> for Target
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Read<Ready, Route, Error = Infrastructure>
        + Read<Topology, TopologyRoute, Error = Infrastructure>
        + Read<Refinements, RefinementRoute, Error = Infrastructure>
        + Accepts<Refinements, RefinementRoute, Error = PublishError<FactError, MachineError>>
        + Charges<Error = Infrastructure>,
{
    type Task = Subject;
    type Error = Failure;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Failure> {
        let answer = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        let Some(input) = answer.value() else {
            return Ok(LocalClosure::Stable);
        };
        cx.charge(input.work_size())?;
        if subject.point.kind != PointKind::World {
            return Ok(LocalClosure::Stable);
        }
        if fixtures::input_pc(input).expect("native runtime block") == self.after_a {
            self.trace.borrow_mut().push(Event::AfterA);
        }
        if fixtures::input_pc(input).expect("native runtime block") != self.a {
            return Ok(LocalClosure::Stable);
        }
        let incoming = <Cx as Read<Topology, TopologyRoute>>::query(
            cx,
            Scoped::new(
                input.universe(),
                Neighbors {
                    point: subject.point,
                    direction: Direction::Predecessors,
                },
            ),
        )?;
        let incoming = incoming.value().unwrap();
        assert_eq!(incoming.len(), 1);
        let root = incoming.iter().next().unwrap().from;
        let branches = <Cx as Read<Topology, TopologyRoute>>::query(
            cx,
            Scoped::new(
                input.universe(),
                Neighbors {
                    point: root,
                    direction: Direction::Successors,
                },
            ),
        )?;
        assert_eq!(
            branches.value().unwrap().len(),
            2,
            "both independent native successors must already be registered"
        );
        let seeded =
            <Cx as Read<Refinements, RefinementRoute>>::query(cx, Scoped::new(subject.scope, ()))?;
        assert_eq!(seeded.value(), Some(&StackFacts::none()));
        let ready = <Cx as Read<Ready, Route>>::query(cx, Scoped::new(input.universe(), ()))?;
        match ready.state {
            Availability::Pending => self
                .trace
                .borrow_mut()
                .push(Event::Pending(*subject, seeded.version)),
            Availability::Value(facts) => {
                assert_eq!(facts, StackFacts::none());
                let change = <Cx as Accepts<Refinements, RefinementRoute>>::accept(
                    cx,
                    Scoped::new(subject.scope, ()),
                    facts,
                )
                .map_err(Failure::Feedback)?;
                assert_eq!(
                    change,
                    Change::Unchanged,
                    "readiness adds no precision or refinement wake"
                );
                self.trace
                    .borrow_mut()
                    .push(Event::Ready(*subject, seeded.version));
            }
            Availability::Absent => panic!("the required provider never resolves absent"),
        }
        Ok(LocalClosure::Stable)
    }
}

type Analysis = Result<
    evm_abstract::analysis::EvmWorldAnalysis<ExcludingWord>,
    CooperationError<Either<Failure, Failure>>,
>;
fn run(
    mode: Mode,
    algebra: &evm_abstract::domain::ValueAlgebra<Policy<'_>>,
) -> (Analysis, Vec<Event>, Fixture) {
    let fixture = fixture();
    let trace = Trace::default();
    let control = Control::new(Observer::default());
    let mut extension = Extension::with_tasks(
        Pair::new(
            Source {
                b: fixture.b,
                mode,
                trace: Rc::clone(&trace),
                cancellation: control.cancellation().clone(),
                seen: false,
            },
            Registered::new(Target {
                a: fixture.a,
                after_a: fixture.after_a,
                trace: Rc::clone(&trace),
            }),
        ),
        Order(true),
    )
    .with_shared_stores(Factory);
    let result = analyze_world_cooperating_with_control(
        fixtures::world(&fixture.code),
        fixtures::entry(),
        fixtures::config(100_000_000),
        algebra,
        &EnginePolicies::default(),
        &mut extension,
        &control,
    );
    let events = trace.borrow().clone();
    (result, events, fixture)
}

#[test]
fn required_pending_defers_native_a_then_equivalent_top_retries_it() {
    let calls = Calls::default();
    let algebra = evm_abstract::domain::ValueAlgebra::new(Policy::new(&calls));
    let (result, events, fixture) = run(Mode::Publish, &algebra);
    let pending = events
        .iter()
        .position(|event| matches!(event, Event::Pending(..)))
        .unwrap();
    let producer = events
        .iter()
        .position(|event| *event == Event::IndependentProducer)
        .unwrap();
    let published = events
        .iter()
        .position(|event| *event == Event::Published)
        .unwrap();
    let ready = events
        .iter()
        .position(|event| matches!(event, Event::Ready(..)))
        .unwrap();
    assert!(
        pending < producer && producer < published && published < ready,
        "{events:?}"
    );
    let Event::Pending(subject, version) = events[pending] else {
        unreachable!()
    };
    assert_eq!(
        events[ready],
        Event::Ready(subject, version),
        "same generation and unchanged seeded Top"
    );
    let result =
        result.expect("a ready unchanged Top must retry the honestly deferred native transfer");
    let after_a = events
        .iter()
        .position(|event| *event == Event::AfterA)
        .unwrap();
    assert!(
        ready < after_a,
        "A's actual successor can only appear after its required provider is ready"
    );
    fixtures::complete(&result);
    assert!(
        result
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&fixture.a_jump))
    );
    assert!(
        result
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&fixture.a_return))
    );
    let returns: BTreeSet<_> = result
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .filter_map(|outcome| {
            outcome
                .data
                .read_word_with(&fixtures::word(0), algebra.borrowed())
                .singleton()
        })
        .collect();
    assert_eq!(returns, BTreeSet::from([U256::ONE, U256::from(2)]));
}

#[test]
fn missing_or_stopped_independent_provider_keeps_native_a_unfinished() {
    let calls = Calls::default();
    let algebra = evm_abstract::domain::ValueAlgebra::new(Policy::new(&calls));
    for mode in [Mode::Never, Mode::Work, Mode::Cancel] {
        let (result, events, _) = run(mode, &algebra);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::Pending(..)))
        );
        assert!(events.contains(&Event::IndependentProducer));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::Published | Event::Ready(..) | Event::AfterA)),
            "{mode:?}: {events:?}"
        );
        match mode {
            Mode::Never => assert!(
                matches!(result, Err(CooperationError::Unfinished(Closure::Open(open))) if open.missing > 0 && open.obligations > 0),
                "missing required X and the deferred native transfer must both remain open"
            ),
            Mode::Work => {
                let result = result.unwrap();
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
            }
            Mode::Cancel => assert!(matches!(result, Err(CooperationError::Cancelled))),
            Mode::Publish => unreachable!(),
        }
    }
}
