use super::{
    intervals::Interpreter,
    language::{
        At, ContextId, Error, Hull, Input, Interval, MAX, Outcome, Outcomes, POINTS, PROGRAM,
        Parities, Pc, State, States, Stop, Task, Union, residue,
    },
    parity::Parity,
};
use analysis_kernel::{
    Accepts, Analyzer, Ascending, Channel, Charges, Closure, Cons, Either, Here, JobId, Kernel,
    KernelError, Lift, LocalClosure, Meter, Nil, Pair, Reachable, Scope, ScopeKey, Scoped,
    SolveReport, There, solve,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub struct Ledger {
    pub used: usize,
    pub cancelled: bool,
}
impl Meter for Ledger {
    type Stop = Stop;
    fn reserve(&mut self, work: usize) -> Result<(), Stop> {
        self.used = self.used.checked_add(work).ok_or(Stop::Work)?;
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        if self.cancelled {
            Err(Stop::Cancelled)
        } else {
            Ok(())
        }
    }
}

pub struct Seed;
impl<Cx> Analyzer<Cx> for Seed
where
    Cx: Accepts<States, Error = Error>
        + Accepts<Parities, There<Here>, Error = Error>
        + Accepts<Outcomes, There<There<Here>>, Error = Error>
        + Charges<Error = KernelError<Stop>>,
{
    type Task = Input;
    type Error = Error;
    fn evaluate(&mut self, input: &Input, cx: &mut Cx) -> Result<LocalClosure, Error> {
        cx.charge(512)?;
        // Bottom initializes contribution accumulators, not a claim that a
        // point is permanently unreachable. Only joint closure permits that.
        for pc in POINTS {
            <Cx as Accepts<States>>::accept(cx, Scoped::new(input.scope, pc), Reachable::Bottom)?;
            <Cx as Accepts<Parities, There<Here>>>::accept(cx, Scoped::new(input.scope, pc), 0)?;
        }
        for outcome in [Outcome::Returned, Outcome::Faulted] {
            <Cx as Accepts<Outcomes, There<There<Here>>>>::accept(
                cx,
                Scoped::new(input.scope, outcome),
                Reachable::Bottom,
            )?;
        }
        let words = (0..=MAX).filter(|word| input.words & (1 << word) != 0);
        <Cx as Accepts<States>>::accept(
            cx,
            Scoped::new(input.scope, Pc::Entry),
            Interval::cover(words.clone()),
        )?;
        <Cx as Accepts<Parities, There<Here>>>::accept(
            cx,
            Scoped::new(input.scope, Pc::Entry),
            words.fold(0, |mask, word| mask | residue(word)),
        )?;
        Ok(LocalClosure::Stable)
    }
}

type Stores = Cons<
    Channel<States, Ascending<Lift<Hull>>>,
    Cons<Channel<Parities, Ascending<Union>>, Cons<Channel<Outcomes, Ascending<Lift<Hull>>>, Nil>>,
>;
pub struct Harness {
    pub kernel: Kernel<Task>,
    pub stores: Stores,
    pub analyzers: Pair<Seed, Pair<Interpreter, Parity>>,
    pub meter: Ledger,
    seeds: BTreeMap<Scope, JobId>,
}
impl Harness {
    pub fn new(import_parities: bool, import_intervals: bool, reverse: bool) -> Self {
        let mut meter = Ledger::default();
        let mut kernel = Kernel::new().unwrap();
        let stores = Cons::new(
            kernel.channel(Ascending(Lift(Hull)), &mut meter).unwrap(),
            Cons::new(
                kernel.channel(Ascending(Union), &mut meter).unwrap(),
                Cons::new(
                    kernel.channel(Ascending(Lift(Hull)), &mut meter).unwrap(),
                    Nil,
                ),
            ),
        );
        Self {
            kernel,
            stores,
            meter,
            seeds: BTreeMap::new(),
            analyzers: Pair::new(
                Seed,
                Pair::new(
                    Interpreter {
                        program: PROGRAM,
                        import_parities,
                        reverse,
                        admitted: BTreeSet::new(),
                        edges: BTreeSet::new(),
                        trace: Vec::new(),
                    },
                    Parity {
                        program: PROGRAM,
                        import_intervals,
                        trace: Vec::new(),
                    },
                ),
            ),
        }
    }
    pub fn invocation(&mut self, context: ContextId, words: u8) -> Scope {
        let scope = self
            .kernel
            .scope(
                ScopeKey {
                    context: context.0,
                    generation: 0,
                    policy: 7,
                },
                &mut self.meter,
            )
            .unwrap();
        self.analyzers
            .right
            .left
            .discover(
                At {
                    scope,
                    pc: Pc::Entry,
                },
                &mut self.kernel,
                &mut self.meter,
            )
            .unwrap();
        // Consumers run before their initial facts, in both component orders.
        let seed = self
            .kernel
            .register(scope, Either::Left(Input { scope, words }), &mut self.meter)
            .unwrap();
        self.seeds.insert(scope, seed);
        self.kernel.seal_scope(scope, &mut self.meter).unwrap();
        scope
    }
    pub fn solve(&mut self, steps: usize) -> SolveReport {
        solve(
            &mut self.kernel,
            &mut self.analyzers,
            &mut self.stores,
            &mut self.meter,
            steps,
        )
        .unwrap()
    }
    pub fn snapshot(&mut self, scope: Scope) -> Snapshot {
        assert_eq!(self.kernel.closure(), Closure::Closed);
        let mut run = self
            .kernel
            .begin(self.seeds[&scope], &mut self.meter)
            .unwrap();
        // Pay for scanning the client trace and copying only this scope's edges.
        run.reserve(
            self.analyzers
                .right
                .left
                .edges
                .len()
                .saturating_add(1)
                .saturating_mul(16),
        )
        .unwrap();
        let edges = self
            .analyzers
            .right
            .left
            .edges
            .iter()
            .filter(|edge| edge.scope == scope)
            .map(|edge| edge.key)
            .collect();
        let states = std::array::from_fn(|i| {
            run.query(&mut self.stores.head, Scoped::new(scope, POINTS[i]))
                .unwrap()
                .value()
                .unwrap()
                .clone()
        });
        let parities = std::array::from_fn(|i| {
            *run.query(&mut self.stores.tail.head, Scoped::new(scope, POINTS[i]))
                .unwrap()
                .value()
                .unwrap()
        });
        let returned = run
            .query(
                &mut self.stores.tail.tail.head,
                Scoped::new(scope, Outcome::Returned),
            )
            .unwrap()
            .value()
            .unwrap()
            .clone();
        let faulted = run
            .query(
                &mut self.stores.tail.tail.head,
                Scoped::new(scope, Outcome::Faulted),
            )
            .unwrap()
            .value()
            .unwrap()
            .clone();
        run.finish(LocalClosure::Stable).unwrap();
        assert_eq!(self.kernel.closure(), Closure::Closed);
        Snapshot {
            edges,
            states,
            parities,
            returned,
            faulted,
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub edges: BTreeSet<(Pc, Pc)>,
    pub states: [State; 5],
    pub parities: [u8; 5],
    pub returned: State,
    pub faulted: State,
}
