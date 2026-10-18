//! Real public RPC entry against a local fixture, never a chain endpoint.

use super::{Reply, Server, healthy};
use crate::{
    Address, U256,
    analysis::{ExecutionConfig, OutcomeKind, RpcStorageSlot, Status, analyze_rpc},
    domain::{AbstractValue, Domain},
    world::Entry,
};
use serde_json::json;
use std::thread;

#[test]
fn public_rpc_route_injects_known_zero_while_unrequested_slots_and_inputs_stay_unknown() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| {
            let mut response = healthy(request);
            match request["method"].as_str().unwrap() {
                "eth_getCode" => response["result"] = json!("0x60026003016008565b5f545f5260205ff3"),
                "eth_getStorageAt" => response["result"] = json!(format!("0x{:064x}", U256::ZERO)),
                _ => {}
            }
            Reply::Json(response)
        });
        let mut input = server.input();
        input.accounts[0].slots.clear();
        let owner = input.accounts[0].address;
        let report = analyze_rpc(&input, Entry::new(owner), ExecutionConfig::default()).unwrap();
        let analysis = report.analysis();
        assert_eq!(
            analysis.status(),
            Status::Converged,
            "{:?}",
            analysis.frontiers()
        );
        let acquisition = analysis.rpc_acquisition().unwrap();
        assert_eq!(acquisition.rounds, 2);
        assert_eq!(
            acquisition.fetched_storage,
            [RpcStorageSlot {
                address: owner,
                slot: U256::ZERO
            }]
        );
        let account = analysis.world().account(owner).unwrap();
        assert!(account.storage_unknown);
        assert_eq!(account.storage[&U256::ZERO].singleton(), Some(U256::ZERO));
        assert!(
            analysis
                .entry()
                .environment
                .calldata
                .len()
                .singleton()
                .is_none()
        );
        let returns: Vec<_> = analysis
            .outcomes()
            .iter()
            .filter(|o| o.kind == OutcomeKind::Return)
            .collect();
        assert!(!returns.is_empty());
        for outcome in returns {
            assert_eq!(
                outcome
                    .data
                    .read_word(&AbstractValue::constant(U256::ZERO), Domain::default())
                    .singleton(),
                Some(U256::ZERO)
            );
            assert!(
                outcome
                    .store
                    .read(
                        owner,
                        &AbstractValue::constant(U256::from(1)),
                        Domain::default()
                    )
                    .singleton()
                    .is_none()
            );
        }
        assert!(
            analysis
                .world()
                .account(Address::repeat_byte(0x77))
                .is_none()
        );
    });
}

#[test]
fn public_rpc_injection_after_runtime_write_retains_rollback_initial_roots() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| {
            let mut response = healthy(request);
            match request["method"].as_str().unwrap() {
                // Write slot 0, demand initial slot 1, then revert this transaction.
                "eth_getCode" => response["result"] = json!("0x60095f55600154505f5ffd"),
                "eth_getStorageAt" => {
                    response["result"] = json!(format!("0x{:064x}", U256::from(3)))
                }
                _ => {}
            }
            Reply::Json(response)
        });
        let mut input = server.input();
        input.accounts[0].slots.clear();
        let owner = input.accounts[0].address;
        let report = analyze_rpc(&input, Entry::new(owner), ExecutionConfig::default()).unwrap();
        let analysis = report.analysis();
        assert_eq!(
            analysis.status(),
            Status::Converged,
            "{:?}",
            analysis.frontiers()
        );
        assert_eq!(
            analysis.rpc_acquisition().unwrap().fetched_storage,
            [RpcStorageSlot {
                address: owner,
                slot: U256::from(1)
            }]
        );
        assert!(
            !analysis
                .world()
                .account(owner)
                .unwrap()
                .storage
                .contains_key(&U256::ZERO)
        );
        let exit = analysis
            .states()
            .iter()
            .filter_map(|state| state.exit.as_ref())
            .next()
            .unwrap();
        assert_eq!(
            exit.store
                .read(
                    owner,
                    &AbstractValue::constant(U256::ZERO),
                    Domain::default()
                )
                .singleton(),
            Some(U256::from(9))
        );
        let reverted: Vec<_> = analysis
            .outcomes()
            .iter()
            .filter(|o| o.kind == OutcomeKind::Revert)
            .collect();
        assert!(!reverted.is_empty());
        for outcome in reverted {
            assert!(
                outcome
                    .store
                    .read(
                        owner,
                        &AbstractValue::constant(U256::ZERO),
                        Domain::default()
                    )
                    .singleton()
                    .is_none()
            );
            assert_eq!(
                outcome
                    .store
                    .read(
                        owner,
                        &AbstractValue::constant(U256::from(1)),
                        Domain::default()
                    )
                    .singleton(),
                Some(U256::from(3))
            );
        }
    });
}
