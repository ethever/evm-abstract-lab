//! Incremental storage acquisition exercises the checked initial-state cache.

use super::{Reply, Server, healthy};
use crate::{
    Address, U256,
    domain::Value,
    world::{
        Existence,
        rpc::{AcquisitionLimit, RpcBlock, RpcError, RpcFailureKind, Session},
    },
};
use alloy_primitives::B256;
use serde_json::{Value as Json, json};
use std::{collections::BTreeSet, thread, time::Duration};

fn storage_reply(request: &Json) -> Json {
    let mut reply = healthy(request);
    if request["method"] == "eth_getStorageAt" {
        reply["result"] = json!(format!(
            "0x{:064x}",
            if request["params"][1] == "0x1" {
                U256::ZERO
            } else {
                U256::from(13)
            }
        ));
    }
    reply
}

#[test]
fn incremental_storage_reuses_observations_and_pins_only_missing_slots() {
    thread::scope(|scope| {
        for block in [
            RpcBlock::Latest,
            RpcBlock::Number(42),
            RpcBlock::Hash(B256::repeat_byte(0x11)),
        ] {
            let server = Server::new(scope, |request| Reply::Json(storage_reply(request)));
            let mut input = server.input();
            input.block = block;
            let mut session = Session::load(&input).unwrap();
            let address = Address::repeat_byte(0x22);
            let initial = session.world().account(address).unwrap().clone();
            let slots = BTreeSet::from([U256::ZERO, U256::from(1), U256::from(2)]);
            assert_eq!(
                session.fetch_storage(address, &slots).unwrap(),
                vec![U256::from(1), U256::from(2)]
            );
            let requests = session.requests();
            assert_eq!(requests, 14);
            assert!(session.fetch_storage(address, &slots).unwrap().is_empty());
            assert_eq!(session.requests(), requests);
            let account = session.world().account(address).unwrap();
            assert_eq!(account.code, initial.code);
            assert_eq!(account.balance, initial.balance);
            assert_eq!(account.nonce, initial.nonce);
            assert_eq!(account.storage[&U256::ZERO], initial.storage[&U256::ZERO]);
            assert_eq!(account.storage[&U256::from(1)], Value::constant(U256::ZERO));
            assert_eq!(
                account.storage[&U256::from(2)],
                Value::constant(U256::from(13))
            );
            assert!(account.storage_unknown);
            let observed = server.requests();
            assert_eq!(observed.len(), requests);
            for request in &observed[8..] {
                match request["method"].as_str().unwrap() {
                    "eth_getStorageAt" => assert_eq!(
                        request["params"][2],
                        json!({"blockHash": B256::repeat_byte(0x11), "requireCanonical": true})
                    ),
                    "eth_chainId" => assert_eq!(request["params"], json!([])),
                    "eth_getBlockByNumber" => {
                        assert_eq!(request["params"], json!(["0x2a", false]))
                    }
                    method => panic!("storage discovery refetched account method {method}"),
                }
            }
            assert_eq!(
                observed
                    .iter()
                    .filter(|request| request["method"] == "eth_getStorageAt")
                    .count(),
                3
            );
            assert_eq!(
                observed
                    .iter()
                    .filter(|request| request["method"] == "eth_getBlockByNumber")
                    .count(),
                2 + usize::from(!matches!(block, RpcBlock::Hash(_)))
            );
            assert_eq!(
                observed
                    .iter()
                    .filter(|request| request["method"] == "eth_getBlockByNumber"
                        && request["params"][0] == "latest")
                    .count(),
                usize::from(matches!(block, RpcBlock::Latest))
            );
        }
    });
}

#[test]
fn incremental_storage_failures_retain_slot_provenance_without_partial_installation() {
    thread::scope(|scope| {
        let request_timeout = Duration::from_millis(500);
        for case in [
            "remote", "null", "short", "long", "id", "http", "json", "limit", "timeout",
        ] {
            let server = Server::new(scope, move |request| {
                if request["method"] != "eth_getStorageAt" || request["params"][1] != "0x2" {
                    return Reply::Json(storage_reply(request));
                }
                let mut response = storage_reply(request);
                match case {
                    "remote" => Reply::Json(json!({"jsonrpc":"2.0", "id":request["id"],
                        "error":{"code":-32602,"message":"blockHash unsupported"}})),
                    "null" => {
                        response["result"] = Json::Null;
                        Reply::Json(response)
                    }
                    "short" => {
                        response["result"] = json!("0x00");
                        Reply::Json(response)
                    }
                    "long" => {
                        response["result"] = json!(format!("0x{}", "00".repeat(33)));
                        Reply::Json(response)
                    }
                    "id" => {
                        response["id"] = json!(999);
                        Reply::Json(response)
                    }
                    "http" => Reply::Http(503),
                    "json" => Reply::Bytes(b"not json".to_vec()),
                    "limit" => Reply::Bytes(vec![b' '; 1025]),
                    "timeout" => Reply::Delay(request_timeout.saturating_mul(2)),
                    _ => unreachable!(),
                }
            });
            let mut input = server.input();
            input.max_response_bytes = 1024;
            if case == "timeout" {
                input.timeout = request_timeout;
            }
            let mut session = Session::load(&input).unwrap();
            let address = Address::repeat_byte(0x22);
            let initial = session.world().account(address).unwrap().clone();
            let error = session
                .fetch_storage(address, &BTreeSet::from([U256::from(1), U256::from(2)]))
                .unwrap_err();
            if case == "timeout" {
                match &error {
                    RpcError::Transport { source, .. } => assert!(source.is_timeout()),
                    other => panic!("expected incremental storage timeout, got {other}"),
                }
            }
            assert_eq!(session.world().account(address), Some(&initial));
            assert_eq!(session.requests(), 12);
            let context = error.context();
            assert_eq!(context.method, "eth_getStorageAt");
            assert_eq!(context.account, Some(address));
            assert_eq!(context.slot, Some(U256::from(2)));
            assert_eq!(context.chain_id, Some(U256::from(1)));
            assert_eq!(context.block_hash, Some(B256::repeat_byte(0x11)));
            assert_eq!(error.failure().context, *context);
            let kind = match case {
                "remote" => RpcFailureKind::Remote,
                "null" => RpcFailureKind::MissingResult,
                "short" | "long" | "id" => RpcFailureKind::Response,
                "http" => RpcFailureKind::Http,
                "json" => RpcFailureKind::Json,
                "limit" => RpcFailureKind::ResponseLimit,
                "timeout" => {
                    assert!(matches!(
                        error.kind(),
                        RpcFailureKind::Transport | RpcFailureKind::Read
                    ));
                    error.kind()
                }
                _ => unreachable!(),
            };
            assert_eq!(error.kind(), kind);
            let observed = server.requests();
            assert_eq!(observed.len(), session.requests());
            assert_eq!(observed.last().unwrap()["method"], "eth_getStorageAt");
            assert!(
                observed
                    .iter()
                    .all(|request| !request.to_string().contains("latest"))
            );
        }
    });
}

#[test]
fn incremental_storage_final_identity_failures_discard_acquired_slots() {
    thread::scope(|scope| {
        for failing_method in ["eth_chainId", "eth_getBlockByNumber"] {
            let server = Server::new(scope, move |request| {
                let mut response = storage_reply(request);
                let id = request["id"].as_u64().unwrap();
                if request["method"] == failing_method && id >= 12 {
                    if failing_method == "eth_chainId" {
                        response["result"] = json!("0x2");
                    } else {
                        response["result"]["hash"] = json!(B256::repeat_byte(0x33));
                    }
                }
                Reply::Json(response)
            });
            let mut session = Session::load(&server.input()).unwrap();
            let address = Address::repeat_byte(0x22);
            let initial = session.world().account(address).unwrap().clone();
            let error = session
                .fetch_storage(address, &BTreeSet::from([U256::from(1)]))
                .unwrap_err();
            assert_eq!(session.world().account(address), Some(&initial));
            assert_eq!(error.context().method, failing_method);
            assert_eq!(error.context().chain_id, Some(U256::from(1)));
            assert_eq!(error.context().block_hash, Some(B256::repeat_byte(0x11)));
            assert_eq!(
                error.kind(),
                if failing_method == "eth_chainId" {
                    RpcFailureKind::ChainMismatch
                } else {
                    RpcFailureKind::BlockMismatch
                }
            );
            assert_eq!(session.requests(), server.requests().len());
        }
    });
}

#[test]
fn incremental_storage_reorg_after_state_read_keeps_orphan_facts_out_of_cache() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| {
            let mut response = storage_reply(request);
            if request["method"] == "eth_getBlockByNumber" {
                assert_eq!(request["params"], json!(["0x2a", false]));
                if request["id"].as_u64().unwrap() >= 13 {
                    response["result"]["hash"] = json!(B256::repeat_byte(0x33));
                }
            }
            // The endpoint can still return the orphan block by its old hash.
            if request["method"] == "eth_getBlockByHash" {
                assert_eq!(response["result"]["hash"], json!(B256::repeat_byte(0x11)));
            }
            Reply::Json(response)
        });
        let mut session = Session::load(&server.input()).unwrap();
        let address = Address::repeat_byte(0x22);
        let initial = session.world().account(address).unwrap().clone();
        let error = session
            .fetch_storage(address, &BTreeSet::from([U256::from(1)]))
            .unwrap_err();
        assert_eq!(error.kind(), RpcFailureKind::BlockMismatch);
        assert_eq!(error.context().method, "eth_getBlockByNumber");
        assert_eq!(error.context().chain_id, Some(U256::from(1)));
        assert_eq!(error.context().block_hash, Some(B256::repeat_byte(0x11)));
        assert_eq!(session.world().account(address), Some(&initial));
        assert_eq!(session.requests(), 13);
        let observed = server.requests();
        assert_eq!(observed.last().unwrap()["method"], "eth_getBlockByNumber");
        assert_eq!(
            observed
                .iter()
                .filter(|request| request["method"] == "eth_getStorageAt"
                    && request["params"][1] == "0x1")
                .count(),
            1
        );
    });
}

#[test]
fn storage_requires_a_valid_pinned_height_without_restricting_code_acquisition() {
    thread::scope(|scope| {
        for block in [RpcBlock::Latest, RpcBlock::Hash(B256::repeat_byte(0x11))] {
            for malformed in [false, true] {
                let server = Server::new(scope, move |request| {
                    let mut response = storage_reply(request);
                    if request["method"] == "eth_getBlockByHash"
                        || request["method"] == "eth_getBlockByNumber"
                    {
                        if malformed {
                            response["result"]["number"] = json!("0x02a");
                        } else {
                            response["result"].as_object_mut().unwrap().remove("number");
                        }
                    }
                    Reply::Json(response)
                });
                let mut input = server.input();
                input.block = block;
                input.accounts[0].slots.clear();
                let mut session = Session::load(&input).unwrap();
                assert!(session.fetch_account(Address::repeat_byte(0x44)).unwrap());
                let requests = session.requests();
                let address = Address::repeat_byte(0x22);
                let initial = session.world().account(address).unwrap().clone();
                let error = session
                    .fetch_storage(address, &BTreeSet::from([U256::from(1)]))
                    .unwrap_err();
                assert_eq!(error.kind(), RpcFailureKind::Response);
                assert_eq!(
                    error.context().method,
                    if matches!(block, RpcBlock::Hash(_)) {
                        "eth_getBlockByHash"
                    } else {
                        "eth_getBlockByNumber"
                    }
                );
                assert_eq!(error.context().chain_id, Some(U256::from(1)));
                assert_eq!(error.context().block_hash, Some(B256::repeat_byte(0x11)));
                assert_eq!(session.world().account(address), Some(&initial));
                assert_eq!(session.requests(), requests);
                assert_eq!(server.requests().len(), requests);
            }
        }
    });
}

#[test]
fn canonical_storage_check_rejects_a_wrong_or_missing_returned_height() {
    thread::scope(|scope| {
        for missing in [false, true] {
            let server = Server::new(scope, move |request| {
                let mut response = storage_reply(request);
                if request["method"] == "eth_getBlockByNumber"
                    && request["id"].as_u64().unwrap() >= 13
                {
                    if missing {
                        response["result"].as_object_mut().unwrap().remove("number");
                    } else {
                        response["result"]["number"] = json!("0x2b");
                    }
                }
                Reply::Json(response)
            });
            let mut session = Session::load(&server.input()).unwrap();
            let address = Address::repeat_byte(0x22);
            let initial = session.world().account(address).unwrap().clone();
            let error = session
                .fetch_storage(address, &BTreeSet::from([U256::from(1)]))
                .unwrap_err();
            assert_eq!(error.kind(), RpcFailureKind::Response);
            assert_eq!(error.context().method, "eth_getBlockByNumber");
            assert_eq!(session.world().account(address), Some(&initial));
            assert_eq!(session.requests(), 13);
        }
    });
}

#[test]
fn incremental_storage_retry_reacquires_the_entire_failed_batch() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| {
            if request["id"] == 12 {
                return Reply::Json(json!({
                    "jsonrpc":"2.0", "id":request["id"],
                    "error":{"code":-32000,"message":"state temporarily unavailable"}
                }));
            }
            Reply::Json(storage_reply(request))
        });
        let mut session = Session::load(&server.input()).unwrap();
        let address = Address::repeat_byte(0x22);
        let slots = BTreeSet::from([U256::ZERO, U256::from(1), U256::from(2)]);
        assert!(session.fetch_storage(address, &slots).is_err());
        assert_eq!(session.requests(), 12);
        assert_eq!(
            session.fetch_storage(address, &slots).unwrap(),
            vec![U256::from(1), U256::from(2)]
        );
        assert_eq!(session.requests(), 18);
        let observed = server.requests();
        for (slot, count) in [("0x0", 1), ("0x1", 2), ("0x2", 2)] {
            assert_eq!(
                observed
                    .iter()
                    .filter(|request| request["method"] == "eth_getStorageAt"
                        && request["params"][1] == slot)
                    .count(),
                count
            );
        }
        assert_eq!(
            session.world().account(address).unwrap().storage[&U256::from(1)],
            Value::constant(U256::ZERO)
        );
    });
}

#[test]
fn incremental_storage_request_budget_covers_slots_and_final_checks() {
    thread::scope(|scope| {
        for max_requests in [11, 13] {
            let server = Server::new(scope, |request| Reply::Json(storage_reply(request)));
            let mut input = server.input();
            input.max_requests = max_requests;
            let mut session = Session::load(&input).unwrap();
            let address = Address::repeat_byte(0x22);
            let initial = session.world().account(address).unwrap().clone();
            let error = session
                .fetch_storage(address, &BTreeSet::from([U256::from(1), U256::from(2)]))
                .unwrap_err();
            assert!(matches!(error, RpcError::AcquisitionLimit {
                resource: AcquisitionLimit::Requests,
                limit,
                ..
            } if limit == max_requests));
            assert_eq!(
                error.context().method,
                if max_requests == 11 {
                    "eth_getStorageAt"
                } else {
                    "eth_getBlockByNumber"
                }
            );
            assert_eq!(
                error.context().slot,
                if max_requests == 11 {
                    Some(U256::from(2))
                } else {
                    None
                }
            );
            assert_eq!(session.world().account(address), Some(&initial));
            assert_eq!(session.requests(), max_requests);
            assert_eq!(server.requests().len(), max_requests);
        }
    });
}

#[test]
fn incremental_storage_requires_an_observed_account_without_networking() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(storage_reply(request)));
        let mut session = Session::load(&server.input()).unwrap();
        let requests = session.requests();
        let address = Address::repeat_byte(0x44);
        let error = session
            .fetch_storage(address, &BTreeSet::from([U256::from(1)]))
            .unwrap_err();
        assert_eq!(error.kind(), RpcFailureKind::Configuration);
        assert_eq!(error.context().method, "eth_getStorageAt");
        assert_eq!(error.context().account, Some(address));
        assert_eq!(error.context().slot, Some(U256::from(1)));
        assert_eq!(session.requests(), requests);
        assert_eq!(server.requests().len(), requests);
    });
}

#[test]
fn incremental_storage_nonzero_observation_establishes_account_presence() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| {
            let mut reply = storage_reply(request);
            match request["method"].as_str().unwrap() {
                "eth_getCode" => reply["result"] = json!("0x"),
                "eth_getBalance" | "eth_getTransactionCount" => reply["result"] = json!("0x0"),
                "eth_getStorageAt" if request["params"][1] == "0x0" => {
                    reply["result"] = json!(format!("0x{:064x}", U256::ZERO))
                }
                _ => {}
            }
            Reply::Json(reply)
        });
        let mut session = Session::load(&server.input()).unwrap();
        let address = Address::repeat_byte(0x22);
        assert_eq!(
            session.world().account(address).unwrap().existence,
            Existence::Unknown
        );
        session
            .fetch_storage(address, &BTreeSet::from([U256::from(1)]))
            .unwrap();
        assert_eq!(
            session.world().account(address).unwrap().existence,
            Existence::Unknown
        );
        session
            .fetch_storage(address, &BTreeSet::from([U256::from(2)]))
            .unwrap();
        assert_eq!(
            session.world().account(address).unwrap().existence,
            Existence::Present
        );
    });
}

#[test]
fn incremental_storage_discovery_is_bounded_by_requests_beyond_initial_slot_admission() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(storage_reply(request)));
        let mut session = Session::load(&server.input()).unwrap();
        let slots: BTreeSet<_> = (1u64..=1025).map(U256::from).collect();
        let installed = session
            .fetch_storage(Address::repeat_byte(0x22), &slots)
            .unwrap();
        assert_eq!(installed.len(), 1025);
        assert_eq!(
            session
                .world()
                .account(Address::repeat_byte(0x22))
                .unwrap()
                .storage
                .len(),
            1026
        );
        assert_eq!(session.requests(), 8 + 4 + 1025);
        assert_eq!(session.requests(), server.requests().len());
    });
}
