//! Explicit opt-in diagnostic on immutable offline captures and full Unknowns.
//! This does not fetch accounts, enable an RPC provider or change model defaults.

use crate::native;
use alloy_primitives::{Address, B256, U256};
use evm_abstract::{
    Fork,
    analysis::{
        AnalysisEngine, AnalysisInput, BoundedJoin, Config, EnginePolicies, ExecutionConfig,
        FeedbackAcceleration, FirstResumeStatus, KnownProcessSummaries, RecentDistinctJumps,
        SemanticPolicies, StructuralPartition, control::Control, cooperation::NoExtension,
    },
    domain::{EvmValue, NumericValue, relational::RelationLimits},
    world::{AddressInput, EvmAccount, EvmEntry, EvmWorld},
};
use serde_json::Value;
use std::{collections::BTreeSet, str::FromStr};

const USDC_WORLD: &str =
    include_str!("../../../../examples/known-recursion/captured/usdc-26180958-observed-world.json");
const WETH_WORLD: &str =
    include_str!("../../../../examples/known-recursion/captured/weth-26180958-observed-world.json");
const USDC_REQUEST: &str =
    include_str!("../../../../examples/known-recursion/captured/usdc-original-request.json");
const WETH_REQUEST: &str =
    include_str!("../../../../examples/known-recursion/captured/weth-original-request.json");
const MANIFEST: &str = include_str!("../../../../examples/known-recursion/captured/manifest.json");
fn scalar(value: &Value) -> U256 {
    U256::from_str(value.as_str().unwrap()).unwrap()
}

fn run_capture(case: &str, world: &str, request: &str) {
    let data: Value = serde_json::from_str(world).unwrap();
    let request: Value = serde_json::from_str(request).unwrap();
    assert_eq!(data["fork"], "osaka");
    assert_eq!(request["fork"], "Osaka");
    assert_eq!(request["environment"]["is_static"], false);
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let mut world = EvmWorld::<NumericValue>::anchored(
        Fork::Osaka,
        scalar(&data["identity"]["chain_id"]),
        B256::from_str(data["identity"]["block_hash"].as_str().unwrap()).unwrap(),
        data["provenance"].as_str().unwrap(),
    );
    for item in data["accounts"].as_array().unwrap() {
        let owner = Address::from_str(item["address"].as_str().unwrap()).unwrap();
        let mut account =
            EvmAccount::from_hex(item["code"].as_str().unwrap(), world.fork()).unwrap();
        account.balance = EvmValue::constant(scalar(&item["balance"]));
        account.nonce = EvmValue::constant(scalar(&item["nonce"]));
        account.storage_unknown = item["storage_unknown"].as_bool().unwrap();
        assert!(
            account.storage_unknown,
            "all unobserved initial slots stay unknown"
        );
        for (slot, value) in item["storage"].as_object().unwrap() {
            account.storage.insert(
                U256::from_str(slot).unwrap(),
                EvmValue::constant(scalar(value)),
            );
        }
        world
            .insert_with_code_hash(
                owner,
                account,
                B256::from_str(item["code_hash"].as_str().unwrap()).unwrap(),
            )
            .unwrap();
    }
    let owner = Address::from_str(request["input"]["Rpc"]["address"].as_str().unwrap()).unwrap();
    assert_eq!(request["environment"]["caller"], "Unknown");
    assert_eq!(request["environment"]["call_value"], "Unknown");
    assert_eq!(request["environment"]["calldata"], "Unknown");
    let mut input = EvmEntry::new(owner);
    // These are the original RPC snapshot's immutable captured facts; they do
    // not instantiate or narrow the original transaction's Unknown values.
    let snapshot = &manifest["cases"][case]["captured_snapshot"];
    input.environment.coinbase =
        AddressInput::Concrete(Address::from_str(snapshot["coinbase"].as_str().unwrap()).unwrap());
    input.environment.timestamp = EvmValue::constant(scalar(&snapshot["timestamp"]));
    input.environment.number = EvmValue::constant(scalar(&snapshot["number"]));
    input.environment.prevrandao = EvmValue::constant(scalar(&snapshot["prevrandao"]));
    input.environment.gas_limit = EvmValue::constant(scalar(&snapshot["gas_limit"]));
    input.environment.chain_id = Some(EvmValue::constant(scalar(&snapshot["chain_id"])));
    input.environment.base_fee = EvmValue::constant(scalar(&snapshot["base_fee"]));
    input.environment.blob_base_fee = EvmValue::constant(scalar(&snapshot["blob_base_fee"]));
    input.environment.block_hashes.insert(
        scalar(&snapshot["number"]) - U256::ONE,
        B256::from_str(snapshot["parent_hash"].as_str().unwrap()).unwrap(),
    );
    let limits = &request["limits"];
    let limit = |key: &str| usize::try_from(limits[key].as_u64().unwrap()).unwrap();
    let values = native::product_with(
        native::spec(
            limit("max_constants"),
            limit("reduction_rounds"),
            limit("max_facts"),
        )
        .with_relations(RelationLimits {
            enabled: limits["relations_enabled"].as_bool().unwrap(),
            max_constraints: limit("max_constraints"),
            max_nodes: limit("max_expression_nodes"),
            max_depth: limit("max_expression_depth"),
        }),
    );
    let policies = EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus).with_state(
        SemanticPolicies::new(
            StructuralPartition,
            BoundedJoin,
            FeedbackAcceleration,
            KnownProcessSummaries,
        ),
    );
    let mut extension = NoExtension;
    let analysis = AnalysisEngine::new(&values, &policies, &mut extension)
        .run(
            AnalysisInput::new(world, input).unwrap(),
            ExecutionConfig {
                root_terminal_coverage: false,
                max_work: 1_000_000_000,
                max_call_depth: limit("max_call_depth"),
                max_memory_bytes: limit("max_memory_bytes"),
                use_summaries: limits["use_summaries"].as_bool().unwrap(),
                analysis: Config {
                    max_states: limit("max_states"),
                    max_transfers: limit("max_transfers"),
                    context_depth: limit("context_depth"),
                },
            },
            &Control::default(),
        )
        .unwrap();
    assert!(analysis.entry().environment.caller.as_concrete().is_none());
    assert!(analysis.entry().environment.value.contains(U256::MAX));
    assert!(
        analysis
            .entry()
            .environment
            .calldata
            .len()
            .contains(U256::from(65536))
    );
    let mut epochs = BTreeSet::new();
    let mut sources = BTreeSet::new();
    let mut owners = BTreeSet::new();
    let mut calls = 0;
    let mut current_deliveries = 0;
    let mut creation_cases = 0;
    let mut actions = BTreeSet::new();
    let mut visits = 0;
    for record in analysis.procedure_summaries() {
        calls += record.regional_cfg.calls.len();
        for procedure in &record.procedures {
            if let Some(key) = &procedure.frame.process {
                epochs.insert(key.epoch);
                sources.insert(key.source);
                owners.insert(key.owner);
            }
        }
        for node in &record.regional_cfg.nodes {
            if let Some(t) = &node.transfer {
                visits += usize::from(t.visited_block);
                if let Some(p) = &t.process {
                    actions.extend(p.actions.iter().copied());
                }
            }
        }
        for call in &record.regional_cfg.calls {
            for d in &call.deliveries {
                current_deliveries += usize::from(d.current);
                creation_cases += d.creation.len();
            }
        }
    }
    eprintln!(
        "GLOBAL_PROCESS_SCREEN_JSON {}",
        serde_json::json!({"case":case,"status":analysis.status(),"work":analysis.work(),
        "config":analysis.config(),"outer_states":analysis.states().len(),"transfers":analysis.transfers(),"full_original_transaction_unknowns":true,
        "scope":"offline captured initial world; opt-in process semantics; native relation product with no SMT provider; functional diagnostic, not performance",
        "frontiers":analysis.frontiers().iter().map(|f|format!("{:?}",f.reason)).collect::<Vec<_>>(),
        "granularity":"whole process control and effects; no instruction or SSA visits", "visited_process_blocks":visits,
        "epochs":epochs,"sources":sources,"owners":owners,"actions":actions,"calls":calls,"current_deliveries":current_deliveries,"creation_cases":creation_cases,
        "records":analysis.procedure_summaries().iter().map(|r|serde_json::json!({"closed":r.closed,"source_current":r.source_current,"bodies":r.state_count,
            "procedures":r.procedures.len(),"calls":r.regional_cfg.calls.len()})).collect::<Vec<_>>()})
    );
    assert_eq!(visits, 0);
}

#[test]
#[ignore = "explicit guarded original-input process diagnostic; not a normal unit gate"]
fn original_usdc_unknown_input_process_screen() {
    run_capture("usdc", USDC_WORLD, USDC_REQUEST)
}
#[test]
#[ignore = "explicit guarded original-input process diagnostic; not a normal unit gate"]
fn original_weth_unknown_input_process_screen() {
    run_capture("weth", WETH_WORLD, WETH_REQUEST)
}
