//! Independent global components; communication uses only public typed routes.
use super::{
    carrier::ExcludingWord,
    fixtures,
    policy::{Calls, Policy},
};
use alloy_primitives::U256;
use analysis_kernel::{
    Accepts, Analyzer, Ascending, Capability, Channel, Charges, Cons, Discover, Either, Here,
    JoinPolicy, Kernel, KernelError, LocalClosure, Meter, Nil, Pair, PublishError, Read,
    ReadGeneration, Reduction, Registered, Scoped,
};
use evm_abstract::{
    analysis::{
        EnginePolicies, ExecutionConfig, MachineError, Status,
        cooperation::{
            Direction, Extension, ExtraStores, GenerationRoute, Generations, InputRoute,
            InputTasks, Inputs, Neighbors, Point, RefinementRoute, Refinements, SharedRoute,
            StackFacts, Subject, Topology, TopologyRoute, analyze_world_cooperating, current_input,
        },
    },
    domain::{
        WordElement, WordObservation,
        facts::FactError,
        symbolic::{ExprId, ExprKind},
    },
};
use revm_bytecode::opcode;
use std::{cell::Cell, collections::BTreeSet, convert::Infallible, rc::Rc};

type Infrastructure = KernelError<MachineError>;
#[derive(Debug, thiserror::Error)]
enum Failure {
    #[error("capability operation failed: {0}")]
    Infrastructure(Infrastructure),
    #[error("bound publication failed: {0}")]
    Bound(PublishError<Infallible, MachineError>),
    #[error("feedback publication failed: {0}")]
    Feedback(PublishError<FactError, MachineError>),
}
impl From<Infrastructure> for Failure {
    fn from(error: Infrastructure) -> Self {
        Self::Infrastructure(error)
    }
}

/// A bound for the source point's first word. True excludes 42; false is Top.
/// The aggregation is ascending (true can weaken to false on native growth).
struct Exclusion;
impl Capability for Exclusion {
    type Key = Point;
    type Value = bool;
    fn key_work(_: &Point) -> usize {
        4
    }
    fn value_work(_: &bool) -> usize {
        1
    }
}
struct Cover;
impl JoinPolicy for Cover {
    type Value = bool;
    type Error = Infallible;
    fn work(&self, _: &bool, _: &bool) -> usize {
        1
    }
    fn join(&self, old: &bool, incoming: &bool) -> Result<bool, Infallible> {
        Ok(*old && *incoming)
    }
    fn equivalent(&self, left: &bool, right: &bool) -> bool {
        left == right
    }
}
struct SharedFactory(Rc<Cell<usize>>);
impl ExtraStores for SharedFactory {
    type Stores = Cons<Channel<Exclusion, Ascending<Cover>>, Nil>;
    fn retirement_work(&self, stores: &Self::Stores) -> usize {
        stores.head.len().saturating_mul(8)
    }
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        self.0.set(self.0.get() + 1);
        Ok(Cons::new(kernel.channel(Ascending(Cover), meter)?, Nil))
    }
}
type BoundRoute = SharedRoute<Here>;

/// Provider owns no semantic map; the shared channel carries its native bound.
#[derive(Default)]
struct Provider;
impl<Cx> Analyzer<Cx> for Provider
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Accepts<Exclusion, BoundRoute, Error = PublishError<Infallible, MachineError>>
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
        let excludes = input
            .payload()
            .active()
            .stack
            .first()
            .is_some_and(|word| !word.numeric().contains(U256::from(42)));
        <Cx as Accepts<Exclusion, BoundRoute>>::accept(
            cx,
            Scoped::new(input.universe(), subject.point),
            excludes,
        )
        .map_err(Failure::Bound)?;
        Ok(LocalClosure::Stable)
    }
}

struct Consumer {
    enabled: bool,
    narrowed: BTreeSet<Subject>,
    foreign_values: usize,
}
fn compares(expression: &ExprId, source: &ExprId) -> bool {
    let ExprKind::Operation { opcode: op, args } = expression.kind() else {
        return false;
    };
    *op == opcode::EQ
        && args.len() == 2
        && (0..2).any(|constant| {
            matches!(args[constant].kind(), ExprKind::Constant(word) if *word == U256::from(42))
                && args[1 - constant] == *source
        })
}
impl<Cx> Reduction<Cx> for Consumer
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Read<Generations, GenerationRoute, Error = Infrastructure>
        + ReadGeneration<Error = Infrastructure>
        + Read<Topology, TopologyRoute, Error = Infrastructure>
        + Read<Exclusion, BoundRoute, Error = Infrastructure>
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
        cx.charge(input.work_size().saturating_mul(4))?;
        let stack = &input.payload().active().stack;
        let Some(guard) = stack.get(1).and_then(|word| word.expression()) else {
            return Ok(LocalClosure::Stable);
        };
        let neighbors = <Cx as Read<Topology, TopologyRoute>>::query(
            cx,
            Scoped::new(
                input.universe(),
                Neighbors {
                    point: subject.point,
                    direction: Direction::Predecessors,
                },
            ),
        )?;
        let Some(edges) = neighbors.value() else {
            return Ok(LocalClosure::Stable);
        };
        let mut covered = !edges.is_empty();
        for edge in edges {
            // The native Inputs come from the OTHER point's actual provider.
            let source = current_input::<ExcludingWord, _>(cx, input.universe(), edge.from)?;
            let Some(source) = source.value() else {
                return Ok(LocalClosure::Stable);
            };
            self.foreign_values += 1;
            cx.charge(source.work_size().saturating_mul(4))?;
            let property = <Cx as Read<Exclusion, BoundRoute>>::query(
                cx,
                Scoped::new(input.universe(), edge.from),
            )?;
            let Some(excludes) = property.value() else {
                return Ok(LocalClosure::Stable);
            };
            covered &= *excludes
                && source
                    .payload()
                    .active()
                    .stack
                    .first()
                    .and_then(|word| word.symbolic_expression())
                    .is_some_and(|source| compares(guard, &source));
        }
        // The fixture's only jump to this block is the source's unconditional
        // transfer. Its expression and published bound cover every contributor;
        // no Pending/absent edge is used as an unreachability proof.
        if self.enabled && covered {
            <Cx as Accepts<Refinements, RefinementRoute>>::accept(
                cx,
                Scoped::new(subject.scope, ()),
                StackFacts::at(1, WordObservation::constant(U256::ZERO)),
            )
            .map_err(Failure::Feedback)?;
            self.narrowed.insert(*subject);
        }
        Ok(LocalClosure::Stable)
    }
}
struct Order(bool);
impl InputTasks for Order {
    type Task = Either<Subject, Subject>;
    const ENABLED: bool = true;
    fn registration_work(&self) -> usize {
        4
    }
    fn tasks(&self, subject: Subject) -> impl Iterator<Item = Self::Task> {
        if self.0 {
            [Either::Left(subject), Either::Right(subject)]
        } else {
            [Either::Right(subject), Either::Left(subject)]
        }
        .into_iter()
    }
}

fn fixture() -> (Vec<u8>, usize) {
    let mut code = vec![
        opcode::PUSH0,
        opcode::SLOAD,
        opcode::PUSH2,
        0,
        6,
        opcode::JUMP,
        opcode::JUMPDEST,
        opcode::DUP1,
        opcode::PUSH1,
        42,
        opcode::EQ,
        opcode::PUSH2,
        0,
        15,
        opcode::JUMP,
        opcode::JUMPDEST,
        opcode::PUSH2,
        0,
        0,
        opcode::JUMPI,
        opcode::POP,
        opcode::PUSH1,
        1,
        opcode::PUSH0,
        opcode::MSTORE,
        opcode::PUSH1,
        32,
        opcode::PUSH0,
        opcode::RETURN,
    ];
    let impossible = code.len();
    code[17..19].copy_from_slice(&(impossible as u16).to_be_bytes());
    code.extend([opcode::JUMPDEST, opcode::INVALID]);
    (code, impossible)
}
#[test]
fn independent_global_components_read_foreign_inputs_and_change_the_real_cfg_in_both_orders() {
    for provider_first in [false, true] {
        for enabled in [false, true] {
            let (code, impossible) = fixture();
            let calls = Calls::default();
            let mut policy = Policy::new(&calls);
            policy.exclusion_queries = false;
            let algebra = evm_abstract::domain::ValueAlgebra::new(policy);
            let factories = Rc::new(Cell::new(0));
            let mut extension = Extension::with_tasks(
                Pair::new(
                    Provider,
                    Registered::new(Consumer {
                        enabled,
                        narrowed: BTreeSet::new(),
                        foreign_values: 0,
                    }),
                ),
                Order(provider_first),
            )
            .with_shared_stores(SharedFactory(Rc::clone(&factories)));
            let result = analyze_world_cooperating(
                fixtures::world(&code),
                fixtures::entry(),
                ExecutionConfig {
                    max_work: 5_000_000_000,
                    root_terminal_coverage: false,
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
            assert_eq!(factories.get(), 1, "custom global channel is created once");
            assert_eq!(
                result
                    .states()
                    .iter()
                    .any(|state| state.executed_pcs.contains(&impossible)),
                !enabled
            );
            let consumer = extension.analyzer.right.rule();
            assert!(consumer.foreign_values > 0);
            assert_eq!(!consumer.narrowed.is_empty(), enabled);
            assert!(
                consumer.narrowed.len() < 30,
                "finite cooperation must settle without selector ping-pong"
            );
        }
    }
}

#[path = "global/cycle.rs"]
mod cycle;
#[path = "global/deferred.rs"]
mod deferred;
#[path = "global/entries.rs"]
mod entries;
#[path = "global/failures.rs"]
mod failures;
#[path = "global/procedures.rs"]
mod procedures;
#[path = "global/readiness.rs"]
mod readiness;
#[path = "global/source_premise.rs"]
mod source_premise;
#[path = "global/summary_validity.rs"]
mod summary_validity;
#[path = "global/topology.rs"]
mod topology;
