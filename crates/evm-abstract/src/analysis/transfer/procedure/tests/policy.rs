//! Production relative slots use injected state, keys and refusal semantics.

#[cfg(test)]
use crate::domain::NumericValue;
use crate::domain::ProductAlgebra;

use super::*;
use crate::analysis::cooperation::NoExtension;
use crate::analysis::engine::{
    Acceleration, CorrelatedJoin, EnginePolicies, FeedbackAcceleration, FirstResumeStatus,
    MachinePartition, Partitioned, PartitionedRelative, PolicyContext, PolicySite, ProcedureOutput,
    ProcedureSummaryPolicy, ProjectedSummaries, RecentDistinctJumps, RelativeState,
    SemanticPolicies, StatePolicy, StructuralPartition, cooperation::Cooperation,
};
use crate::analysis::testing::Entry;
use crate::analysis::worklist::runtime::Runtime;
use crate::domain::ValueAlgebra;
use std::convert::Infallible;

mod process_growth;

struct Harness<S: StatePolicy<ProductAlgebra, Option<bool>>> {
    world: World,
    entry: Entry,
    config: crate::analysis::ExecutionConfig,
    table: EvmTable<NumericValue, S::Key>,
    runtime: Runtime,
    caller: analysis_kernel::RunningJob,
    domain: ValueAlgebra<ProductAlgebra>,
    policies: EnginePolicies<RecentDistinctJumps, FirstResumeStatus, S>,
    cooperation: Cooperation<NumericValue, NoExtension, Infallible>,
    budget: WorkBudget,
    counters: Counters,
}

impl<S: StatePolicy<ProductAlgebra, Option<bool>>> Harness<S> {
    fn new(code: &str, state: S) -> Self {
        Self::with_config(code, state, crate::analysis::machine::fixtures::config())
    }

    fn with_config(code: &str, state: S, config: crate::analysis::ExecutionConfig) -> Self {
        let (world, entry) = fixture(&[(0x101, code)]);
        let config_numeric = crate::analysis::testing::NumericLimits::machine();
        let mut budget = WorkBudget::new(config.max_work);
        let mut runtime = Runtime::new(&mut budget).unwrap();
        runtime.register_world(0, 1, &mut budget).unwrap();
        let (_, caller) = runtime.next(false, &mut budget).unwrap().unwrap();
        Self {
            world,
            entry,
            domain: ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec())),
            budget,
            config,
            table: EvmTable::default(),
            runtime,
            caller,
            policies: EnginePolicies::default().with_state(state),
            cooperation: Cooperation::new(Default::default()),
            counters: Counters::default(),
        }
    }

    fn input(&self, value: u64) -> MachinePayload {
        let mut input =
            super::super::testing::initial(&self.world, &self.entry, self.domain.borrowed())
                .unwrap();
        input.active_mut().stack = vec![word(value)];
        input.normalize();
        input
    }

    fn set_relations(&mut self, relations: crate::domain::relational::RelationLimits) {
        self.domain =
            crate::analysis::testing::product(self.domain.spec().with_relations(relations));
    }

    fn solver(
        &mut self,
    ) -> EvmSolver<
        '_,
        ProductAlgebra,
        RecentDistinctJumps,
        FirstResumeStatus,
        S,
        NoExtension,
        Infallible,
    > {
        EvmSolver {
            table: &mut self.table,
            world: &self.world,
            entry: &self.entry,
            config: &self.config,
            domain: self.domain.borrowed(),
            policies: &self.policies,
            budget: &mut self.budget,
            counters: &mut self.counters,
            storage_reads: StorageReadPolicy::Abstract,
            runtime: &mut self.runtime,
            cooperation: &mut self.cooperation,
            active: None,
            caller: Some(&mut self.caller),
            namespace: 0,
            producer: None,
            deferred: false,
            consumed: None,
        }
    }
}

/// Distinct Body/Subscriber classes need not be distinct Procedure classes.
/// The boolean vocabulary is finite; executable shape stays structural.
struct SitePartition;

impl MachinePartition<ProductAlgebra, Option<bool>> for SitePartition {
    type Key = (crate::analysis::MachineKey, Option<bool>, bool);

    fn prepare(
        &self,
        input: &MachinePayload,
        site: PolicySite,
        class: Option<bool>,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<Partitioned<Self::Key, NumericValue>, MachineError> {
        let Partitioned {
            key: (key, class),
            payload,
        } = StructuralPartition.prepare(input, site, class, cx)?;
        Ok(Partitioned {
            key: (key, class, false),
            payload,
        })
    }

    fn prepare_relative(
        &self,
        input: &RelativeState<NumericValue>,
        site: PolicySite,
        class: Option<bool>,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<PartitionedRelative<Self::Key, NumericValue>, MachineError> {
        let split = matches!(site, PolicySite::Body | PolicySite::Subscriber)
            && input.frames[0].stack.first().and_then(EvmValue::singleton) == Some(U256::from(7));
        let PartitionedRelative {
            key: (key, class),
            payload,
        } = StructuralPartition.prepare_relative(input, site, class, cx)?;
        Ok(PartitionedRelative {
            key: (key, class, split),
            payload,
        })
    }
}

#[test]
fn procedure_entry_retains_growth_when_body_preparation_selects_another_key() {
    let state = SemanticPolicies::new(
        SitePartition,
        CorrelatedJoin,
        FeedbackAcceleration,
        ProjectedSummaries,
    );
    let mut h = Harness::new("00", state);
    h.set_relations(crate::domain::relational::RelationLimits {
        enabled: false,
        ..h.domain.spec().relations()
    });
    let [first, second, third] = [7, 9, 11].map(|value| h.input(value));
    let mut solver = h.solver();
    let procedure = solver.procedure(first).unwrap();
    let original_entry = solver.table.procedures[procedure].first_body();
    assert_eq!(solver.procedure(second).unwrap(), procedure);
    assert_eq!(solver.procedure(third).unwrap(), procedure);
    let retained = &solver.table.procedures[procedure];
    assert_eq!(retained.entries.len(), 2);
    assert_eq!(retained.entry_updates, 2);
    for value in [7, 9, 11] {
        assert!(retained.entry_payload.active().stack[0].contains(U256::from(value)));
    }
    assert_eq!(
        solver.table.nodes[original_entry].payload.active().stack[0].singleton(),
        Some(U256::from(7))
    );
    assert!(
        retained.entries.iter().any(|id| {
            solver.table.nodes[*id].payload.active().stack[0].contains(U256::from(11))
        })
    );
}

#[test]
fn procedure_entry_anchor_wakes_its_reader_when_the_body_key_changes() {
    let state = SemanticPolicies::new(
        SitePartition,
        CorrelatedJoin,
        FeedbackAcceleration,
        ProjectedSummaries,
    );
    let mut h = Harness::new("00", state);
    h.set_relations(crate::domain::relational::RelationLimits {
        enabled: false,
        ..h.domain.spec().relations()
    });
    let first = h.input(7);
    let next = h.input(9);
    {
        let mut solver = h.solver();
        let procedure = solver.procedure(first).unwrap();
        let source = solver.table.entry_source(procedure).unwrap();
        let slot = source.slot.clone();
        let revision = *source.revision;
        solver
            .runtime
            .with_run(
                solver.caller.as_deref_mut().unwrap(),
                solver.budget,
                |run| {
                    run.read_slot(source.slot, source.revision, 1)
                        .map_err(crate::analysis::worklist::kernel_failure)?;
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(solver.procedure(next).unwrap(), procedure);
        let source = solver.table.entry_source(procedure).unwrap();
        assert!(*source.revision > revision);
        assert_eq!(solver.table.procedures[procedure].entries.len(), 2);
        for value in [7, 9] {
            assert!(source.payload.active().stack[0].contains(U256::from(value)));
        }
        let version = solver
            .runtime
            .with_run(
                solver.caller.as_deref_mut().unwrap(),
                solver.budget,
                |run| {
                    run.slot_version(&slot)
                        .map_err(crate::analysis::worklist::kernel_failure)
                },
            )
            .unwrap();
        assert!(
            version.get() > 0,
            "the original entry source, not the new Body slot, changed"
        );
    }
    let Harness {
        mut runtime,
        caller,
        mut budget,
        ..
    } = h;
    runtime.finish(caller, &mut budget).unwrap();
    assert!(
        runtime.contains_world(&0),
        "the actual entry reader must reawaken"
    );
}

struct RefuseBody;

impl MachinePartition<ProductAlgebra, Option<bool>> for RefuseBody {
    type Key = (crate::analysis::MachineKey, Option<bool>);

    fn prepare(
        &self,
        input: &MachinePayload,
        site: PolicySite,
        class: Option<bool>,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<Partitioned<Self::Key, NumericValue>, MachineError> {
        StructuralPartition.prepare(input, site, class, cx)
    }

    fn prepare_relative(
        &self,
        input: &RelativeState<NumericValue>,
        site: PolicySite,
        class: Option<bool>,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<PartitionedRelative<Self::Key, NumericValue>, MachineError> {
        if site == PolicySite::Body {
            Err(MachineError::States)
        } else {
            StructuralPartition.prepare_relative(input, site, class, cx)
        }
    }
}

#[test]
fn body_refusal_preserves_the_published_procedure_entry_identity() {
    let mut h = Harness::new(
        "00",
        SemanticPolicies::new(
            RefuseBody,
            CorrelatedJoin,
            FeedbackAcceleration,
            ProjectedSummaries,
        ),
    );
    for value in [7, 9] {
        let input = h.input(value);
        assert_eq!(h.solver().procedure(input), Err(MachineError::States));
        assert_eq!(h.table.procedures.len(), 1);
        assert_eq!(h.table.procedures_by_key.len(), 1);
        assert!(h.table.nodes.is_empty());
        let source = h
            .table
            .entry_source(0)
            .expect("entry identity survives Body refusal");
        assert_eq!(source.namespace, 0);
        assert!(source.payload.active().stack[0].contains(U256::from(7)));
        assert_eq!(source.payload.active_frame().code_address, h.entry.address);
    }
}

#[test]
fn subscriber_policy_key_separates_templates_with_the_same_caller_and_callee() {
    let state = SemanticPolicies::new(
        SitePartition,
        CorrelatedJoin,
        FeedbackAcceleration,
        ProjectedSummaries,
    );
    let mut h = Harness::new("005b00", state);
    let mut payload = h.input(7);
    let mut solver = h.solver();
    let caller = solver.procedure(payload.clone()).unwrap();
    let from = solver.table.procedures[caller].first_body();
    let mut child = payload.active().clone();
    child.key.basic_block_index = 1;
    child.stack.clear();
    payload.call_stack.push_child(ChildFrame {
        state: child,
        continuation: Continuation {
            return_block: Some(1),
            output_offset: word(0),
            output_size: word(0),
            creation: None,
        },
    });
    payload.normalize();
    solver.subscribe(from, payload.clone()).unwrap();
    payload.call_stack.iter_mut().next().unwrap().stack[0] = word(9);
    solver.subscribe(from, payload).unwrap();
    assert_eq!(solver.table.subscribers.len(), 2);
    assert_eq!(
        solver.table.subscribers[0].from,
        solver.table.subscribers[1].from
    );
    assert_eq!(
        solver.table.subscribers[0].callee,
        solver.table.subscribers[1].callee
    );
    assert_eq!(solver.table.subscribers_by_key.len(), 2);
    assert_eq!(
        solver.table.subscribers[0].payload.frames[0].stack[0].singleton(),
        Some(U256::from(7))
    );
    assert_eq!(
        solver.table.subscribers[1].payload.frames[0].stack[0].singleton(),
        Some(U256::from(9))
    );
    solver.run();
    assert!(
        solver.table.frontiers.is_empty(),
        "{:?}",
        solver.table.frontiers
    );
    assert!(
        solver
            .table
            .subscribers
            .iter()
            .all(|subscriber| !subscriber.delivered.is_empty())
    );
}

/// A caller's work refusal must also govern the runtime retention retry.
struct RejectRuntimeCoarsening;

impl ProcedureSummaryPolicy<ProductAlgebra> for RejectRuntimeCoarsening {
    fn admit_relative(
        &self,
        current: &RelativeState<NumericValue>,
        incoming: &RelativeState<NumericValue>,
        site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<RelativeState<NumericValue>, MachineError> {
        if site == PolicySite::RuntimeRetry {
            cx.reserve(usize::MAX)?;
        }
        ProjectedSummaries.admit_relative(current, incoming, site, acceleration, cx)
    }

    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<NumericValue>>,
        incoming: &ProcedureOutput<NumericValue>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, ProductAlgebra>,
    ) -> Result<ProcedureOutput<NumericValue>, MachineError> {
        ProjectedSummaries.admit_output(current, incoming, acceleration, cx)
    }
}

#[test]
fn runtime_retry_refuses_injected_policy_instead_of_executing_legacy_bound() {
    let state = SemanticPolicies::new(
        StructuralPartition,
        CorrelatedJoin,
        FeedbackAcceleration,
        RejectRuntimeCoarsening,
    );
    let mut h = Harness::new("525b00", state);
    let domain = h.domain.borrowed();
    let mut input = h.input(7);
    input
        .active_mut()
        .memory
        .write_byte(&word(0), &domain.join(&word(1), &word(2)), domain)
        .unwrap();
    input.active_mut().stack.push(word(0));
    input.normalize();
    let summary = super::super::testing::form(&input, &h.entry, domain).unwrap();
    let live = super::super::testing::instantiate(&summary, domain).unwrap();
    h.set_relations(crate::domain::relational::RelationLimits {
        max_nodes: live.active().memory.state_root().nodes() + 2,
        ..h.domain.spec().relations()
    });
    let mut solver = h.solver();
    solver.procedure(input).unwrap();
    let original = solver.table.nodes[0].payload.clone();
    let live = instantiate(&original, solver.domain).unwrap();
    let first = execute_inner_with(
        solver.world,
        solver.entry,
        live,
        solver.config,
        solver.domain,
        &solver.policies.context,
        solver.budget,
        solver.storage_reads,
        true,
    );
    assert!(!first.retention_failures.is_empty());
    let retry = solver.retry_runtime(0, &original, first);
    assert!(retry.successors.is_empty());
    assert!(
        retry
            .frontiers
            .iter()
            .any(|frontier| frontier.reason == MachineError::Work.frontier_reason())
    );
    assert!(semantic_eq(&original, &solver.table.nodes[0].payload));
    assert!(matches!(
        solver.table.nodes[0].runtime_retry,
        super::super::coarse_retry::EvmRuntimeRetry::Attempted
    ));
}
