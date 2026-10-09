//! Cancellation drops both the pending operation and its connection reactor.

use super::{Reply, Server, healthy};
use crate::{
    Address, U256,
    analysis::{self, ExecutionConfig, control::Control, progress::Observer},
    world::{
        Entry, EvmEnvironment,
        rpc::{RpcError, Session},
    },
};
use std::{
    collections::BTreeSet,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[test]
fn cancelling_pending_headers_or_body_closes_socket_and_installs_no_batch_facts() {
    for body in [false, true] {
        thread::scope(|scope| {
            let (started, received) = mpsc::channel();
            let (closed, disconnected) = mpsc::channel();
            let server = Server::new(scope, move |request| {
                if request["method"] == "eth_getStorageAt" && request["params"][1] == "0x2" {
                    Reply::Hold {
                        started: started.clone(),
                        closed: closed.clone(),
                        body,
                    }
                } else {
                    Reply::Json(healthy(request))
                }
            });
            let control = Control::new(Observer::default());
            let mut input = server.input();
            input.timeout = Duration::from_secs(15);
            let mut session = Session::load_with_control(&input, &control).unwrap();
            let address = Address::repeat_byte(0x22);
            let before = session.world().account(address).unwrap().clone();
            let cancellation = control.cancellation().clone();
            scope.spawn(move || {
                received.recv_timeout(Duration::from_secs(5)).unwrap();
                cancellation.cancel();
            });
            let start = Instant::now();
            let error = session
                .fetch_storage(address, &BTreeSet::from([U256::from(1), U256::from(2)]))
                .unwrap_err();
            assert!(matches!(error, RpcError::Cancelled { .. }), "{error}");
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "must not wait for the 15 second HTTP timeout"
            );
            assert!(
                disconnected.recv_timeout(Duration::from_secs(3)).unwrap(),
                "cancelled reactor must close the pending socket before session drop"
            );
            assert_eq!(session.world().account(address), Some(&before));
            let requests = server.requests();
            assert_eq!(requests.last().unwrap()["method"], "eth_getStorageAt");
            assert_eq!(requests.last().unwrap()["params"][1], "0x2");
            assert!(matches!(
                session.fetch_account(Address::repeat_byte(0x55)),
                Err(RpcError::Cancelled { .. })
            ));
            assert_eq!(server.requests().len(), requests.len());
        });
    }
}

#[test]
fn cancelled_analysis_never_starts_rpc_or_returns_an_incomplete_success() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(healthy(request)));
        let control = Control::new(Observer::default());
        control.cancellation().cancel();
        let address = Address::repeat_byte(0x22);
        let entry = Entry {
            address,
            environment: EvmEnvironment {
                to: address.into(),
                ..EvmEnvironment::default()
            },
        };
        let error = analysis::analyze_rpc_with_control(
            &server.input(),
            entry,
            ExecutionConfig::default(),
            &control,
        )
        .unwrap_err();
        assert!(matches!(error, analysis::RpcAnalysisError::Cancelled(_)));
        assert!(server.requests().is_empty());
    });
}

#[test]
fn cancellation_during_storage_discovery_returns_cancelled_instead_of_a_frontier() {
    thread::scope(|scope| {
        let (started, received) = mpsc::channel();
        let (closed, disconnected) = mpsc::channel();
        let server = Server::new(scope, move |request| {
            if request["method"] == "eth_getStorageAt" && request["params"][1] == "0x2" {
                return Reply::Hold {
                    started: started.clone(),
                    closed: closed.clone(),
                    body: false,
                };
            }
            let mut reply = healthy(request);
            if request["method"] == "eth_getCode" {
                reply["result"] = serde_json::json!("0x60025400");
            }
            Reply::Json(reply)
        });
        let control = Control::new(Observer::default());
        let cancellation = control.cancellation().clone();
        scope.spawn(move || {
            received.recv_timeout(Duration::from_secs(5)).unwrap();
            cancellation.cancel();
        });
        let address = Address::repeat_byte(0x22);
        let entry = Entry {
            address,
            environment: EvmEnvironment {
                to: address.into(),
                ..EvmEnvironment::default()
            },
        };
        let error = analysis::analyze_rpc_with_control(
            &server.input(),
            entry,
            ExecutionConfig::default(),
            &control,
        )
        .unwrap_err();
        assert!(matches!(error, analysis::RpcAnalysisError::Cancelled(_)));
        assert!(disconnected.recv_timeout(Duration::from_secs(3)).unwrap());
        let requests = server.requests();
        assert_eq!(requests.last().unwrap()["params"][1], "0x2");
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["method"] == "eth_getCode")
                .count(),
            1
        );
    });
}
