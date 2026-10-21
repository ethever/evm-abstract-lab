//! A public-API client defines its own routed capability and two components.

#[cfg(test)]
use crate::analysis::testing::{Account, ByteArray, Entry, TestValue, World};
use crate::{
    Fork,
    analysis::{
        EnginePolicies, ExecutionConfig, FrontierReason, Status,
        cooperation::{
            Extension, ExtraStores, InputRoute, Inputs, PairedTasks, PerInput, RefinementRoute,
            Refinements, StackFacts, Subject,
        },
    },
    domain::{NumericValue, ProductAlgebra, ValueAlgebra, WordObservation},
};
use alloy_primitives::{Address, U256};
use analysis_kernel::{
    Accepts, Analyzer, Ascending, Capability, Change, Channel, Charges, Cons, Here, JoinPolicy,
    Kernel, KernelError, LocalClosure, Meter, Nil, Pair, Read, Reduction, Registered, Scoped,
    There,
};
use std::{cell::Cell, rc::Rc};

type Route = There<There<Here>>;
struct KnownStack;
impl Capability for KnownStack {
    type Key = ();
    type Value = Option<U256>;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &Option<U256>) -> usize {
        8
    }
}
struct Fixed;
impl JoinPolicy for Fixed {
    type Value = Option<U256>;
    type Error = ();
    fn work(&self, _: &Self::Value, _: &Self::Value) -> usize {
        16
    }
    fn equivalent(&self, a: &Self::Value, b: &Self::Value) -> bool {
        a == b
    }
    fn join(&self, a: &Self::Value, b: &Self::Value) -> Result<Self::Value, ()> {
        if a == b { Ok(*a) } else { Err(()) }
    }
}
struct Stores(Rc<Cell<usize>>);
impl ExtraStores for Stores {
    type Stores = Cons<Channel<KnownStack, Ascending<Fixed>>, Nil>;
    fn retirement_work(&self, stores: &Self::Stores) -> usize {
        stores.head.len().saturating_mul(16).saturating_add(1)
    }
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        let channel = kernel.channel::<KnownStack, _, _>(Ascending(Fixed), meter)?;
        self.0.set(self.0.get() + 1);
        Ok(Cons::new(channel, Nil))
    }
}
#[derive(Debug)]
enum Failure<K, P> {
    Infrastructure(K),
    Publication(P),
}
struct Provider(Rc<Cell<usize>>);
impl<Cx> Analyzer<Cx> for Provider
where
    Cx: Read<Inputs<NumericValue>, InputRoute>
        + Read<
            Refinements,
            RefinementRoute,
            Error = <Cx as Read<Inputs<NumericValue>, InputRoute>>::Error,
        > + Accepts<KnownStack, Route>
        + Charges<Error = <Cx as Read<Inputs<NumericValue>, InputRoute>>::Error>,
{
    type Task = Subject;
    type Error = Failure<
        <Cx as Read<Inputs<NumericValue>, InputRoute>>::Error,
        <Cx as Accepts<KnownStack, Route>>::Error,
    >;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.charge(16).map_err(Failure::Infrastructure)?;
        let input = <Cx as Read<Inputs<NumericValue>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )
        .map_err(Failure::Infrastructure)?;
        let Some(input) = input.value() else {
            return Ok(LocalClosure::Stable);
        };
        <Cx as Read<Refinements, RefinementRoute>>::query(cx, Scoped::new(subject.scope, ()))
            .map_err(Failure::Infrastructure)?;
        let value = if let Some(value) = input.payload().active().stack.first() {
            cx.charge(value.singleton_work())
                .map_err(Failure::Infrastructure)?;
            value.singleton()
        } else {
            None
        };
        self.0.set(self.0.get() + 1);
        <Cx as Accepts<KnownStack, Route>>::accept(cx, Scoped::new(subject.scope, ()), value)
            .map_err(Failure::Publication)?;
        Ok(LocalClosure::Stable)
    }
}
struct Consumer {
    changed: Rc<Cell<usize>>,
    exhaust: bool,
}
impl<Cx> Reduction<Cx> for Consumer
where
    Cx: Read<KnownStack, Route>
        + Read<Refinements, RefinementRoute, Error = <Cx as Read<KnownStack, Route>>::Error>
        + Accepts<Refinements, RefinementRoute>
        + Charges<Error = <Cx as Read<KnownStack, Route>>::Error>,
{
    type Task = Subject;
    type Error = Failure<
        <Cx as Read<KnownStack, Route>>::Error,
        <Cx as Accepts<Refinements, RefinementRoute>>::Error,
    >;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.charge(16).map_err(Failure::Infrastructure)?;
        if self.exhaust && self.changed.get() > 0 {
            cx.charge(50_000_000).map_err(Failure::Infrastructure)?;
        }
        let known = <Cx as Read<KnownStack, Route>>::query(cx, Scoped::new(subject.scope, ()))
            .map_err(Failure::Infrastructure)?;
        let Some(value) = known.value() else {
            return Ok(LocalClosure::Stable);
        };
        <Cx as Read<Refinements, RefinementRoute>>::query(cx, Scoped::new(subject.scope, ()))
            .map_err(Failure::Infrastructure)?;
        cx.charge(2048).map_err(Failure::Infrastructure)?;
        let facts = value.map_or_else(StackFacts::none, |value| {
            StackFacts::at(0, WordObservation::constant(value))
        });
        if <Cx as Accepts<Refinements, RefinementRoute>>::accept(
            cx,
            Scoped::new(subject.scope, ()),
            facts,
        )
        .map_err(Failure::Publication)?
            == Change::Changed
        {
            self.changed.set(self.changed.get() + 1);
        }
        Ok(LocalClosure::Stable)
    }
}

fn run(exhaust: bool) -> (crate::analysis::WorldAnalysis, usize, usize, usize) {
    let owner = Address::from_word(U256::from(0x101).into());
    let mut world = World::new(Fork::Osaka, "public-extra-store-client");
    let mut account = Account::from_hex("5f5b00", Fork::Osaka).unwrap();
    account.balance = TestValue::constant(U256::from(1_000_000));
    world.insert(owner, account).unwrap();
    let entry = Entry::concrete(
        owner,
        Address::ZERO,
        TestValue::constant(U256::ZERO),
        ByteArray::empty(),
    );
    let factories = Rc::new(Cell::new(0));
    let produced = Rc::new(Cell::new(0));
    let changed = Rc::new(Cell::new(0));
    // Consumer runs first and reads Pending. The framework discovers the Q
    // dependency, wakes it after Provider publication, and closes the cycle.
    let mut extension = Extension {
        analyzer: Pair::new(
            Registered::new(Consumer {
                changed: Rc::clone(&changed),
                exhaust,
            }),
            Provider(Rc::clone(&produced)),
        ),
        tasks: PairedTasks::new(PerInput, PerInput),
        stores: Stores(Rc::clone(&factories)),
        shared: crate::analysis::cooperation::NoStores,
    };
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let result =
        crate::analysis::AnalysisEngine::new(&algebra, &EnginePolicies::default(), &mut extension)
            .run(
                crate::analysis::AnalysisInput::new(world, entry).unwrap(),
                ExecutionConfig {
                    max_work: 10_000_000,
                    root_terminal_coverage: false,
                    ..ExecutionConfig::default()
                },
                &crate::analysis::control::Control::default(),
            )
            .unwrap();
    (result, factories.get(), produced.get(), changed.get())
}

#[test]
fn client_owned_typed_capability_and_components_join_the_production_fixed_point() {
    let (result, factories, produced, changed) = run(false);
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert_eq!(
        factories, 2,
        "fresh typed stores for each immutable World input"
    );
    assert!(produced >= 2);
    assert!(changed > 0);
    assert!(
        result
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&1))
    );
    assert!(result.frontiers().is_empty());
}

#[test]
fn external_ledger_refusal_after_feedback_keeps_the_honest_partial_report() {
    let (result, _, _, changed) = run(true);
    assert!(
        changed > 0,
        "the ledger stops after a sound feedback publication"
    );
    assert_eq!(result.status(), Status::Incomplete);
    assert!(
        result
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::Work)
    );
    assert!(
        result
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&0))
    );
    assert!(result.work() <= result.config().max_work);
}

#[path = "tests/lifetime.rs"]
mod lifetime;
mod readonly;

#[path = "tests/shared.rs"]
mod shared;
