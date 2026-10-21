//! Shared procedure fixtures with fixed engine policies and a numeric algebra.

use super::*;
use crate::{
    analysis::MachinePayload,
    analysis::testing::{Entry, World},
    domain::{AlgebraRef, ProductAlgebra, ValueAlgebra},
};
use std::ops::{Deref, DerefMut};

pub(super) type Solver<'a, D = ProductAlgebra> = EvmSolver<
    'a,
    D,
    crate::analysis::engine::RecentDistinctJumps,
    crate::analysis::engine::FirstResumeStatus,
    crate::analysis::engine::SemanticPolicies,
    crate::analysis::cooperation::NoExtension,
    std::convert::Infallible,
>;

pub(super) struct Table<D: EvmNumericAlgebra = ProductAlgebra> {
    inner: EvmTable<D::Element>,
    pub runtime: Runtime,
    caller: RunningJob,
    domain: ValueAlgebra<D>,
    policies: EnginePolicies,
    cooperation: Cooperation<
        D::Element,
        crate::analysis::cooperation::NoExtension,
        std::convert::Infallible,
    >,
}

impl Default for Table {
    fn default() -> Self {
        Self::with_algebra(ValueAlgebra::new(ProductAlgebra::default()))
    }
}

impl<D: EvmNumericAlgebra> Table<D> {
    pub(super) fn algebra(&self) -> AlgebraRef<'_, D> {
        self.domain.borrowed()
    }

    pub(super) fn with_algebra(domain: ValueAlgebra<D>) -> Self {
        let mut setup = WorkBudget::new(usize::MAX);
        let mut runtime = Runtime::new(&mut setup).unwrap();
        runtime.register_world(0, 1, &mut setup).unwrap();
        let (_, caller) = runtime.next(false, &mut setup).unwrap().unwrap();
        Self {
            inner: EvmTable::default(),
            runtime,
            caller,
            domain,
            policies: EnginePolicies::default(),
            cooperation: Cooperation::new(Default::default()),
        }
    }

    /// Keep the supplied algebra, including its private carrier behavior.
    pub(super) fn solver<'a>(
        &'a mut self,
        world: &'a EvmWorld<D::Element>,
        entry: &'a EvmEntry<D::Element>,
        config: &'a crate::analysis::ExecutionConfig,
        budget: &'a mut WorkBudget,
        counters: &'a mut Counters,
        storage_reads: StorageReadPolicy,
    ) -> Solver<'a, D> {
        Solver {
            table: &mut self.inner,
            world,
            entry,
            config,
            domain: self.domain.borrowed(),
            policies: &self.policies,
            budget,
            counters,
            storage_reads,
            runtime: &mut self.runtime,
            cooperation: &mut self.cooperation,
            active: None,
            caller: Some(&mut self.caller),
            namespace: if config.use_summaries { 0 } else { 1 },
            producer: None,
            deferred: false,
            consumed: None,
        }
    }
}

impl<D: EvmNumericAlgebra> Deref for Table<D> {
    type Target = EvmTable<D::Element>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl<D: EvmNumericAlgebra> DerefMut for Table<D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Table {
    pub(super) fn configure(&mut self, spec: crate::domain::DomainSpec) {
        self.domain = crate::analysis::testing::product(spec);
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "preserved default fixture call convention"
    )]
    pub(super) fn execute(
        &mut self,
        source: usize,
        world: &World,
        entry: &Entry,
        payload: MachinePayload,
        config: &crate::analysis::ExecutionConfig,
        domain: AlgebraRef<'_, ProductAlgebra>,
        budget: &mut WorkBudget,
        counters: &mut Counters,
        storage_reads: StorageReadPolicy,
    ) -> (
        super::super::Execution,
        crate::analysis::ProcedureSummaryRecord,
    ) {
        self.inner.execute_with(
            source,
            world,
            entry,
            payload,
            config,
            domain,
            &self.policies,
            budget,
            counters,
            storage_reads,
            &mut self.runtime,
            &mut self.caller,
            &mut self.cooperation,
        )
    }
}

pub(in crate::analysis) use super::super::initial;
pub(super) use payload::{form, instantiate};

#[expect(
    clippy::too_many_arguments,
    reason = "preserved default fixture call convention"
)]
pub(super) fn execute_inner(
    world: &World,
    entry: &Entry,
    payload: MachinePayload,
    config: &crate::analysis::ExecutionConfig,
    domain: AlgebraRef<'_, ProductAlgebra>,
    budget: &mut WorkBudget,
    storage_reads: StorageReadPolicy,
    procedure: bool,
) -> super::super::Execution {
    execute_inner_with(
        world,
        entry,
        payload,
        config,
        domain,
        &crate::analysis::engine::RecentDistinctJumps,
        budget,
        storage_reads,
        procedure,
    )
}

pub(in crate::analysis) fn execute(
    world: &World,
    entry: &Entry,
    payload: MachinePayload,
    config: &crate::analysis::ExecutionConfig,
    domain: AlgebraRef<'_, ProductAlgebra>,
    budget: &mut WorkBudget,
    storage_reads: StorageReadPolicy,
) -> super::super::Execution {
    super::super::execute(
        world,
        entry,
        payload,
        config,
        domain,
        &crate::analysis::engine::RecentDistinctJumps,
        budget,
        storage_reads,
    )
}
