//! Versioned observations are applied only when constructing a new initial machine.

use super::{
    CatalogError, CodeCatalog, CodeId, CodeMode, CodeObservation, FactDependencies, FactError,
    FactKey, FactRevision, Observation, Observed, SnapshotId, StateFacts,
};
use crate::{
    analysis::{
        ConfigError, ExecutionConfig, WorldAnalysis,
        control::{Cancelled, Control},
        engine::{self, Counters, ExecutionReuse},
        transfer::StorageReadPolicy,
    },
    domain::{AbstractValue, NumericValue},
    resource::WorkBudget,
    world::{Account, Code, Entry, Existence, SnapshotIdentity, World, WorldError},
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
pub struct SessionAnalysis {
    snapshot: SnapshotId,
    revision: FactRevision,
    entry_code: Option<CodeId>,
    analysis: WorldAnalysis,
    dependencies: FactDependencies,
    reuse: ReuseStats,
    demand_driven: bool,
}

impl SessionAnalysis {
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
    pub fn analysis(&self) -> &WorldAnalysis {
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
pub enum SessionError {
    /// Cooperative cancellation of execution or session preparation.
    #[error(transparent)]
    Cancelled(#[from] Cancelled),
    /// Invalid execution configuration or root environment.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Invalid catalog contents.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// Rejected atomic initial-fact batch.
    #[error(transparent)]
    Facts(#[from] FactError),
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
    #[error(transparent)]
    World(#[from] WorldError),
    /// An adapter seed may carry header observations, never account state.
    #[error("initial seed must be empty and match the fact snapshot and catalog fork")]
    Seed,
}

/// One root execution over a shared code catalog and immutable initial facts.
///
/// There is no provider or RPC path here. Refinements create fresh runtime state
/// under cumulative budgets. Certified stack-only computation can be reused;
/// no historical Store or checkpoint is ever changed. General stateful selective
/// continuation is not claimed by the narrow pure-block reuse mechanism.
pub struct AnalysisSession {
    catalog: Arc<CodeCatalog>,
    facts: StateFacts,
    entry_code: Option<CodeId>,
    entry: Entry,
    config: ExecutionConfig,
    // Cloning this empty seed preserves offline symbolic source identity across
    // revisions; adapters can additionally retain pinned header observations.
    seed: World,
    budget: WorkBudget,
    counters: Counters,
    has_executed: bool,
    executed_revision: Option<FactRevision>,
    pending_changes: BTreeSet<FactKey>,
    latest: Option<Arc<SessionAnalysis>>,
    reuse: ExecutionReuse,
}

impl AnalysisSession {
    /// Compatibility adapter retaining every abstract initial World assumption.
    /// Exact consequences populate StateFacts; bounds and symbolic/provenance
    /// layers remain in the immutable base rather than being replaced by Top.
    pub fn from_world(
        world: World,
        entry: Entry,
        config: ExecutionConfig,
    ) -> Result<Self, ConfigError> {
        config.domain()?;
        entry.environment.validate()?;
        let snapshot = match world.identity() {
            SnapshotIdentity::Offline { label } => SnapshotId::offline(label),
            SnapshotIdentity::Chain {
                chain_id,
                block_hash,
            } => SnapshotId::chain(*chain_id, *block_hash),
        };
        let mut catalog = CodeCatalog::new(world.fork());
        let mut observations = Vec::new();
        for (address, account) in world.accounts() {
            let address = *address;
            let code = match &account.code {
                Code::Runtime(program) if program.byte_len() == 0 => Some(CodeObservation::Empty),
                Code::Runtime(program) => match catalog.with_runtime(program.clone()) {
                    Ok((next, id)) => {
                        catalog = next;
                        Some(CodeObservation::Runtime(id))
                    }
                    // Compilation is optional acceleration for the compatibility
                    // route. On a content collision retain the original base
                    // Program and ordinary execution; never equate its semantics.
                    Err(_) => None,
                },
                Code::Empty => Some(CodeObservation::Empty),
                Code::Delegation(target) => Some(CodeObservation::Delegation(*target)),
                Code::Unknown => None,
            };
            if let Some(code) = code {
                observations.push(Observation::Code { address, code });
            }
            if let Some(value) = account.balance.singleton() {
                observations.push(Observation::Balance { address, value });
            }
            if let Some(value) = account.nonce.singleton() {
                observations.push(Observation::Nonce { address, value });
            }
            if account.existence != Existence::Unknown {
                observations.push(Observation::Existence {
                    address,
                    exists: account.existence == Existence::Present,
                });
            }
            if !account.storage_unknown
                && account
                    .storage
                    .values()
                    .all(|value| value.singleton().is_some())
            {
                observations.push(Observation::StorageComplete { address });
            }
            for (slot, value) in &account.storage {
                if let Some(value) = value.singleton() {
                    observations.push(Observation::Storage {
                        address,
                        slot: *slot,
                        value,
                    });
                }
            }
        }
        let catalog = Arc::new(catalog);
        Ok(Self {
            reuse: ExecutionReuse::new(
                Arc::clone(&catalog),
                config.analysis.max_states,
                config.max_work,
                config.max_memory_bytes,
            ),
            catalog,
            facts: StateFacts::from_base(snapshot, observations),
            entry_code: None,
            entry,
            budget: WorkBudget::new(config.max_work),
            config,
            seed: world,
            counters: Counters::default(),
            has_executed: false,
            executed_revision: None,
            pending_changes: BTreeSet::new(),
            latest: None,
        })
    }

    /// Immutable abstract assumptions, independent of later exact observations.
    pub fn base(&self) -> &World {
        &self.seed
    }
    /// Start an offline session. Known code/address implies no other account facts.
    pub fn new(
        catalog: Arc<CodeCatalog>,
        facts: StateFacts,
        entry_code: CodeId,
        entry: Entry,
        config: ExecutionConfig,
    ) -> Result<Self, SessionError> {
        let seed = match facts.snapshot().identity() {
            SnapshotIdentity::Offline { label } => {
                World::offline(catalog.fork(), label, "layered initial observations")
            }
            SnapshotIdentity::Chain {
                chain_id,
                block_hash,
            } => World::anchored(
                catalog.fork(),
                *chain_id,
                *block_hash,
                "layered pinned initial observations",
            ),
        };
        Self::with_seed(catalog, facts, Some(entry_code), entry, config, seed)
    }

    /// Adapter-only seed carries identity/header data, never transaction effects.
    pub(crate) fn with_seed(
        catalog: Arc<CodeCatalog>,
        facts: StateFacts,
        entry_code: Option<CodeId>,
        entry: Entry,
        config: ExecutionConfig,
        seed: World,
    ) -> Result<Self, SessionError> {
        config.domain()?;
        entry.environment.validate().map_err(ConfigError::from)?;
        if seed.fork() != catalog.fork()
            || seed.identity() != facts.snapshot().identity()
            || !seed.accounts().is_empty()
        {
            return Err(SessionError::Seed);
        }
        validate_entry(&catalog, &facts, entry_code, entry.address)?;
        // Validate the complete batch before making the session observable.
        project_world(&catalog, &facts, &seed, entry_code, entry.address)?;
        Ok(Self {
            reuse: ExecutionReuse::new(
                Arc::clone(&catalog),
                config.analysis.max_states,
                config.max_work,
                config.max_memory_bytes,
            ),
            catalog,
            facts,
            entry_code,
            entry,
            budget: WorkBudget::new(config.max_work),
            config,
            seed,
            counters: Counters::default(),
            has_executed: false,
            executed_revision: None,
            pending_changes: BTreeSet::new(),
            latest: None,
        })
    }

    /// Shared compiled semantics, independent of this session's snapshot.
    pub fn catalog(&self) -> &Arc<CodeCatalog> {
        &self.catalog
    }

    /// Current immutable initial observations.
    pub fn facts(&self) -> &StateFacts {
        &self.facts
    }

    /// Compile additional code without asserting which account supplies it.
    pub fn supply_runtime(
        &mut self,
        program: crate::bytecode::Program,
    ) -> Result<CodeId, SessionError> {
        let (catalog, id) = self.catalog.with_runtime(program)?;
        self.catalog = Arc::new(catalog);
        self.reuse.set_catalog(Arc::clone(&self.catalog));
        Ok(id)
    }

    /// Session-scoped cancellation retains the engine's shared control contract.
    pub fn analyze_with_control(
        &mut self,
        control: &Control,
    ) -> Result<Arc<SessionAnalysis>, SessionError> {
        control.scope(|| {
            control.checkpoint()?;
            let report = self.analyze()?;
            control.checkpoint()?;
            Ok(report)
        })
    }

    /// Analyze the current revision or share its unchanged report without rerunning.
    /// A new revision always creates fresh Store and saved rollback checkpoints.
    pub fn analyze(&mut self) -> Result<Arc<SessionAnalysis>, SessionError> {
        self.analyze_policy(StorageReadPolicy::Abstract)
    }

    /// Request concrete missing initial code/finite slots without doing I/O.
    /// Supply the returned typed demands through `refine`, then call again.
    /// Unknown targets/keys still follow the ordinary conservative executor.
    pub fn analyze_with_demands(&mut self) -> Result<Arc<SessionAnalysis>, SessionError> {
        self.analyze_policy(StorageReadPolicy::Discover)
    }

    fn analyze_policy(
        &mut self,
        policy: StorageReadPolicy,
    ) -> Result<Arc<SessionAnalysis>, SessionError> {
        // A same-revision report is still a new API request: both the current
        // thread scope and the session's captured token must allow its return.
        if crate::analysis::MachineError::check_cancelled().is_err() || !self.budget.charge(0) {
            return Err(SessionError::Cancelled(Cancelled));
        }
        if let Some(latest) = &self.latest
            && latest.demand_driven == (policy == StorageReadPolicy::Discover)
        {
            return Ok(Arc::clone(latest));
        }
        let world = project_world(
            &self.catalog,
            &self.facts,
            &self.seed,
            self.entry_code,
            self.entry.address,
        )?;
        let analysis = engine::run_reusing(
            world,
            self.entry.clone(),
            self.config.clone(),
            &mut self.budget,
            &mut self.counters,
            policy,
            Some(&mut self.reuse),
        )?;
        if crate::analysis::MachineError::check_cancelled().is_err() || !self.budget.charge(0) {
            return Err(SessionError::Cancelled(Cancelled));
        }
        let report = Arc::new(SessionAnalysis {
            snapshot: self.facts.snapshot().clone(),
            revision: self.facts.revision(),
            entry_code: self.entry_code,
            analysis,
            dependencies: self.reuse.dependencies.clone(),
            reuse: self.reuse.stats(),
            demand_driven: policy == StorageReadPolicy::Discover,
        });
        self.has_executed = true;
        self.executed_revision = Some(self.facts.revision());
        self.pending_changes.clear();
        self.latest = Some(Arc::clone(&report));
        Ok(report)
    }

    /// Shared executor for acquisition adapters using their cumulative ledger.
    /// Observations still enter exclusively through `refine` and fresh projection.
    pub(in crate::analysis) fn run_metered(
        &mut self,
        budget: &mut WorkBudget,
        counters: &mut Counters,
        policy: StorageReadPolicy,
    ) -> Result<WorldAnalysis, SessionError> {
        let world = project_world(
            &self.catalog,
            &self.facts,
            &self.seed,
            self.entry_code,
            self.entry.address,
        )?;
        let analysis = engine::run_reusing(
            world,
            self.entry.clone(),
            self.config.clone(),
            budget,
            counters,
            policy,
            Some(&mut self.reuse),
        )?;
        self.has_executed = true;
        self.executed_revision = Some(self.facts.revision());
        self.pending_changes.clear();
        self.latest = None;
        Ok(analysis)
    }

    /// Ordinary World routes need no projection when no facts have been added:
    /// their original immutable input already contains every base assumption.
    pub(crate) fn execute_base(
        mut self,
        observer: &crate::analysis::progress::Observer,
    ) -> Result<WorldAnalysis, ConfigError> {
        self.counters = Counters::with_observer(observer);
        // This adapter consumes the session, so a memo could never be reused
        // after fact injection. Keep the shared catalog/dependency path without
        // charging one-shot calls for constructing disposable memo entries.
        self.reuse.disable_memo();
        engine::run_reusing(
            self.seed,
            self.entry,
            self.config,
            &mut self.budget,
            &mut self.counters,
            StorageReadPolicy::Abstract,
            Some(&mut self.reuse),
        )
    }

    /// Install a monotone initial-fact batch, with explicit graph invalidation.
    /// Current runtime state and previous reports cannot be patched by this API.
    pub fn refine(
        &mut self,
        snapshot: &SnapshotId,
        observations: impl IntoIterator<Item = Observation>,
    ) -> Result<RefinementEffect, SessionError> {
        let previous = self.facts.revision();
        let refinement = self.facts.refine(snapshot, observations)?;
        if refinement.added().is_empty() {
            return Ok(RefinementEffect::Unchanged { revision: previous });
        }
        validate_entry(
            &self.catalog,
            refinement.facts(),
            self.entry_code,
            self.entry.address,
        )?;
        project_world(
            &self.catalog,
            refinement.facts(),
            &self.seed,
            self.entry_code,
            self.entry.address,
        )?;
        let current = refinement.facts().revision();
        let added = refinement.added().to_vec();
        self.facts = refinement.into_facts();
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

fn runtime(catalog: &CodeCatalog, id: CodeId) -> Result<Code, SessionError> {
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

fn validate_entry(
    catalog: &CodeCatalog,
    facts: &StateFacts,
    entry_code: Option<CodeId>,
    address: crate::Address,
) -> Result<(), SessionError> {
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

fn project_world(
    catalog: &CodeCatalog,
    facts: &StateFacts,
    seed: &World,
    entry_code: Option<CodeId>,
    entry_address: crate::Address,
) -> Result<World, SessionError> {
    let mut addresses: BTreeSet<_> = facts.observations().map(|(key, _)| key.address()).collect();
    addresses.insert(entry_address);
    let mut accounts = std::collections::BTreeMap::new();
    for address in addresses {
        let mut account = seed
            .account(address)
            .cloned()
            .unwrap_or_else(Account::unknown);
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
        )?;
        account.nonce = intersect(
            &account.nonce,
            facts.nonce(address),
            FactKey::Nonce(address),
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
                        AbstractValue::constant(crate::U256::ZERO)
                    } else {
                        AbstractValue::top()
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
                )?,
            );
        }
    }
    Ok(seed.with_initial_accounts(accounts)?)
}

fn conflict(key: FactKey) -> SessionError {
    FactError::Conflict { key }.into()
}

fn intersect(
    base: &AbstractValue,
    observed: Observed<crate::U256>,
    key: FactKey,
) -> Result<AbstractValue, SessionError> {
    match observed {
        Observed::Unobserved => Ok(base.clone()),
        Observed::Known(value) if *base == AbstractValue::top() => {
            Ok(AbstractValue::constant(value))
        }
        Observed::Known(value) if base.contains(value) => {
            Ok(base.clone().with_numeric(NumericValue::constant(value)))
        }
        Observed::Known(_) => Err(conflict(key)),
    }
}
