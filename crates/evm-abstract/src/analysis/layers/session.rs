//! Versioned observations are applied only when constructing a new initial machine.

use super::{
    CatalogError, CodeCatalog, CodeId, CodeMode, CodeObservation, FactDependencies, FactError,
    FactKey, FactRevision, Observation, Observed, SnapshotId, StateFacts,
};
use crate::{
    analysis::{
        AnalysisEngine, AnalysisInput, ConfigError, ContextPolicy, EvmWorldAnalysis,
        ExecutionConfig, PartitionPolicy, StatePolicy,
        control::{Cancelled, Control},
        cooperation::{Composition, CooperationError},
        engine::{Counters, EvmExecutionReuse},
        transfer::StorageReadPolicy,
    },
    domain::{EvmNumericAlgebra, EvmValue, WordElement},
    resource::WorkBudget,
    world::{Code, EvmAccount, EvmWorld, Existence, WorldError},
};
use std::{collections::BTreeSet, sync::Arc};
use thiserror::Error;

pub use crate::analysis::engine::ReuseStats;

/// Concrete missing initial observations requested by discovery execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FactDemand {
    /// Missing account code, including an unresolved delegation implementation.
    Code(crate::Address),
    /// Missing initial slot, independently of the executing code address.
    Storage {
        /// Concrete persistent-storage owner, which can differ from code address.
        address: crate::Address,
        /// Full-width initial storage key required before execution continues.
        slot: crate::U256,
    },
}

/// Explicit effect of injecting observations into the same initial snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefinementEffect {
    /// No new observation was supplied; cached results remain valid.
    Unchanged {
        /// Existing revision, retained exactly.
        revision: FactRevision,
    },
    /// No graph has run yet; the first execution will consume the new facts.
    InitialFacts {
        /// Previous immutable revision.
        previous: FactRevision,
        /// New immutable revision.
        current: FactRevision,
        /// Explicit locations added by this batch.
        added: Vec<FactKey>,
    },
    /// A fresh EVM execution is required. Old reports keep their original
    /// facts, runtime writes, historical roots, and rollback checkpoints.
    RestartRequired {
        /// Revision consumed by the invalidated graph or previous fact batch.
        previous: FactRevision,
        /// Revision that produced the indexed graph. Several injections can
        /// accumulate before the next execution; they never reset invalidation.
        executed_revision: FactRevision,
        /// Facts that the next root execution must consume.
        current: FactRevision,
        /// Explicit locations added by this batch.
        added: Vec<FactKey>,
        /// Conservative read index from the previous execution.
        dependencies: FactDependencies,
        /// Direct and transitively affected graph states. This is invalidation
        /// evidence, not permission to transplant unaffected Store payloads.
        affected_states: BTreeSet<usize>,
    },
}

/// An immutable EVM result and the exact fact revision that produced it.
#[derive(Debug)]
pub struct SessionAnalysis<N: WordElement> {
    snapshot: SnapshotId,
    revision: FactRevision,
    entry_code: Option<CodeId>,
    analysis: EvmWorldAnalysis<N>,
    dependencies: FactDependencies,
    reuse: ReuseStats,
    demand_driven: bool,
}

impl<N: WordElement> SessionAnalysis<N> {
    /// Snapshot whose initial-state set was analyzed.
    pub fn snapshot(&self) -> &SnapshotId {
        &self.snapshot
    }

    /// Exact immutable observation version, independent of subsequent injection.
    pub fn revision(&self) -> FactRevision {
        self.revision
    }

    /// Compiled source identity, including runtime/initcode mode.
    pub fn entry_code(&self) -> Option<CodeId> {
        self.entry_code
    }

    /// Native execution retains ownership, static-call and rollback semantics.
    pub fn analysis(&self) -> &EvmWorldAnalysis<N> {
        &self.analysis
    }

    /// Initial reads include omitted/negative facts and interaction wildcards.
    pub fn dependencies(&self) -> &FactDependencies {
        &self.dependencies
    }

    /// Cumulative pure computation reuse; runtime state is always fresh.
    pub fn reuse(&self) -> ReuseStats {
        self.reuse
    }

    /// Missing concrete observations. Unknown target/key sets remain abstract.
    pub fn demands(&self) -> BTreeSet<FactDemand> {
        self.analysis
            .frontiers()
            .iter()
            .filter_map(|frontier| match frontier.reason {
                crate::analysis::FrontierReason::MissingCode(address) => {
                    Some(FactDemand::Code(address))
                }
                crate::analysis::FrontierReason::MissingStorage { address, slot } => {
                    Some(FactDemand::Storage { address, slot })
                }
                _ => None,
            })
            .collect()
    }

    /// Whether finite missing observations were requested before abstraction.
    pub fn demand_driven(&self) -> bool {
        self.demand_driven
    }
}

/// Incoherent inputs fail before publishing a new fact revision or report.
#[derive(Debug, Error)]
pub enum SessionError<E> {
    /// Failure of the actual borrowed engine or analyzer assembly.
    #[error("{0}")]
    Cooperation(#[from] CooperationError<E>),
    /// Required input projection could not be paid from the cumulative ledger.
    #[error("session preparation work exhausted")]
    Work,
    /// Cooperative cancellation of execution or session preparation.
    #[error("{0}")]
    Cancelled(Cancelled),
    /// Invalid execution configuration or root environment.
    #[error("{0}")]
    Config(ConfigError),
    /// Invalid catalog contents.
    #[error("{0}")]
    Catalog(CatalogError),
    /// Rejected atomic initial-fact batch.
    #[error("{0}")]
    Facts(FactError),
    /// A fact references bytes that were not supplied to this offline catalog.
    #[error("compiled code {0:?} is missing from the catalog")]
    MissingCode(CodeId),
    /// Installed account code must be runtime; initcode needs a CREATE frame.
    #[error("code {0:?} is initcode, not installed runtime")]
    InitcodeBinding(CodeId),
    /// Entry code and explicitly observed code must describe the same source.
    #[error("entry code conflicts with initial observation at {0}")]
    EntryCode(crate::Address),
    /// Snapshot fork or account invariants failed during initial projection.
    #[error("{0}")]
    World(WorldError),
    /// An adapter seed may carry header observations, never account state.
    #[error("initial seed must be empty and match the fact snapshot and catalog fork")]
    Seed,
}

impl<E> From<Cancelled> for SessionError<E> {
    fn from(error: Cancelled) -> Self {
        Self::Cancelled(error)
    }
}

impl<E> From<ConfigError> for SessionError<E> {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl<E> From<CatalogError> for SessionError<E> {
    fn from(error: CatalogError) -> Self {
        Self::Catalog(error)
    }
}

impl<E> From<FactError> for SessionError<E> {
    fn from(error: FactError) -> Self {
        Self::Facts(error)
    }
}

impl<E> From<WorldError> for SessionError<E> {
    fn from(error: WorldError) -> Self {
        Self::World(error)
    }
}

/// One root execution over a shared code catalog and immutable initial facts.
///
/// There is no provider or RPC path here. Refinements create fresh runtime state
/// under cumulative budgets. Certified stack-only computation can be reused;
/// no historical Store or checkpoint is ever changed. General stateful selective
/// continuation is not claimed by the narrow pure-block reuse mechanism.
pub struct AnalysisSession<'a, D: EvmNumericAlgebra, C, P, S, X> {
    engine: AnalysisEngine<'a, D, C, P, S, X>,
    catalog: Arc<CodeCatalog>,
    input: AnalysisInput<D::Element>,
    config: ExecutionConfig,
    budget: Option<WorkBudget>,
    counters: Counters,
    has_executed: bool,
    executed_revision: Option<FactRevision>,
    pending_changes: BTreeSet<FactKey>,
    latest: Option<Arc<SessionAnalysis<D::Element>>>,
    reuse: EvmExecutionReuse<D::Element>,
}

impl<'a, D, C, P, S, X> AnalysisSession<'a, D, C, P, S, X>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element>,
{
    /// Freeze the actual borrowed policies over a validated immutable request.
    /// Catalog failures are surfaced; a code collision cannot silently discard
    /// source identity or switch execution routes.
    pub fn new(
        engine: AnalysisEngine<'a, D, C, P, S, X>,
        input: AnalysisInput<D::Element>,
        config: ExecutionConfig,
    ) -> Result<Self, SessionError<<X as Composition<D::Element>>::Error>> {
        config.validate()?;
        engine
            .algebra
            .spec()
            .validate()
            .map_err(ConfigError::from)?;
        let mut catalog = CodeCatalog::new(input.world.fork());
        for account in input.world.accounts().values() {
            if let Code::Runtime(program) = &account.code {
                let (next, _) = catalog.with_runtime(program.clone())?;
                catalog = next;
            }
        }
        let catalog = Arc::new(catalog);
        Ok(Self {
            reuse: EvmExecutionReuse::new(
                Arc::clone(&catalog),
                config.analysis.max_states,
                config.max_work,
                config.max_memory_bytes,
            ),
            engine,
            catalog,
            input,
            config,
            budget: None,
            counters: Counters::default(),
            has_executed: false,
            executed_revision: None,
            pending_changes: BTreeSet::new(),
            latest: None,
        })
    }
    /// Original abstract assumptions and symbolic input source.
    pub fn base(&self) -> &EvmWorld<D::Element> {
        &self.input.world
    }

    /// Shared compiled semantics, independent of this session's snapshot.
    pub fn catalog(&self) -> &Arc<CodeCatalog> {
        &self.catalog
    }

    /// Current immutable initial observations.
    pub fn facts(&self) -> &StateFacts {
        &self.input.facts
    }

    /// Compile additional code without asserting which account supplies it.
    pub fn supply_runtime(
        &mut self,
        program: crate::bytecode::Program,
    ) -> Result<CodeId, SessionError<<X as Composition<D::Element>>::Error>> {
        let (catalog, id) = self.catalog.with_runtime(program)?;
        self.catalog = Arc::new(catalog);
        self.reuse.set_catalog(Arc::clone(&self.catalog));
        Ok(id)
    }

    fn entry_code(&self) -> Option<CodeId> {
        self.input
            .world
            .runtime(self.input.entry.address)
            .and_then(|p| {
                self.catalog
                    .source(alloy_primitives::keccak256(p.bytes()), CodeMode::Runtime)
            })
            .map(|code| code.id())
    }
    /// Execute this revision or share its unchanged immutable report.
    pub fn run(
        &mut self,
        control: &Control,
    ) -> Result<Arc<SessionAnalysis<D::Element>>, SessionError<<X as Composition<D::Element>>::Error>>
    {
        self.run_policy(control, StorageReadPolicy::Abstract)
    }
    /// Request concrete missing initial code/slots without performing I/O.
    pub fn run_with_demands(
        &mut self,
        control: &Control,
    ) -> Result<Arc<SessionAnalysis<D::Element>>, SessionError<<X as Composition<D::Element>>::Error>>
    {
        self.run_policy(control, StorageReadPolicy::Discover)
    }
    fn run_policy(
        &mut self,
        control: &Control,
        policy: StorageReadPolicy,
    ) -> Result<Arc<SessionAnalysis<D::Element>>, SessionError<<X as Composition<D::Element>>::Error>>
    {
        control.scope(|| {
            control.checkpoint()?;
            // Bind the persistent ledger to its first real operation's control.
            let budget = self
                .budget
                .get_or_insert_with(|| WorkBudget::new(self.config.max_work));
            if crate::analysis::MachineError::check_cancelled().is_err() || !budget.charge(0) {
                return Err(SessionError::Cancelled(Cancelled));
            }
            if let Some(latest) = &self.latest
                && latest.demand_driven == (policy == StorageReadPolicy::Discover)
            {
                return Ok(Arc::clone(latest));
            }
            self.counters.set_observer(control.observer());
            let analysis = self.run_metered(policy)?;
            control.checkpoint()?;
            let report = Arc::new(SessionAnalysis {
                snapshot: self.input.snapshot().clone(),
                revision: self.input.revision(),
                entry_code: self.entry_code(),
                analysis,
                dependencies: self.reuse.dependencies.clone(),
                reuse: self.reuse.stats(),
                demand_driven: policy == StorageReadPolicy::Discover,
            });
            self.latest = Some(Arc::clone(&report));
            Ok(report)
        })
    }
    fn run_metered(
        &mut self,
        policy: StorageReadPolicy,
    ) -> Result<EvmWorldAnalysis<D::Element>, SessionError<<X as Composition<D::Element>>::Error>>
    {
        let entry_code = self.entry_code();
        let budget = self
            .budget
            .as_mut()
            .expect("run_policy establishes the persistent ledger");
        // Pay traversal and actual native clones before any copy/query. The base
        // keeps all abstract assumptions; facts only restrict this same request.
        super::payment::pay_input(&self.input, budget).map_err(|error| match error {
            crate::resource::CloneError::Cancelled => SessionError::Cancelled(Cancelled),
            crate::resource::CloneError::Work => SessionError::Work,
        })?;
        let world = project_world(
            &self.catalog,
            &self.input.facts,
            &self.input.world,
            entry_code,
            self.input.entry.address,
            budget,
        )?;
        let input = AnalysisInput {
            world,
            entry: self.input.entry.clone(),
            facts: self.input.facts.clone(),
        };
        let analysis = self.engine.run_metered(
            input,
            self.config.clone(),
            budget,
            &mut self.counters,
            policy,
            Some(&mut self.reuse),
        )?;
        self.has_executed = true;
        self.executed_revision = Some(self.input.revision());
        self.pending_changes.clear();
        self.latest = None;
        Ok(analysis)
    }
    pub(in crate::analysis) fn run_uncached(
        &mut self,
        control: &Control,
        policy: StorageReadPolicy,
    ) -> Result<EvmWorldAnalysis<D::Element>, SessionError<<X as Composition<D::Element>>::Error>>
    {
        control.scope(|| {
            control.checkpoint()?;
            self.budget
                .get_or_insert_with(|| WorkBudget::new(self.config.max_work));
            self.counters.set_observer(control.observer());
            let result = self.run_metered(policy)?;
            control.checkpoint()?;
            Ok(result)
        })
    }
    pub(in crate::analysis) fn reserve_acquisition(
        &mut self,
        work: usize,
    ) -> Option<crate::analysis::FrontierReason> {
        use crate::analysis::{FrontierReason, Limit};
        if self.counters.states >= self.config.analysis.max_states {
            return Some(FrontierReason::Budget(Limit::States));
        }
        if self.counters.transfers >= self.config.analysis.max_transfers {
            return Some(FrontierReason::Budget(Limit::Transfers));
        }
        let budget = self
            .budget
            .get_or_insert_with(|| WorkBudget::new(self.config.max_work));
        if work == usize::MAX || budget.exhausted() || !budget.charge(work) {
            Some(FrontierReason::Work)
        } else {
            None
        }
    }
    /// Cumulative execution work, including discarded refinement rounds.
    pub fn work_used(&self) -> usize {
        self.budget.as_ref().map_or(0, WorkBudget::used)
    }
    /// Cumulative state allocations, including invalidated graphs.
    pub fn states_created(&self) -> usize {
        self.counters.states
    }

    /// Install a monotone initial-fact batch, with explicit graph invalidation.
    /// Current runtime state and previous reports cannot be patched by this API.
    pub fn refine(
        &mut self,
        snapshot: &SnapshotId,
        observations: impl IntoIterator<Item = Observation>,
        control: &Control,
    ) -> Result<RefinementEffect, SessionError<<X as Composition<D::Element>>::Error>> {
        control.scope(|| {
            control.checkpoint()?;
            let budget = self
                .budget
                .get_or_insert_with(|| WorkBudget::new(self.config.max_work));
            crate::resource::clone_payment::checkpoint(budget).map_err(session_clone_error)?;
            let result = self.refine_scoped(snapshot, observations)?;
            control.checkpoint()?;
            Ok(result)
        })
    }
    fn refine_scoped(
        &mut self,
        snapshot: &SnapshotId,
        observations: impl IntoIterator<Item = Observation>,
    ) -> Result<RefinementEffect, SessionError<<X as Composition<D::Element>>::Error>> {
        let previous = self.input.facts.revision();
        let refinement = self.input.facts.refine(snapshot, observations)?;
        if refinement.added().is_empty() {
            return Ok(RefinementEffect::Unchanged { revision: previous });
        }
        validate_entry(
            &self.catalog,
            refinement.facts(),
            self.entry_code(),
            self.input.entry.address,
        )?;
        let budget = self
            .budget
            .get_or_insert_with(|| WorkBudget::new(self.config.max_work));
        validate_refinement(&self.catalog, &self.input.world, refinement.facts(), budget)?;
        let current = refinement.facts().revision();
        let added = refinement.added().to_vec();
        crate::resource::clone_payment::checkpoint(
            self.budget
                .as_ref()
                .expect("refine establishes the retained ledger"),
        )
        .map_err(session_clone_error)?;
        self.input.facts = refinement.into_facts();
        self.latest = None;
        if self.has_executed {
            let dependencies = self.reuse.dependencies.clone();
            self.pending_changes.extend(added.iter().copied());
            let affected_states = dependencies
                .affected_states(&self.pending_changes.iter().copied().collect::<Vec<_>>());
            Ok(RefinementEffect::RestartRequired {
                previous,
                executed_revision: self
                    .executed_revision
                    .expect("executed sessions retain their revision"),
                current,
                added,
                dependencies,
                affected_states,
            })
        } else {
            Ok(RefinementEffect::InitialFacts {
                previous,
                current,
                added,
            })
        }
    }
}

fn runtime<E>(catalog: &CodeCatalog, id: CodeId) -> Result<Code, SessionError<E>> {
    if id.mode() != CodeMode::Runtime {
        return Err(SessionError::InitcodeBinding(id));
    }
    let program = catalog.program(id).ok_or(SessionError::MissingCode(id))?;
    Ok(if program.byte_len() == 0 {
        Code::Empty
    } else {
        Code::Runtime(program.clone())
    })
}

fn validate_entry<E>(
    catalog: &CodeCatalog,
    facts: &StateFacts,
    entry_code: Option<CodeId>,
    address: crate::Address,
) -> Result<(), SessionError<E>> {
    let Some(entry_code) = entry_code else {
        return Ok(());
    };
    let supplied = runtime(catalog, entry_code)?;
    let observed = match facts.code(address) {
        Observed::Unobserved => return Ok(()),
        Observed::Known(CodeObservation::Runtime(id)) => runtime(catalog, id)?,
        Observed::Known(CodeObservation::Empty) => Code::Empty,
        Observed::Known(CodeObservation::Delegation(target)) => Code::Delegation(target),
    };
    if observed != supplied {
        return Err(SessionError::EntryCode(address));
    }
    Ok(())
}

fn project_world<N: WordElement, E>(
    catalog: &CodeCatalog,
    facts: &StateFacts,
    seed: &EvmWorld<N>,
    entry_code: Option<CodeId>,
    entry_address: crate::Address,
    budget: &mut WorkBudget,
) -> Result<EvmWorld<N>, SessionError<E>> {
    let mut addresses: BTreeSet<_> = facts.observations().map(|(key, _)| key.address()).collect();
    addresses.extend(seed.accounts().keys().copied());
    addresses.insert(entry_address);
    let mut accounts = std::collections::BTreeMap::new();
    for address in addresses {
        let mut account = seed
            .account(address)
            .cloned()
            .unwrap_or_else(EvmAccount::unknown);
        if let Observed::Known(exists) = facts.existence(address) {
            let observed = if exists {
                Existence::Present
            } else {
                Existence::Absent
            };
            if account.existence != Existence::Unknown && account.existence != observed {
                return Err(conflict(FactKey::Existence(address)));
            }
            account.existence = observed;
        }
        account.balance = intersect(
            &account.balance,
            facts.balance(address),
            FactKey::Balance(address),
            budget,
        )?;
        account.nonce = intersect(
            &account.nonce,
            facts.nonce(address),
            FactKey::Nonce(address),
            budget,
        )?;
        if facts.storage_complete(address) {
            account.storage_unknown = false;
        }
        if let Observed::Known(code) = facts.code(address) {
            let code = match code {
                CodeObservation::Empty => Code::Empty,
                CodeObservation::Delegation(target) => Code::Delegation(target),
                CodeObservation::Runtime(id) => runtime(catalog, id)?,
            };
            let both_empty = |code: &Code| {
                matches!(code, Code::Empty)
                    || matches!(code, Code::Runtime(program) if program.byte_len() == 0)
            };
            if account.code != Code::Unknown
                && account.code != code
                && !(both_empty(&account.code) && both_empty(&code))
            {
                return Err(conflict(FactKey::Code(address)));
            }
            if account.code == Code::Unknown {
                account.code = code;
            }
        }
        for (slot, value) in &mut account.storage {
            *value = intersect(
                value,
                facts.storage(address, *slot),
                FactKey::Storage {
                    address,
                    slot: *slot,
                },
                budget,
            )?;
        }
        // Binding executable source never fills any other account component.
        if address == entry_address
            && let Some(entry_code) = entry_code
        {
            account.code = runtime(catalog, entry_code)?;
        }
        accounts.insert(address, account);
    }
    // Each slot is visited once, rather than scanning the full table per account.
    for (_, observation) in facts.observations() {
        if let Observation::Storage {
            address,
            slot,
            value,
        } = observation
        {
            let account = accounts
                .get_mut(address)
                .expect("each observed address was collected");
            let base = seed.account(*address);
            let initial = base
                .and_then(|a| a.storage.get(slot))
                .cloned()
                .unwrap_or_else(|| {
                    if base.is_some_and(|a| !a.storage_unknown || a.existence == Existence::Absent)
                    {
                        EvmValue::constant(crate::U256::ZERO)
                    } else {
                        EvmValue::top()
                    }
                });
            account.storage.insert(
                *slot,
                intersect(
                    &initial,
                    Observed::Known(*value),
                    FactKey::Storage {
                        address: *address,
                        slot: *slot,
                    },
                    budget,
                )?,
            );
        }
    }
    Ok(seed.with_initial_accounts(accounts)?)
}

fn conflict<E>(key: FactKey) -> SessionError<E> {
    FactError::Conflict { key }.into()
}

fn intersect<N: WordElement, E>(
    base: &EvmValue<N>,
    observed: Observed<crate::U256>,
    key: FactKey,
    budget: &mut WorkBudget,
) -> Result<EvmValue<N>, SessionError<E>> {
    let Observed::Known(value) = observed else {
        return Ok(base.clone());
    };
    let observation = crate::domain::WordObservation::constant(value);
    let work = base
        .clone_work()
        .saturating_add(base.work_size())
        .saturating_add(base.numeric().constrain_work(&observation));
    crate::resource::clone_payment::pay(budget, work).map_err(|error| match error {
        crate::resource::CloneError::Cancelled => SessionError::Cancelled(Cancelled),
        crate::resource::CloneError::Work => SessionError::Work,
    })?;
    if !base.contains(value) {
        return Err(conflict(key));
    }
    let numeric = base
        .numeric()
        .constrain(&observation)
        .map_err(|_| conflict(key))?;
    crate::resource::clone_payment::checkpoint(budget)
        .map_err(|_| SessionError::Cancelled(Cancelled))?;
    Ok(base.clone().with_numeric(numeric))
}

/// Validate a complete proposed revision against the original request before
/// committing it. Native constraints stay on N; no observation round trip can
/// erase hidden carrier predicates or authorize an unpriced opaque query.
fn validate_refinement<N: WordElement, E>(
    catalog: &CodeCatalog,
    seed: &EvmWorld<N>,
    facts: &StateFacts,
    budget: &mut WorkBudget,
) -> Result<(), SessionError<E>> {
    for (key, observation) in facts.observations() {
        if let Some(account) = seed.account(key.address()) {
            match observation {
                Observation::Code { address, code } => {
                    let observed = match code {
                        CodeObservation::Empty => Code::Empty,
                        CodeObservation::Delegation(target) => Code::Delegation(*target),
                        CodeObservation::Runtime(id) => runtime(catalog, *id)?,
                    };
                    let empty = |code: &Code| {
                        matches!(code, Code::Empty)
                            || matches!(code, Code::Runtime(program) if program.byte_len() == 0)
                    };
                    if account.code != Code::Unknown
                        && account.code != observed
                        && !(empty(&account.code) && empty(&observed))
                    {
                        return Err(conflict(FactKey::Code(*address)));
                    }
                }
                Observation::Balance { value, .. } => {
                    intersect(&account.balance, Observed::Known(*value), *key, budget)?;
                }
                Observation::Nonce { value, .. } => {
                    intersect(&account.nonce, Observed::Known(*value), *key, budget)?;
                }
                Observation::Storage { slot, value, .. } => {
                    if let Some(base) = account.storage.get(slot) {
                        intersect(base, Observed::Known(*value), *key, budget)?;
                    } else if !account.storage_unknown && *value != crate::U256::ZERO {
                        return Err(conflict(*key));
                    }
                }
                Observation::Existence { exists, .. } => {
                    if account.existence != Existence::Unknown
                        && (*exists != (account.existence == Existence::Present))
                    {
                        return Err(conflict(*key));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn session_clone_error<E>(error: crate::resource::CloneError) -> SessionError<E> {
    match error {
        crate::resource::CloneError::Cancelled => SessionError::Cancelled(Cancelled),
        crate::resource::CloneError::Work => SessionError::Work,
    }
}
