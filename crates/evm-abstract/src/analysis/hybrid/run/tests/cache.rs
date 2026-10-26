//! Actual summary-reuse input, cache effects and the retained Hybrid payer.

use super::*;
use crate::{
    Address, Fork, U256,
    analysis::{
        Config, EvmMachinePayload, EvmProcedureSummaryRecord, EvmWorldAnalysis, HybridAnalysis,
        HybridInterpretation, HybridNativeBinding, MachineError, MachinePartition, Partitioned,
        PartitionedRelative, PolicyContext, PolicySite, ProcedureSummaryKind, RelativeState,
        SemanticPolicies, StatePolicy, Status, StructuralPartition, SummaryStats,
        TransferSemantics,
    },
    domain::{DomainSpec, EvmValue, NumericValue, relational::RelationLimits},
    ssa,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
    str::FromStr,
};

const WORK: usize = 2_000_000_000;

fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

fn fixture() -> (EvmWorld<NumericValue>, EvmEntry<NumericValue>) {
    let input: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/worlds/summary-reuse.json"
    )))
    .unwrap();
    let mut world = EvmWorld::offline(
        Fork::from_str(input["fork"].as_str().unwrap()).unwrap(),
        input["identity"]["label"].as_str().unwrap(),
        input["provenance"].as_str().unwrap(),
    );
    for item in input["accounts"].as_array().unwrap() {
        let mut account =
            EvmAccount::from_hex(item["code"].as_str().unwrap(), world.fork()).unwrap();
        account.balance = EvmValue::constant(item["balance"].as_str().unwrap().parse().unwrap());
        account.nonce = EvmValue::constant(item["nonce"].as_str().unwrap().parse().unwrap());
        account.storage_unknown = item["storage_unknown"].as_bool().unwrap();
        assert_eq!(item["existence"], "present");
        for (key, value) in item["storage"].as_object().unwrap() {
            account.storage.insert(
                key.parse().unwrap(),
                EvmValue::constant(value.as_str().unwrap().parse().unwrap()),
            );
        }
        world
            .insert(item["address"].as_str().unwrap().parse().unwrap(), account)
            .unwrap();
    }
    let entry = EvmEntry::concrete(
        address(0x101),
        address(0x1000),
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    (world, entry)
}

fn algebra() -> ValueAlgebra<ProductAlgebra> {
    ValueAlgebra::new(ProductAlgebra::from_spec(
        DomainSpec::new(
            NonZeroUsize::new(512).unwrap(),
            NonZeroUsize::new(16).unwrap(),
            NonZeroUsize::new(4096).unwrap(),
        )
        .with_relations(RelationLimits {
            enabled: true,
            max_constraints: 2048,
            max_nodes: 16384,
            max_depth: 256,
        }),
    ))
}

fn execute<S: StatePolicy<ProductAlgebra, Option<bool>>>(
    world: EvmWorld<NumericValue>,
    entry: EvmEntry<NumericValue>,
    state: S,
    summaries: bool,
    regions: usize,
    detail: usize,
) -> (HybridAnalysis<NumericValue>, WorkBudget) {
    let policies = EnginePolicies::default().with_state(state);
    let mut extension = NoExtension;
    let original = entry.clone();
    let mut budget = WorkBudget::new(WORK);
    assert!(budget.charge(13));
    let mut counters = engine::Counters::default();
    let report = AnalysisEngine::new(&algebra(), &policies, &mut extension)
        .run_hybrid_metered(
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig {
                max_work: WORK,
                max_call_depth: 1025,
                max_memory_bytes: 64 * 1024 * 1024,
                use_summaries: summaries,
                root_terminal_coverage: true,
                analysis: Config {
                    context_depth: 128,
                    max_states: 100_000,
                    max_transfers: 10_000_000,
                },
            },
            HybridConfig {
                max_detail_work: detail,
                max_detail_regions: regions,
            },
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        )
        .unwrap();
    assert_eq!(report.baseline().entry(), &original);
    assert!(report.coverage().baseline_current);
    assert_eq!(report.status(), Status::Converged);
    let work = report.coverage().work;
    assert_eq!(work.prior, 13);
    assert_eq!(
        work.total,
        13 + work.baseline + work.preparation + work.detail
    );
    assert_eq!(work.total, budget.used());
    assert!(work.preparation + work.detail <= work.detail_allowance);
    assert!(budget.used() <= WORK);
    assert!(counters.transfers >= report.baseline().transfers());
    if let Some(native) = report.detail() {
        assert_eq!(native.work(), budget.used());
        assert_eq!(native.entry(), &original);
        assert_eq!(native.snapshot(), report.baseline().snapshot());
        assert_eq!(native.revision(), report.baseline().revision());
    }
    (report, budget)
}

// One injective source map covers the entire returned-data/Store table, retaining
// all content and aliases. Only opaque producer/fresh names differ across runs.
fn relations(report: &crate::analysis::EvmWorldAnalysis<NumericValue>) -> Value {
    fn rename(
        value: &mut Value,
        inherited: Option<&str>,
        scopes: &mut BTreeMap<String, usize>,
        fresh: &mut BTreeMap<(String, u64), usize>,
    ) {
        match value {
            Value::Object(fields) => {
                let scope = fields
                    .get("identity_scope")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(scope) = &scope {
                    let next = scopes.len();
                    let id = *scopes.entry(scope.clone()).or_insert(next);
                    fields.insert(
                        "identity_scope".into(),
                        Value::String(format!("producer-{id}")),
                    );
                }
                let context = scope.as_deref().or(inherited);
                if let Some(id) = fields.get_mut("Fresh") {
                    let key = (context.unwrap().to_owned(), id.as_u64().unwrap());
                    let next = fresh.len();
                    *id = Value::from(*fresh.entry(key).or_insert(next));
                }
                for child in fields.values_mut() {
                    rename(child, context, scopes, fresh);
                }
            }
            Value::Array(values) => {
                for child in values {
                    rename(child, inherited, scopes, fresh);
                }
            }
            _ => {}
        }
    }
    let mut value = serde_json::to_value(
        report
            .outcomes()
            .iter()
            .map(|o| (o.kind, &o.data, &o.store))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    rename(&mut value, None, &mut BTreeMap::new(), &mut BTreeMap::new());
    value
}

fn verify(report: &HybridAnalysis<NumericValue>, budget: &mut WorkBudget) -> Value {
    assert!(report.coverage().detail_current);
    assert_eq!(
        report.coverage().detail_binding,
        HybridNativeBinding::OriginalOwner
    );
    assert_eq!(report.coverage().chosen, HybridInterpretation::Native);
    let native = report.chosen();
    assert!(native.frontiers().is_empty());
    let graph = ssa::build_world(native).unwrap();
    graph.verify(native).unwrap();
    assert_eq!(graph.blocks().len(), native.states().len());
    assert_eq!(graph.transitions().len(), native.edges().len());
    for (index, transition) in graph.transitions().iter().enumerate() {
        assert_eq!(transition.edge, index);
        assert_eq!(transition.kind, native.edges()[index].kind);
    }
    let before = budget.used();
    let normalized = ssa::build_normalized_world(native, budget).unwrap();
    normalized.verify(native, budget).unwrap();
    assert!(budget.used() > before && budget.used() <= WORK);
    for state in native.states() {
        let evidence = state.execution_evidence().unwrap();
        assert!(evidence.is_current());
        for (to, kind) in evidence.successors() {
            assert!(
                native
                    .edges()
                    .iter()
                    .any(|e| e.from == state.id && e.to == *to && e.kind == *kind)
            );
        }
    }
    serde_json::to_value(graph).unwrap()
}

// Unselected callees run in the relative table, outside graph-certificate hit
// statistics. Input growth can leave older closed records noncurrent, so qualify
// only the latest receipt for each real World caller before inspecting reuse.
fn relative_calls(
    native: &EvmWorldAnalysis<NumericValue>,
) -> Vec<&EvmProcedureSummaryRecord<NumericValue>> {
    let mut latest = BTreeMap::new();
    for record in native.procedure_summaries() {
        latest.insert(record.source_state, record);
    }
    let calls = latest.into_values().collect::<Vec<_>>();
    assert_eq!(
        calls.len(),
        2,
        "both original CALL sites must be represented"
    );
    for record in &calls {
        assert!(record.closed && record.source_current);
        assert!(record.version.generation > 0);
        assert_eq!(record.kind, ProcedureSummaryKind::KnownCall);
        let source = &native.states()[record.source_state];
        assert_eq!(source.entry.call_stack.depth(), 2);
        assert_eq!(source.key.frames[0].address, address(0x101));
        let root = record.root_procedure.unwrap();
        let procedure = &record.procedures[root];
        assert_eq!(procedure.version.namespace, record.version.namespace);
        assert_eq!(procedure.semantics, TransferSemantics::Native);
        assert_eq!(&procedure.frame, source.active());
        assert_eq!(procedure.frame.address, address(0x200));
        assert_eq!(procedure.frame.code_address, address(0x200));
        assert_eq!(
            procedure.frame.code_hash,
            source.program().unwrap().bytes_digest()
        );
        assert!(!procedure.frame.is_static);
        assert!(procedure.outputs.iter().any(|output| {
            output.kind == crate::analysis::OutcomeKind::Return
                && output.data.len().contains(U256::from(32))
        }));
        let bodies = record
            .regional_cfg
            .nodes
            .iter()
            .filter(|node| node.procedure == root)
            .collect::<Vec<_>>();
        assert!(!bodies.is_empty());
        for body in bodies {
            assert!(!body.excluded);
            assert_eq!(body.frame.address, procedure.frame.address);
            assert_eq!(body.frame.code_address, procedure.frame.code_address);
            assert_eq!(body.frame.code_hash, procedure.frame.code_hash);
            assert_eq!(body.frame.caller, procedure.frame.caller);
            assert_eq!(body.frame.is_static, procedure.frame.is_static);
            let transfer = body.transfer.as_ref().unwrap();
            assert_eq!(transfer.semantics, TransferSemantics::Native);
            assert!(transfer.process.is_none());
            assert!(transfer.current && transfer.complete && transfer.visited_block);
            assert_eq!(transfer.input_revision, body.input_revision);
        }
    }
    assert_ne!(
        native.states()[calls[0].source_state].key.frames[0].basic_block_index,
        native.states()[calls[1].source_state].key.frames[0].basic_block_index,
        "reuse must cross the two distinct caller continuations"
    );
    calls
}

fn verify_relative_reuse<'a>(
    on: &'a EvmWorldAnalysis<NumericValue>,
    off: &EvmWorldAnalysis<NumericValue>,
) -> Vec<&'a EvmProcedureSummaryRecord<NumericValue>> {
    let calls = relative_calls(on);
    let isolated = relative_calls(off);
    let identity = |record: &&EvmProcedureSummaryRecord<NumericValue>| {
        (record.version.namespace, record.root_procedure.unwrap())
    };
    assert_eq!(calls.iter().map(identity).collect::<BTreeSet<_>>().len(), 1);
    assert_eq!(
        isolated.iter().map(identity).collect::<BTreeSet<_>>().len(),
        2
    );
    let root = calls[0].root_procedure.unwrap();
    assert_eq!(
        calls[0].procedures[root].version,
        calls[1].procedures[root].version
    );
    assert!(calls[0].procedures[root].shares_storage_with(&calls[1].procedures[root]));
    assert!(
        calls
            .iter()
            .any(|record| record.reused > 0 && record.transfers == 0)
    );
    assert!(on.transfers() < off.transfers());
    // Disabling cross-caller reuse still permits each source to close recursion
    // and reprove its own input. Distinct namespaces, rather than reused == 0,
    // are the negative oracle for sharing between these actual CALL sites.
    assert_eq!(on.summary_stats(), &SummaryStats::default());
    assert_eq!(off.summary_stats(), &SummaryStats::default());
    assert!(on.summaries().is_empty() && off.summaries().is_empty());
    eprintln!(
        "HYBRID_RELATIVE_CACHE_JSON {}",
        serde_json::json!({
            "on": calls.iter().map(|record| (record.source_state, record.version.namespace, record.root_procedure, record.reused, record.transfers)).collect::<Vec<_>>(),
            "off": isolated.iter().map(|record| (record.source_state, record.version.namespace, record.root_procedure, record.reused, record.transfers)).collect::<Vec<_>>(),
            "on_transfers": on.transfers(), "off_transfers": off.transfers(),
        })
    );
    calls
}

#[test]
fn original_summary_reuse_preserves_full_graph_ssa_and_joint_outcomes_on_one_ledger() {
    let (world, entry) = fixture();
    let (on, mut on_budget) = execute(
        world.clone(),
        entry.clone(),
        SemanticPolicies::default(),
        true,
        1,
        usize::MAX,
    );
    let native = on.detail().unwrap();
    assert!(
        native
            .edges()
            .iter()
            .filter(|e| e.kind == crate::analysis::MachineEdgeKind::Call)
            .count()
            >= 2
    );
    let algebra = algebra();
    let zero = EvmValue::constant(U256::ZERO);
    assert!(native.outcomes().iter().any(|o| {
        o.kind == crate::analysis::OutcomeKind::Return
            && o.data.len().contains(U256::from(32))
            && o.data
                .read_word_checked(&zero, algebra.borrowed())
                .unwrap()
                .contains(U256::from(7))
            && o.store
                .read(address(0x200), &zero, algebra.borrowed())
                .contains(U256::from(7))
            && o.store
                .read(address(0x101), &zero, algebra.borrowed())
                .singleton()
                == Some(U256::ZERO)
            && o.store.nonce(address(0x200)).singleton() == Some(U256::ZERO)
            && o.store.read_balance(address(0x101)).singleton() == Some(U256::from(1_000_000))
    }));
    let fingerprint = world.fingerprint().unwrap();
    assert_eq!(native.world().fingerprint().unwrap(), fingerprint);
    assert_eq!(native.entry().environment, entry.environment);
    let (off, mut off_budget) = execute(
        world,
        entry,
        SemanticPolicies::default(),
        false,
        1,
        usize::MAX,
    );
    assert_eq!(off.chosen().summary_stats(), &SummaryStats::default());
    assert!(off.chosen().summaries().is_empty());
    verify_relative_reuse(native, off.chosen());
    assert_eq!(
        native.states().iter().map(|s| &s.key).collect::<Vec<_>>(),
        off.chosen()
            .states()
            .iter()
            .map(|s| &s.key)
            .collect::<Vec<_>>()
    );
    assert_eq!(native.edges(), off.chosen().edges());
    assert_eq!(verify(&on, &mut on_budget), verify(&off, &mut off_budget));
    assert_eq!(relations(native), relations(off.chosen()));
    eprintln!(
        "HYBRID_CACHE_JSON {}",
        serde_json::json!({"case":"original-summary-reuse", "on":native.summary_stats(), "off":off.chosen().summary_stats(), "on_work":on.coverage().work,"off_work":off.coverage().work,"on_with_ssa":on_budget.used(),"off_with_ssa":off_budget.used()})
    );
}

#[test]
fn cached_child_history_remains_selected_or_erased_after_prefix_rebinding() {
    let (mut world, entry) = fixture();
    let mut child = EvmAccount::from_hex("600456005b5f545f5260205ff3", world.fork()).unwrap();
    child.balance = EvmValue::constant(U256::from(1_000_000));
    child
        .storage
        .insert(U256::ZERO, EvmValue::constant(U256::from(7)));
    // This separate precision control adds a real child jump; the original
    // fixture above is always loaded unchanged from its tracked JSON.
    let mut modified = EvmWorld::new(world.fork(), "hybrid-cache:child-jump");
    modified
        .insert(address(0x101), world.accounts()[&address(0x101)].clone())
        .unwrap();
    modified.insert(address(0x200), child).unwrap();
    world = modified;
    for regions in [1, 2] {
        let (on, mut on_budget) = execute(
            world.clone(),
            entry.clone(),
            SemanticPolicies::default(),
            true,
            regions,
            usize::MAX,
        );
        let (off, mut off_budget) = execute(
            world.clone(),
            entry.clone(),
            SemanticPolicies::default(),
            false,
            regions,
            usize::MAX,
        );
        assert_eq!(verify(&on, &mut on_budget), verify(&off, &mut off_budget));
        assert_eq!(relations(on.chosen()), relations(off.chosen()));
        let mut jumped = false;
        let mut sites = 0;
        for site in on
            .local_detail()
            .filter(|p| p.frame.code_address == address(0x200))
        {
            sites += 1;
            assert_eq!(site.context_selected, regions == 2);
            if !site.frame.jump_history.is_empty() {
                jumped = true;
            }
        }
        assert_eq!(jumped, regions == 2);
        if regions == 1 {
            assert_eq!(sites, 0, "unselected bodies use regional IDs");
            for record in verify_relative_reuse(on.chosen(), off.chosen()) {
                let root = record.root_procedure.unwrap();
                let bodies = record
                    .regional_cfg
                    .nodes
                    .iter()
                    .filter(|node| node.procedure == root)
                    .collect::<Vec<_>>();
                assert_eq!(bodies.len(), 2, "entry jump and its real destination");
                assert!(bodies.iter().all(|body| body.frame.jump_history.is_empty()));
                let program = on.chosen().states()[record.source_state].program().unwrap();
                let pc = |body: &&crate::analysis::EvmRegionalNode| {
                    program.blocks()[body.frame.basic_block_index].instructions[0].pc
                };
                assert_eq!(
                    bodies.iter().map(pc).collect::<BTreeSet<_>>(),
                    BTreeSet::from([0, 4])
                );
                let entry = bodies.iter().copied().find(|body| pc(body) == 0).unwrap();
                let target = bodies.iter().copied().find(|body| pc(body) == 4).unwrap();
                assert!(
                    entry
                        .transfer
                        .as_ref()
                        .unwrap()
                        .successors
                        .iter()
                        .any(|edge| edge.to == target.id
                            && edge.kind
                                == crate::analysis::MachineEdgeKind::Intraprocedural(
                                    crate::analysis::EdgeKind::Jump
                                ))
                );
            }
        } else {
            assert!(sites > 0);
            assert!(on.chosen().summary_stats().hits > 0);
            assert!(on.chosen().summary_stats().imported_states > 0);
            assert_eq!(off.chosen().summary_stats(), &SummaryStats::default());
            assert!(off.chosen().summaries().is_empty());
            assert!(on.chosen().summaries().iter().any(|r| {
                r.reused_at
                    .iter()
                    .any(|id| on.chosen().states()[*id].active().code_address == address(0x200))
            }));
        }
    }
}

// A caller may decline the portability promise even for structural inputs.
// The outer wrapper must honor that actual capability instead of inferring it.
struct LocalOnly;
impl MachinePartition<ProductAlgebra, Option<bool>> for LocalOnly {
    type Key = <StructuralPartition as MachinePartition<ProductAlgebra, Option<bool>>>::Key;
    fn prepare(
        &self,
        input: &EvmMachinePayload<NumericValue>,
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
        StructuralPartition.prepare_relative(input, site, class, cx)
    }
}

#[test]
fn nonportable_inner_keeps_real_cache_disabled_without_losing_native_execution() {
    let (world, entry) = fixture();
    for regions in [1, 2] {
        let SemanticPolicies {
            join,
            acceleration,
            summary,
            ..
        } = SemanticPolicies::default();
        let (report, mut budget) = execute(
            world.clone(),
            entry.clone(),
            SemanticPolicies::new(LocalOnly, join, acceleration, summary),
            true,
            regions,
            usize::MAX,
        );
        let graph = verify(&report, &mut budget);
        assert_eq!(report.chosen().summary_stats(), &SummaryStats::default());
        assert!(report.chosen().summaries().is_empty());
        if regions == 2 {
            // Select the child too, so graph replay is actually eligible and
            // this negative tests LocalOnly's capability, not relative routing.
            let (portable, mut portable_budget) = execute(
                world.clone(),
                entry.clone(),
                SemanticPolicies::default(),
                true,
                regions,
                usize::MAX,
            );
            let native = portable.chosen();
            assert!(native.summary_stats().hits > 0);
            assert!(native.summary_stats().imported_states > 0);
            assert!(native.summaries().iter().any(|record| {
                !record.reused_at.is_empty()
                    && record.input.world_fingerprint == world.fingerprint().unwrap()
                    && record.input.environment == entry.environment
                    && record.outputs.iter().any(|output| {
                        output.kind == crate::analysis::OutcomeKind::Return
                            && output.data.len().contains(U256::from(32))
                    })
            }));
            assert_eq!(graph, verify(&portable, &mut portable_budget));
            assert_eq!(relations(report.chosen()), relations(native));
        }
    }
}

#[test]
fn optional_cache_detail_cannot_spend_after_its_visible_allowance() {
    let (world, entry) = fixture();
    let (report, budget) = execute(world, entry, SemanticPolicies::default(), true, 1, 1);
    assert!(report.detail().is_none());
    assert_eq!(report.coverage().work.detail_allowance, 1);
    assert_eq!(
        report.coverage().work.preparation + report.coverage().work.detail,
        0
    );
    assert_eq!(report.coverage().chosen, HybridInterpretation::Process);
    assert_eq!(report.work(), budget.used());
}
