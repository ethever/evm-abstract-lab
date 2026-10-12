use super::validate;
use crate::{
    Fork, U256,
    world::rpc::{ResponseReason, RpcContext, RpcError, RpcInput, context},
};
fn scope(method: &'static str) -> RpcContext {
    context(
        &RpcInput::new("http://localhost", Fork::Osaka),
        method,
        None,
        None,
    )
}
#[test]
fn sdk_boundary_rejects_duplicate_response_members_even_when_null() {
    for input in [
        r#"{"jsonrpc":"2.0","id":1,"result":null,"result":"0x1"}"#,
        r#"{"jsonrpc":"2.0","id":1,"error":null,"error":{"code":1,"message":"bad"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"id":2,"result":"0x1"}"#,
        r#"{"jsonrpc":"2.0","jsonrpc":"2.0","id":1,"result":"0x1"}"#,
    ] {
        assert!(
            validate(input.as_bytes(), 1, &scope("eth_chainId")).is_err(),
            "{input}"
        );
    }
}
#[test]
fn canonical_quantity_policy_supplements_sdk_integer_decoding() {
    for value in ["0x", "0x00", "1", "0X1", "0xg", "0x-1"] {
        let reply = format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{value}"}}"#);
        assert!(validate(reply.as_bytes(), 1, &scope("eth_chainId")).is_err());
    }
    let reply = r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#;
    assert!(validate(reply.as_bytes(), 1, &scope("eth_chainId")).is_ok());
    let largest = format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{:#x}"}}"#, U256::MAX);
    assert!(validate(largest.as_bytes(), 1, &scope("eth_chainId")).is_ok());
    let oversized = format!(
        r#"{{"jsonrpc":"2.0","id":1,"result":"0x1{}"}}"#,
        "0".repeat(64)
    );
    assert!(validate(oversized.as_bytes(), 1, &scope("eth_chainId")).is_err());
}

#[test]
fn null_identity_members_keep_their_specific_protocol_failures() {
    let version = br#"{"jsonrpc":null,"id":1,"result":"0x1"}"#;
    assert!(matches!(
        validate(version, 1, &scope("eth_chainId")),
        Err(RpcError::Response {
            reason: ResponseReason::Version,
            ..
        })
    ));
    let id = br#"{"jsonrpc":"2.0","id":null,"result":"0x1"}"#;
    assert!(matches!(
        validate(id, 1, &scope("eth_chainId")),
        Err(RpcError::Response {
            reason: ResponseReason::Id {
                expected: 1,
                observed: None
            },
            ..
        })
    ));
}

#[test]
fn canonical_data_policy_allows_empty_bytes_and_retains_leading_zeros() {
    for value in ["0x", "0x00", "0x0001"] {
        let reply = format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{value}"}}"#);
        assert!(validate(reply.as_bytes(), 1, &scope("eth_getCode")).is_ok());
    }
    let reply = br#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#;
    assert!(validate(reply, 1, &scope("eth_getCode")).is_err());
}
