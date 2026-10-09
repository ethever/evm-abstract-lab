//! Caller-selected budgets have no independent fixed acquisition ceilings.

use super::{
    AccountRequest, Address, BTreeSet, Fork, Reply, RpcInput, Server, Session, U256, healthy,
};
use crate::world::rpc::{ConfigurationReason, RpcError, configured_loader};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

#[test]
fn explicit_limits_have_no_independent_account_slot_or_response_ceiling() {
    let mut input = RpcInput::new("http://127.0.0.1:1", Fork::Osaka);
    input.max_response_bytes = 65 * 1024 * 1024;
    input.max_accounts = 4097;
    input.accounts = (0_u64..4097)
        .map(|index| {
            let mut address = [0; 20];
            address[12..].copy_from_slice(&index.to_be_bytes());
            AccountRequest {
                address: Address::from(address),
                slots: BTreeSet::new(),
            }
        })
        .collect();
    input.accounts[0].slots = (0_u64..1025).map(U256::from).collect();
    configured_loader(&input).unwrap();
    input.accounts.push(input.accounts[0].clone());
    input.max_accounts += 1;
    assert!(matches!(
        configured_loader(&input),
        Err(RpcError::Configuration {
            reason: ConfigurationReason::InitialAccounts,
            ..
        })
    ));
}

#[test]
fn increased_budgets_accept_large_responses_and_discover_accounts_and_slots() {
    thread::scope(|scope| {
        let first = AtomicBool::new(true);
        let server = Server::new(scope, move |request| {
            let mut reply = healthy(request);
            if first.swap(false, Ordering::Relaxed) {
                reply["padding"] = serde_json::json!("x".repeat(4 * 1024 * 1024));
            }
            Reply::Json(reply)
        });
        let mut input = server.input();
        input.timeout = Duration::from_secs(30);
        input.max_response_bytes = 8 * 1024 * 1024;
        input.max_accounts = 4097;
        input.max_requests = 32;
        let mut session = Session::load(&input).unwrap();
        let address = Address::repeat_byte(0x55);
        assert!(session.fetch_account(address).unwrap());
        let slots = BTreeSet::from([U256::from(2)]);
        assert_eq!(
            session.fetch_storage(address, &slots).unwrap(),
            vec![U256::from(2)]
        );
        assert_eq!(session.world().accounts().len(), 2);
        assert_eq!(session.requests(), server.requests().len());
        assert!(session.requests() > 10);
    });
}

#[test]
fn initial_slot_acquisition_can_exceed_the_former_1024_slot_ceiling() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(healthy(request)));
        let mut input = server.input();
        input.accounts[0].slots = (0_u64..1025).map(U256::from).collect();
        input.max_requests = 1100;
        let session = Session::load(&input).unwrap();
        let account = session.world().account(Address::repeat_byte(0x22)).unwrap();
        assert_eq!(account.storage.len(), 1025);
        assert_eq!(session.requests(), server.requests().len());
        assert!(session.requests() > 1025);
    });
}

#[test]
fn acquisition_limits_must_still_be_positive() {
    for field in 0..4 {
        let mut input = RpcInput::new("http://127.0.0.1:1", Fork::Osaka);
        match field {
            0 => input.timeout = Duration::ZERO,
            1 => input.max_response_bytes = 0,
            2 => input.max_accounts = 0,
            _ => input.max_requests = 0,
        }
        assert!(matches!(
            configured_loader(&input),
            Err(RpcError::Configuration {
                reason: ConfigurationReason::Limits,
                ..
            })
        ));
    }
}
