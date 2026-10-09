//! RPC headers must reach execution, and fee observations share the same pin.

use super::{Reply, Server, healthy};
use crate::{
    Address, Fork, U256,
    analysis::{
        self, ExecutionConfig, Status,
        progress::{Event, Observer, Phase},
    },
    domain::AbstractValue,
    world::{
        Entry, EvmEnvironment,
        rpc::{self, RpcError},
    },
};
use alloy_primitives::B256;
use serde_json::{Value as Json, json};
use std::{sync::mpsc, thread};

fn block_reply(request: &Json) -> Json {
    if request["method"] == "eth_feeHistory" {
        assert_eq!(request["params"], json!(["0x1", "0x2a", []]));
        return json!({"jsonrpc":"2.0", "id":request["id"], "result": {"oldestBlock":"0x2a", "baseFeePerBlobGas":["0x123", "0x456"]}});
    }
    let mut reply = healthy(request);
    if request["method"] == "eth_getBlockByHash" || request["method"] == "eth_getBlockByNumber" {
        reply["result"]["excessBlobGas"] = json!("0x42");
        reply["result"]["blobGasUsed"] = json!("0x20000");
    }
    if request["method"] == "eth_getCode" {
        // NUMBER TIMESTAMP COINBASE PREVRANDAO GASLIMIT BASEFEE BLOBBASEFEE
        // CHAINID PUSH1(parent number) BLOCKHASH STOP.
        reply["result"] = json!("0x4342414445484a4660294000");
    }
    reply
}

fn entry() -> Entry {
    let address = Address::repeat_byte(0x22);
    Entry {
        address,
        environment: EvmEnvironment {
            to: address.into(),
            ..EvmEnvironment::default()
        },
    }
}

#[test]
fn pinned_header_values_reach_both_library_entrypoints_under_each_supported_fork() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(block_reply(request)));
        for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
            let mut input = server.input();
            input.fork = fork;
            let expected = vec![
                U256::from(42),
                U256::from(0x1234),
                U256::from_be_slice(Address::repeat_byte(0x33).as_slice()),
                U256::from_be_slice(B256::repeat_byte(0x44).as_slice()),
                U256::from(30_000_000),
                U256::from(7),
                U256::from(0x123),
                U256::from(1),
                U256::from_be_slice(B256::repeat_byte(0x10).as_slice()),
            ];
            let world = rpc::load(&input).unwrap();
            let snapshot = world.snapshot_environment().unwrap();
            assert_eq!(snapshot.number, U256::from(42));
            assert_eq!(snapshot.blob_base_fee, Some(U256::from(0x123)));
            // Arbitrary observed fee intentionally differs from a fork-only formula.
            let direct =
                analysis::analyze_world(world, entry(), ExecutionConfig::default()).unwrap();
            let acquired = analysis::analyze_rpc(&input, entry(), ExecutionConfig::default())
                .unwrap()
                .into_analysis();
            for analysis in [direct, acquired] {
                assert_eq!(analysis.status(), Status::Converged);
                assert_eq!(
                    analysis.states()[0]
                        .exit_stack
                        .iter()
                        .map(|word| word.singleton().unwrap())
                        .collect::<Vec<_>>(),
                    expected
                );
                assert_eq!(
                    analysis.entry().environment.number.singleton(),
                    Some(U256::from(42))
                );
                assert_eq!(analysis.entry().environment.value, AbstractValue::top());
                assert_eq!(
                    analysis.entry().environment.caller,
                    entry().environment.caller
                );
            }
        }
    });
}

#[test]
fn explicit_environment_overrides_survive_rpc_defaults_and_snapshot_stays_observed() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(block_reply(request)));
        let mut entry = entry();
        let environment = &mut entry.environment;
        environment.number = AbstractValue::constant(U256::from(99));
        environment.timestamp = AbstractValue::constant(U256::from(88));
        environment.coinbase = Address::repeat_byte(0x55).into();
        environment.prevrandao = AbstractValue::constant(U256::from(77));
        environment.gas_limit = AbstractValue::constant(U256::from(66));
        environment.base_fee = AbstractValue::constant(U256::from(55));
        environment.blob_base_fee = AbstractValue::constant(U256::from(44));
        environment.chain_id = Some(AbstractValue::constant(U256::from(33)));
        environment
            .block_hashes
            .insert(U256::from(41), B256::repeat_byte(0x66));
        let result =
            analysis::analyze_rpc(&server.input(), entry.clone(), ExecutionConfig::default())
                .unwrap()
                .into_analysis();
        assert_eq!(result.entry().environment, entry.environment);
        assert_eq!(
            result.world().snapshot_environment().unwrap().number,
            U256::from(42)
        );
    });
}

#[test]
fn missing_optional_fees_stay_unknown_and_required_headers_fail_before_accounts() {
    thread::scope(|scope| {
        for missing in [
            "parentHash",
            "number",
            "timestamp",
            "miner",
            "mixHash",
            "gasLimit",
            "optional",
        ] {
            let server = Server::new(scope, move |request| {
                let mut reply = healthy(request);
                if request["method"] == "eth_getBlockByHash" {
                    let object = reply["result"].as_object_mut().unwrap();
                    object.remove(if missing == "optional" {
                        "baseFeePerGas"
                    } else {
                        missing
                    });
                }
                Reply::Json(reply)
            });
            let result = rpc::load(&server.input());
            if missing == "optional" {
                let world = result.unwrap();
                assert_eq!(world.snapshot_environment().unwrap().base_fee, None);
                assert_eq!(world.snapshot_environment().unwrap().blob_base_fee, None);
                let analysis =
                    analysis::analyze_world(world, entry(), ExecutionConfig::default()).unwrap();
                assert_eq!(analysis.entry().environment.base_fee, AbstractValue::top());
                assert_eq!(
                    analysis.entry().environment.blob_base_fee,
                    AbstractValue::top()
                );
            } else {
                let error = result.unwrap_err();
                assert!(matches!(error, RpcError::Response { .. }));
                assert!(error.failure().json_line.is_some());
                assert_eq!(server.requests().len(), 2);
            }
        }
    });
}

#[test]
fn inconsistent_fee_or_header_observations_never_publish_a_snapshot() {
    thread::scope(|scope| {
        for failure in [
            "header",
            "reorg",
            "oldest",
            "fees",
            "unavailable",
            "one-blob-field",
        ] {
            let server = Server::new(scope, move |request| {
                let mut reply = block_reply(request);
                let id = request["id"].as_u64().unwrap();
                match failure {
                    "header" if request["method"] == "eth_getBlockByNumber" => {
                        reply["result"]["timestamp"] = json!("0x999")
                    }
                    "reorg" if request["method"] == "eth_getBlockByNumber" && id > 5 => {
                        reply["result"]["hash"] = json!(B256::repeat_byte(0x99))
                    }
                    "oldest" if request["method"] == "eth_feeHistory" => {
                        reply["result"]["oldestBlock"] = json!("0x29")
                    }
                    "fees" if request["method"] == "eth_feeHistory" => {
                        reply["result"]["baseFeePerBlobGas"] = json!(["0x1"])
                    }
                    "unavailable" if request["method"] == "eth_feeHistory" => {
                        reply = json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"fee history unavailable"}})
                    }
                    "one-blob-field" if id == 2 => {
                        reply["result"]
                            .as_object_mut()
                            .unwrap()
                            .remove("blobGasUsed");
                    }
                    _ => {}
                }
                Reply::Json(reply)
            });
            let error = rpc::load(&server.input()).unwrap_err();
            if failure == "reorg" {
                assert!(matches!(error, RpcError::BlockMismatch { .. }));
            }
            if failure == "unavailable" {
                assert_eq!(error.failure().rpc_code, Some(-32601));
            }
            assert!(
                !server
                    .requests()
                    .iter()
                    .any(|request| request["method"] == "eth_getCode")
            );
        }
    });
}

#[test]
fn rpc_progress_records_pin_acquisition_and_execution_without_changing_results() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(healthy(request)));
        let (sender, receiver) = mpsc::sync_channel(100);
        let result = analysis::analyze_rpc_with_observer(
            &server.input(),
            entry(),
            ExecutionConfig::default(),
            &Observer::new(sender),
        )
        .unwrap();
        let events: Vec<_> = receiver.try_iter().collect();
        for phase in [
            Phase::Validating,
            Phase::Pinning,
            Phase::Acquiring,
            Phase::Analyzing,
        ] {
            assert!(events.contains(&Event::Phase(phase)));
        }
        let stats = result.analysis().rpc_acquisition().unwrap();
        assert!(events.contains(&Event::Acquisition {
            round: 1,
            accounts: 1,
            slots: 1,
            requests: stats.requests
        }));
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Execution {
                states: 1,
                transfers: 1,
                ..
            }
        )));
    });
}

#[test]
fn nested_native_causes_survive_actual_rpc_code_and_snapshot_failures() {
    use rpc::{
        CodeFailure, DelegationFailure, FailureCause, HeaderField, HeaderValue, ResponseReason,
    };
    thread::scope(|scope| {
        for case in ["eof", "delegation", "header", "storage"] {
            let server = Server::new(scope, move |request| {
                let mut reply = healthy(request);
                match (case, request["method"].as_str().unwrap()) {
                    ("eof", "eth_getCode") => reply["result"] = json!("0xef0001"),
                    ("delegation", "eth_getCode") => {
                        reply["result"] = json!(format!("0xef0101{}", "55".repeat(20)))
                    }
                    ("header", "eth_getBlockByHash") if request["id"].as_u64().unwrap() > 2 => {
                        reply["result"]["timestamp"] = json!("0x999")
                    }
                    ("storage", "eth_getStorageAt") => reply["result"] = json!("0x00"),
                    _ => {}
                }
                Reply::Json(reply)
            });
            let mut input = server.input();
            input.fork = Fork::Prague;
            let failure = rpc::load(&input).unwrap_err().failure();
            let expected = match case {
                "eof" => FailureCause::Code(CodeFailure::UnsupportedEof),
                "delegation" => FailureCause::Code(CodeFailure::InvalidDelegation(
                    DelegationFailure::UnsupportedVersion,
                )),
                "header" => FailureCause::Response(ResponseReason::HeaderChanged {
                    field: HeaderField::Timestamp,
                    expected: HeaderValue::Quantity(U256::from(0x1234)),
                    observed: HeaderValue::Quantity(U256::from(0x999)),
                }),
                "storage" => FailureCause::Response(ResponseReason::StorageLength {
                    expected: 32,
                    observed: 1,
                }),
                _ => unreachable!(),
            };
            assert_eq!(failure.cause, Some(expected));
            let serialized = serde_json::to_string(&failure).unwrap();
            assert!(!serialized.contains(&server.endpoint));
        }
    });
}

#[test]
fn world_hash_failure_keeps_original_account_and_both_hashes_in_serialized_evidence() {
    use crate::world::{Account, World};
    use rpc::{FailureCause, RpcContext, WorldFailure};
    let address = Address::repeat_byte(0x22);
    let expected = B256::repeat_byte(0x55);
    let observed = alloy_primitives::keccak256([0]);
    let mut world = World::new(Fork::Osaka, "code hash mismatch");
    let source = world
        .insert_with_code_hash(
            address,
            Account::from_hex("00", Fork::Osaka).unwrap(),
            expected,
        )
        .unwrap_err();
    assert!(world.accounts().is_empty());
    let failure = RpcError::World {
        context: Box::new(RpcContext {
            chain_id: Some(U256::from(1)),
            block_hash: Some(B256::repeat_byte(0x11)),
            method: "eth_getCode",
            account: Some(address),
            slot: None,
        }),
        source,
    }
    .failure();
    assert_eq!(
        failure.cause,
        Some(FailureCause::World(WorldFailure::CodeHash {
            address,
            expected,
            observed
        }))
    );
}
