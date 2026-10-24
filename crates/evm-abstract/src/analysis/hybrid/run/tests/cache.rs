//! Actual summary-reuse input, cache effects and the retained Hybrid payer.

use super::*;
use crate::{
    Address, Fork, U256,
    analysis::{
        Config, EvmMachinePayload, HybridAnalysis, HybridInterpretation, HybridNativeBinding,
        MachineError, MachinePartition, Partitioned, PartitionedRelative, PolicyContext,
        PolicySite, RelativeState, SemanticPolicies, StatePolicy, Status, StructuralPartition,
        SummaryStats,
    },
    domain::{DomainSpec, EvmValue, NumericValue, relational::RelationLimits},
    ssa,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use serde_json::Value;
use std::{collections::BTreeMap, num::NonZeroUsize, str::FromStr};

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
        native.summary_stats().hits > 0,
        "{:?}",
        native.summary_stats()
    );
    assert!(native.summary_stats().imported_states > 0);
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
    assert!(native.summaries().iter().any(|r| {
        !r.reused_at.is_empty()
            && r.input.world_fingerprint == fingerprint
            && r.input.environment == entry.environment
            && r.outputs.iter().any(|o| {
                o.kind == crate::analysis::OutcomeKind::Return
                    && o.data.len().contains(U256::from(32))
            })
    }));
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
        assert!(on.chosen().summary_stats().hits > 0);
        assert_eq!(verify(&on, &mut on_budget), verify(&off, &mut off_budget));
        assert_eq!(relations(on.chosen()), relations(off.chosen()));
        let mut jumped = false;
        for site in on
            .local_detail()
            .filter(|p| p.frame.code_address == address(0x200))
        {
            assert_eq!(site.context_selected, regions == 2);
            if !site.frame.jump_history.is_empty() {
                jumped = true;
            }
        }
        assert_eq!(jumped, regions == 2);
        assert!(on.chosen().summaries().iter().any(|r| {
            r.reused_at
                .iter()
                .any(|id| on.chosen().states()[*id].active().code_address == address(0x200))
        }));
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
    let SemanticPolicies {
        join,
        acceleration,
        summary,
        ..
    } = SemanticPolicies::default();
    let (report, mut budget) = execute(
        world,
        entry,
        SemanticPolicies::new(LocalOnly, join, acceleration, summary),
        true,
        1,
        usize::MAX,
    );
    verify(&report, &mut budget);
    assert_eq!(report.chosen().summary_stats(), &SummaryStats::default());
    assert!(report.chosen().summaries().is_empty());
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
