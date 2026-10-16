//! A concrete subset of issue123's original Unknown input. Unobserved initial
//! slots remain unknown in the abstract world; revm chooses zero for one witness.
use super::{oracle, support::*};
use alloy_primitives::{Address, B256, U256, hex};
use evm_abstract::{
    Fork,
    analysis::{Config, ExecutionConfig, OutcomeKind},
    domain::{AbstractValue, Domain, Profile, relational::RelationLimits},
    world::{Account, AddressInput, ByteArray, Entry, World},
};
use revm_bytecode::opcode;
use serde_json::Value;
use std::str::FromStr;

const OBSERVATIONS: &str =
    include_str!("../../../../examples/known-recursion/captured/usdc-26180958-observed-world.json");
const WITNESS: &str =
    include_str!("../../../../examples/known-recursion/captured/usdc-100-byte-witness.json");
const ORIGINAL_REQUEST: &str =
    include_str!("../../../../examples/known-recursion/captured/usdc-original-request.json");
const MANIFEST: &str = include_str!("../../../../examples/known-recursion/captured/manifest.json");

fn scalar(value: &Value) -> U256 {
    U256::from_str(value.as_str().unwrap()).unwrap()
}

pub(super) fn observed_world() -> World {
    let data: Value = serde_json::from_str(OBSERVATIONS).unwrap();
    assert_eq!(data["fork"], "osaka");
    let mut world = World::anchored(
        Fork::Osaka,
        scalar(&data["identity"]["chain_id"]),
        B256::from_str(data["identity"]["block_hash"].as_str().unwrap()).unwrap(),
        data["provenance"].as_str().unwrap(),
    );
    for item in data["accounts"].as_array().unwrap() {
        let owner = Address::from_str(item["address"].as_str().unwrap()).unwrap();
        let mut account = Account::from_hex(item["code"].as_str().unwrap(), world.fork()).unwrap();
        account.balance = AbstractValue::constant(scalar(&item["balance"]));
        account.nonce = AbstractValue::constant(scalar(&item["nonce"]));
        assert_eq!(item["existence"], "present");
        account.storage_unknown = item["storage_unknown"].as_bool().unwrap();
        assert!(
            account.storage_unknown,
            "unobserved slots must remain unknown"
        );
        for (slot, value) in item["storage"].as_object().unwrap() {
            account.storage.insert(
                U256::from_str(slot).unwrap(),
                AbstractValue::constant(scalar(value)),
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
    world
}

pub(super) fn witness_entry() -> Entry {
    let witness: Value = serde_json::from_str(WITNESS).unwrap();
    let owner = Address::from_str(witness["to"].as_str().unwrap()).unwrap();
    let caller = Address::from_str(witness["caller"].as_str().unwrap()).unwrap();
    let calldata = hex::decode(witness["calldata"].as_str().unwrap()).unwrap();
    assert_eq!(calldata.len(), 100);
    let mut input = Entry::concrete(
        owner,
        caller,
        AbstractValue::constant(scalar(&witness["value"])),
        ByteArray::exact(&calldata),
    );
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let snapshot = &manifest["cases"]["usdc"]["captured_snapshot"];
    input.environment.coinbase =
        AddressInput::Concrete(Address::from_str(snapshot["coinbase"].as_str().unwrap()).unwrap());
    input.environment.timestamp = AbstractValue::constant(scalar(&snapshot["timestamp"]));
    input.environment.number = AbstractValue::constant(scalar(&snapshot["number"]));
    input.environment.prevrandao = AbstractValue::constant(scalar(&snapshot["prevrandao"]));
    input.environment.gas_limit = AbstractValue::constant(scalar(&snapshot["gas_limit"]));
    input.environment.base_fee = AbstractValue::constant(scalar(&snapshot["base_fee"]));
    input.environment.blob_base_fee = AbstractValue::constant(scalar(&snapshot["blob_base_fee"]));
    input.environment.chain_id = Some(AbstractValue::constant(scalar(&snapshot["chain_id"])));
    input.environment.block_hashes.insert(
        scalar(&snapshot["number"]) - U256::from(1),
        B256::from_str(snapshot["parent_hash"].as_str().unwrap()).unwrap(),
    );
    input
}

/// Replay the captured request's retained execution/domain/retention limits.
/// Native-provider fields are asserted only as historical raw-capture evidence;
/// cost_version3 runs local reasoning, so its work is not old SMT timing/fuel.
/// Synthetic tests retain the complete library default (depth32/work20M).
fn original_policy() -> ExecutionConfig {
    let request: Value = serde_json::from_str(ORIGINAL_REQUEST).unwrap();
    let limits = &request["limits"];
    let limit = |name: &str| usize::try_from(limits[name].as_u64().unwrap()).unwrap();
    assert_eq!(limits["domain_profile"], "Product");
    assert_eq!(limits["smt_provider"], "Z3");
    assert!(limits["smt_rlimit"].as_u64().is_some_and(|fuel| fuel > 0));
    ExecutionConfig {
        max_work: limit("max_work"),
        max_call_depth: limit("max_call_depth"),
        max_memory_bytes: limit("max_memory_bytes"),
        use_summaries: limits["use_summaries"].as_bool().unwrap(),
        analysis: Config {
            domain_profile: Profile::Product,
            max_states: limit("max_states"),
            max_transfers: limit("max_transfers"),
            context_depth: limit("context_depth"),
            max_constants: limit("max_constants"),
            reduction_rounds: limit("reduction_rounds"),
            max_facts: limit("max_facts"),
            relations: RelationLimits {
                enabled: limits["relations_enabled"].as_bool().unwrap(),
                max_constraints: limit("max_constraints"),
                max_nodes: limit("max_expression_nodes"),
                max_depth: limit("max_expression_depth"),
            },
        },
    }
}

#[test]
fn captured_usdc_self_delegate_reverts_and_restores_observed_initial_state() {
    let world = observed_world();
    let input = witness_entry();
    let owner = input.address;
    let caller = input.environment.caller.as_concrete().unwrap();
    let implementation_slot =
        U256::from_str("0x7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c3")
            .unwrap();
    let initial = world.account(owner).unwrap().storage[&implementation_slot]
        .singleton()
        .unwrap();
    let observed = oracle::execute(&world, &input);
    eprintln!(
        "USDC concrete witness: kind={:?} steps={} max_depth={} depth_failures={} emitted_logs={} committed_logs={}",
        observed.kind,
        observed.steps.len(),
        observed.steps.iter().map(|s| s.depth).max().unwrap(),
        observed.depth_failures,
        observed.emitted_logs.len(),
        observed.logs.len()
    );
    assert_eq!(observed.kind, OutcomeKind::Revert);
    assert_eq!(observed.storage[&(owner, implementation_slot)], initial);
    assert!(
        observed.logs.is_empty(),
        "the parent's emitted log must roll back"
    );
    assert!(
        observed
            .emitted_logs
            .iter()
            .any(|(address, _, _)| *address == owner)
    );
    assert!(observed.steps.iter().any(|s| s.code == owner
        && s.owner == owner
        && s.caller == caller
        && s.depth == 1
        && s.pc == 742
        && s.opcode == opcode::CALL));
    assert!(observed.steps.iter().any(|s| s.code == owner
        && s.owner == owner
        && s.caller == owner
        && s.depth > 1
        && s.pc == 1680
        && s.opcode == opcode::DELEGATECALL));
    assert!(observed.steps.iter().any(|s| s.code == owner
        && s.owner == owner
        && s.pc == 2119
        && s.storage_write == Some((implementation_slot, U256::from_be_slice(owner.as_slice())))));

    let analysis = run_config(world, input, original_policy());
    complete_recursion(&analysis);
    for procedure in analysis
        .procedure_summaries()
        .iter()
        .flat_map(|r| &r.procedures)
    {
        assert_eq!(procedure.frame.code_address, owner);
        assert_eq!(procedure.frame.address, owner);
        assert_eq!(procedure.frame.caller, AddressInput::Concrete(owner));
        assert!(!procedure.frame.is_static);
        assert_eq!(
            procedure
                .input
                .read(
                    owner,
                    &AbstractValue::constant(implementation_slot),
                    Domain::default()
                )
                .singleton(),
            Some(U256::from_be_slice(owner.as_slice()))
        );
    }
    for outcome in terminals(&analysis, OutcomeKind::Revert) {
        assert_eq!(
            outcome
                .store
                .read(
                    owner,
                    &AbstractValue::constant(implementation_slot),
                    Domain::default()
                )
                .singleton(),
            Some(initial)
        );
        assert!(!outcome.store.logs_unknown());
        assert!(outcome.store.possible_logs().is_empty());
        for (account_address, account) in analysis.world().accounts() {
            assert_eq!(outcome.store.code(*account_address), Some(&account.code));
            assert_eq!(
                outcome.store.nonce(*account_address).singleton(),
                account.nonce.singleton()
            );
            assert_eq!(
                outcome.store.read_balance(*account_address).singleton(),
                account.balance.singleton()
            );
            assert_eq!(outcome.store.existence(*account_address), account.existence);
            assert_eq!(
                outcome.store.created_in_transaction(*account_address),
                Some(false)
            );
            assert_eq!(
                outcome.store.pending_destruction(*account_address),
                Some(false)
            );
            for (slot, value) in &account.storage {
                assert_eq!(
                    outcome
                        .store
                        .read(
                            *account_address,
                            &AbstractValue::constant(*slot),
                            Domain::default()
                        )
                        .singleton(),
                    value.singleton()
                );
            }
            // This unobserved slot must not acquire an invented zero baseline.
            let unobserved = outcome
                .store
                .read(*account_address, &word(0), Domain::default());
            assert!(unobserved.numeric().is_top());
            only(
                &outcome
                    .store
                    .read_transient(*account_address, &word(0), Domain::default()),
                &[0],
            );
        }
    }
    observed.covers(&analysis);
}
