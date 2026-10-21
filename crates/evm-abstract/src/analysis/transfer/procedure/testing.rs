//! Default-only fixture adapters; production always uses the injected view.

use super::*;
use crate::{
    analysis::MachinePayload,
    domain::{Domain, ValueAlgebra},
    world::{Entry, World},
};
use std::ops::{Deref, DerefMut};

pub(super) type Solver<'a> = EvmSolver<
    'a,
    Domain,
    crate::analysis::engine::RecentDistinctJumps,
    crate::analysis::engine::FirstResumeStatus,
    crate::analysis::engine::SemanticPolicies,
    crate::analysis::engine::cooperation::Empty,
    std::convert::Infallible,
>;

pub(super) struct Table {
    inner: EvmTable<NumericValue>,
    pub runtime: Runtime,
    caller: RunningJob,
    domain: ValueAlgebra<Domain>,
    policies: EnginePolicies,
    cooperation: Cooperation<
        NumericValue,
        crate::analysis::engine::cooperation::Empty,
        std::convert::Infallible,
    >,
}

impl Default for Table {
    fn default() -> Self {
        let mut setup = WorkBudget::new(usize::MAX);
        let mut runtime = Runtime::new(&mut setup).unwrap();
        runtime.register_world(0, 1, &mut setup).unwrap();
        let (_, caller) = runtime.next(false, &mut setup).unwrap().unwrap();
        Self {
            inner: EvmTable::default(),
            runtime,
            caller,
            domain: ValueAlgebra::new(Domain::default()),
            policies: EnginePolicies::default(),
            cooperation: Cooperation::new(Default::default()),
        }
    }
}

impl Deref for Table {
    type Target = EvmTable<NumericValue>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl DerefMut for Table {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Table {
    pub(super) fn solver<'a>(
        &'a mut self,
        world: &'a World,
        entry: &'a Entry,
        config: &'a crate::analysis::ExecutionConfig,
        budget: &'a mut WorkBudget,
        counters: &'a mut Counters,
        storage_reads: StorageReadPolicy,
    ) -> Solver<'a> {
        self.domain = ValueAlgebra::new(config.domain().unwrap());
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
        domain: Domain,
        budget: &mut WorkBudget,
        counters: &mut Counters,
        storage_reads: StorageReadPolicy,
    ) -> (
        super::super::Execution,
        crate::analysis::ProcedureSummaryRecord,
    ) {
        let domain = ValueAlgebra::new(domain);
        self.inner.execute_with(
            source,
            world,
            entry,
            payload,
            config,
            domain.borrowed(),
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

pub(in crate::analysis) fn initial(
    world: &World,
    entry: &Entry,
    domain: Domain,
) -> Result<MachinePayload, FrontierReason> {
    super::super::initial(world, entry, ValueAlgebra::new(domain).borrowed())
}

pub(super) fn form(
    payload: &MachinePayload,
    entry: &Entry,
    domain: Domain,
) -> Result<crate::analysis::state::PayloadSummary, MachineError> {
    payload::form(payload, entry, ValueAlgebra::new(domain).borrowed())
}

pub(super) fn instantiate(
    payload: &crate::analysis::state::PayloadSummary,
    domain: Domain,
) -> Result<MachinePayload, MachineError> {
    instantiate_with(payload, ValueAlgebra::new(domain).borrowed())
}

pub(super) fn join(
    old: &crate::analysis::state::PayloadSummary,
    next: &crate::analysis::state::PayloadSummary,
    domain: Domain,
    widen: bool,
) -> Result<crate::analysis::state::PayloadSummary, MachineError> {
    join_with(old, next, ValueAlgebra::new(domain).borrowed(), widen)
}

#[expect(
    clippy::too_many_arguments,
    reason = "preserved default fixture call convention"
)]
pub(super) fn execute_inner(
    world: &World,
    entry: &Entry,
    payload: MachinePayload,
    config: &crate::analysis::ExecutionConfig,
    domain: Domain,
    budget: &mut WorkBudget,
    storage_reads: StorageReadPolicy,
    procedure: bool,
) -> super::super::Execution {
    execute_inner_with(
        world,
        entry,
        payload,
        config,
        ValueAlgebra::new(domain).borrowed(),
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
    domain: Domain,
    budget: &mut WorkBudget,
    storage_reads: StorageReadPolicy,
) -> super::super::Execution {
    super::super::execute(
        world,
        entry,
        payload,
        config,
        ValueAlgebra::new(domain).borrowed(),
        &crate::analysis::engine::RecentDistinctJumps,
        budget,
        storage_reads,
    )
}
