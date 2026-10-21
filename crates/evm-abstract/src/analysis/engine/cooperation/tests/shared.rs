//! A client shared table retains local snapshots only until their scope retires.

#[cfg(test)]
use crate::analysis::testing::{Account, ByteArray, Entry, TestValue, World};
use crate::domain::EvmNumericAlgebra as _;
use crate::{
    Fork,
    analysis::{
        EnginePolicies, ExecutionConfig, MachineError, Status,
        cooperation::{Extension, ExtraStores, Input, InputRoute, Inputs, SharedRoute, Subject},
    },
    domain::{NumericValue, ProductAlgebra, ValueAlgebra},
};
use alloy_primitives::{Address, U256};
use analysis_kernel::{
    Accepts, Analyzer, Ascending, Capability, Channel, Charges, Here, JoinPolicy, Kernel,
    KernelError, LocalClosure, Meter, PublishError, Read, ReclaimScope, Scope, Scoped, Select,
    There,
};
use std::{cell::Cell, convert::Infallible, rc::Rc, sync::Weak};

#[derive(Default)]
struct Counts {
    created: Cell<usize>,
    dropped: Cell<usize>,
    peak: Cell<usize>,
    reclaimed: Cell<usize>,
    max_cells: Cell<usize>,
}
struct Snapshot {
    input: Input<NumericValue>,
    counts: Rc<Counts>,
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        self.counts.dropped.set(self.counts.dropped.get() + 1);
    }
}
struct Snapshots;
impl Capability for Snapshots {
    type Key = ();
    type Value = Rc<Snapshot>;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &Self::Value) -> usize {
        2
    }
    fn drop_work(value: &Self::Value) -> usize {
        value.input.work_size().saturating_add(8)
    }
}
struct KeepSnapshot;
impl JoinPolicy for KeepSnapshot {
    type Value = Rc<Snapshot>;
    type Error = Infallible;
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize {
        old.input
            .work_size()
            .saturating_add(incoming.input.work_size())
            .saturating_add(8)
    }
    fn join(&self, old: &Self::Value, _: &Self::Value) -> Result<Self::Value, Infallible> {
        Ok(Rc::clone(old))
    }
    fn equivalent(&self, old: &Self::Value, next: &Self::Value) -> bool {
        old.input.subject() == next.input.subject()
    }
}
struct Marker;
impl Capability for Marker {
    type Key = ();
    type Value = u8;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(_: &u8) -> usize {
        1
    }
}
struct Max;
impl JoinPolicy for Max {
    type Value = u8;
    type Error = Infallible;
    fn work(&self, _: &u8, _: &u8) -> usize {
        1
    }
    fn join(&self, old: &u8, incoming: &u8) -> Result<u8, Infallible> {
        Ok((*old).max(*incoming))
    }
    fn equivalent(&self, old: &u8, next: &u8) -> bool {
        old == next
    }
}
struct Stores {
    snapshots: Channel<Snapshots, Ascending<KeepSnapshot>>,
    marker: Channel<Marker, Ascending<Max>>,
    counts: Rc<Counts>,
}
impl Select<Here> for Stores {
    type Output = Channel<Snapshots, Ascending<KeepSnapshot>>;
    fn select(&self) -> &Self::Output {
        &self.snapshots
    }
    fn select_mut(&mut self) -> &mut Self::Output {
        &mut self.snapshots
    }
}
impl Select<There<Here>> for Stores {
    type Output = Channel<Marker, Ascending<Max>>;
    fn select(&self) -> &Self::Output {
        &self.marker
    }
    fn select_mut(&mut self) -> &mut Self::Output {
        &mut self.marker
    }
}
impl ReclaimScope for Stores {
    fn reclaim_scope<J, M: Meter>(
        &mut self,
        kernel: &mut Kernel<J>,
        retired: Scope,
        meter: &mut M,
    ) -> Result<usize, KernelError<M::Stop>> {
        let removed = self.snapshots.reclaim_scope(kernel, retired, meter)?;
        let world_removed = self.marker.reclaim_scope(kernel, retired, meter)?;
        assert_eq!(
            world_removed, 0,
            "local retirement must preserve world aggregates"
        );
        assert_eq!(self.marker.len(), 1);
        self.counts
            .reclaimed
            .set(self.counts.reclaimed.get() + removed);
        self.counts
            .max_cells
            .set(self.counts.max_cells.get().max(self.snapshots.len()));
        Ok(removed)
    }
}
struct Factory(Rc<Counts>);
impl ExtraStores for Factory {
    type Stores = Stores;
    fn retirement_work(&self, _: &Stores) -> usize {
        0
    } // Each publication prepays disposal.
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Stores, KernelError<M::Stop>> {
        Ok(Stores {
            snapshots: kernel.channel(Ascending(KeepSnapshot), meter)?,
            marker: kernel.channel(Ascending(Max), meter)?,
            counts: Rc::clone(&self.0),
        })
    }
}
#[derive(Debug, thiserror::Error)]
enum Failure {
    #[error("{0}")]
    Kernel(KernelError<MachineError>),
    #[error("{0}")]
    Publication(PublishError<Infallible, MachineError>),
}
impl From<KernelError<MachineError>> for Failure {
    fn from(error: KernelError<MachineError>) -> Self {
        Self::Kernel(error)
    }
}
impl From<PublishError<Infallible, MachineError>> for Failure {
    fn from(error: PublishError<Infallible, MachineError>) -> Self {
        Self::Publication(error)
    }
}
struct Watch {
    counts: Rc<Counts>,
    weak: Vec<Weak<crate::analysis::EvmMachinePayload<NumericValue>>>,
    dead_during_run: usize,
}
impl<Cx> Analyzer<Cx> for Watch
where
    Cx: Read<Inputs<NumericValue>, InputRoute, Error = KernelError<MachineError>>
        + Accepts<Snapshots, SharedRoute<Here>, Error = PublishError<Infallible, MachineError>>
        + Accepts<Marker, SharedRoute<There<Here>>, Error = PublishError<Infallible, MachineError>>
        + Charges<Error = KernelError<MachineError>>,
{
    type Task = Subject;
    type Error = Failure;
    fn evaluate(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Failure> {
        let answer = <Cx as Read<Inputs<NumericValue>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        let Some(input) = answer.value() else {
            return Ok(LocalClosure::Stable);
        };
        cx.charge(input.work_size().saturating_mul(2).saturating_add(32))?;
        self.counts.created.set(self.counts.created.get() + 1);
        self.counts.peak.set(
            self.counts
                .peak
                .get()
                .max(self.counts.created.get() - self.counts.dropped.get()),
        );
        self.weak.push(input.downgrade());
        <Cx as Accepts<Marker, SharedRoute<There<Here>>>>::accept(
            cx,
            Scoped::new(input.universe(), ()),
            1,
        )?;
        <Cx as Accepts<Snapshots, SharedRoute<Here>>>::accept(
            cx,
            Scoped::new(subject.scope, ()),
            Rc::new(Snapshot {
                input: input.clone(),
                counts: Rc::clone(&self.counts),
            }),
        )?;
        self.dead_during_run = self
            .weak
            .iter()
            .filter(|weak| weak.strong_count() == 0)
            .count();
        Ok(LocalClosure::Stable)
    }
}
#[test]
fn client_shared_scoped_payloads_are_reclaimed_across_generations_without_touching_world_values() {
    let owner = Address::from_word(U256::from(0x101).into());
    let mut world = World::new(Fork::Osaka, "shared-client-retirement");
    world
        .insert(
            owner,
            Account::from_hex("5f5b600101600156", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = Entry::concrete(
        owner,
        Address::ZERO,
        TestValue::constant(U256::ZERO),
        ByteArray::empty(),
    );
    let counts = Rc::new(Counts::default());
    let mut extension = Extension::new(Watch {
        counts: Rc::clone(&counts),
        weak: Vec::new(),
        dead_during_run: 0,
    })
    .with_shared_stores(Factory(Rc::clone(&counts)));
    let result = crate::analysis::AnalysisEngine::new(
        &ValueAlgebra::new(ProductAlgebra::default()),
        &EnginePolicies::default(),
        &mut extension,
    )
    .run(
        crate::analysis::AnalysisInput::new(world, entry).unwrap(),
        ExecutionConfig {
            max_work: 1_000_000_000,
            root_terminal_coverage: false,
            ..ExecutionConfig::default()
        },
        &crate::analysis::control::Control::default(),
    )
    .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(counts.reclaimed.get() > 3);
    assert!(
        extension.analyzer.dead_during_run > 0,
        "client-held retired native snapshots must drop during execution"
    );
    assert!(counts.peak.get() <= result.states().len() + 1);
    assert!(counts.max_cells.get() <= result.states().len());
    assert_eq!(counts.created.get(), counts.dropped.get());
    assert!(
        extension
            .analyzer
            .weak
            .iter()
            .all(|weak| weak.strong_count() == 0)
    );
}
