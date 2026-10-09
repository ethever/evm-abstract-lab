use super::{Data, Header, Method, Params, Quantity, Reply, Request, Selector};
use alloy_primitives::{B256, U256};

#[test]
fn typed_envelopes_reject_duplicate_fields_even_when_first_value_is_null() {
    for input in [
        r#"{"jsonrpc":"2.0","id":1,"result":null,"result":"0x1"}"#,
        r#"{"jsonrpc":"2.0","id":1,"error":null,"error":{"code":1,"message":"bad"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"id":2,"result":"0x1"}"#,
        r#"{"jsonrpc":"2.0","jsonrpc":"2.0","id":1,"result":"0x1"}"#,
    ] {
        assert!(
            serde_json::from_str::<Reply<Quantity>>(input).is_err(),
            "{input}"
        );
    }
    let reply: Reply<Quantity> =
        serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"result":null}"#).unwrap();
    assert_eq!(reply.result, Some(None));
    assert!(reply.error.is_none());
}

#[test]
fn quantities_and_data_keep_distinct_canonical_rules() {
    for input in ["0x", "0x00", "1", "0X1", "0xg", "0x-1"] {
        assert!(serde_json::from_str::<Quantity>(&format!("\"{input}\"")).is_err());
    }
    let largest = Quantity(U256::MAX);
    let encoded = serde_json::to_string(&largest).unwrap();
    assert_eq!(serde_json::from_str::<Quantity>(&encoded).unwrap(), largest);
    assert_eq!(
        serde_json::from_str::<Data>(r#""0x0001""#).unwrap().0,
        [0, 1]
    );
    assert!(serde_json::from_str::<Data>(r#""0x1""#).is_err());
}

#[test]
fn state_request_uses_typed_hash_selector_and_header_extensions_are_ignored() {
    let params = Params::Account(
        Default::default(),
        Selector {
            block_hash: B256::repeat_byte(0x11),
            require_canonical: true,
        },
    );
    let request = Request {
        jsonrpc: "2.0",
        id: 7,
        method: Method::Code,
        params: &params,
    };
    let text = serde_json::to_string(&request).unwrap();
    assert!(text.contains(r#""method":"eth_getCode""#));
    assert!(text.contains(r#""requireCanonical":true"#));
    let header = format!(
        r#"{{"hash":"{0}","parentHash":"{0}","number":"0x1","timestamp":"0x2","miner":"0x0000000000000000000000000000000000000000","mixHash":"{0}","gasLimit":"0x3","extension":{{"any":[null,true,42]}}}}"#,
        B256::ZERO
    );
    let parsed: Header = serde_json::from_str(&header).unwrap();
    assert_eq!(parsed.number.0, U256::from(1));
    assert!(parsed.base_fee.is_none());
    let duplicate = header.replace(
        r#""extension""#,
        r#""baseFeePerGas":null,"baseFeePerGas":"0x1","extension""#,
    );
    assert!(serde_json::from_str::<Header>(&duplicate).is_err());
}
