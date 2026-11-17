//! A bounded code-only RPC adapter. SDK envelopes and Ethereum types retain
//! exact IDs, words and block selectors; endpoint URLs never enter errors.
use super::invalid;
use alloy::{
    primitives::Bytes,
    rpc::{
        json_rpc::{Id, Request, Response, ResponsePayload, RpcSend},
        types::{BlockId, BlockNumberOrTag, Header},
    },
};
use alloy_primitives::{Address, B256, U256};
use evm_abstract_protocol::{
    self as api, ApiError, ApiErrorCode, BlockSelector, DecompileAddressInput,
    DecompileAddressSource, ErrorDetails, RpcFailure, RpcFailureKind,
};
use serde::de::DeserializeOwned;
use std::{str::FromStr, time::Duration};

const MAX_RESPONSE: usize = 4 * 1024 * 1024;
struct Client {
    http: reqwest::Client,
    endpoint: String,
    next: u64,
    account: String,
    chain: Option<String>,
    block: Option<String>,
}
impl Client {
    fn failure(&self, method: &str, kind: RpcFailureKind, message: &str) -> ApiError {
        ApiError {
            code: ApiErrorCode::Rpc,
            message: message.into(),
            details: ErrorDetails::Rpc(Box::new(RpcFailure {
                kind,
                chain_id: self.chain.clone(),
                block_hash: self.block.clone(),
                method: method.into(),
                account: Some(self.account.clone()),
                slot: None,
                resource: None,
                limit: (kind == RpcFailureKind::ResponseLimit).then_some(MAX_RESPONSE),
                http_status: None,
                rpc_code: None,
                json_line: None,
                json_column: None,
                cause: None,
                message: message.into(),
            })),
        }
    }
    async fn call<T: DeserializeOwned, P: RpcSend>(
        &mut self,
        method: &'static str,
        params: P,
    ) -> Result<T, ApiError> {
        self.next += 1;
        let id = self.next;
        let request = Request::new(method, Id::Number(id), params);
        let mut response = self
            .http
            .post(&self.endpoint)
            .json(&request)
            .send()
            .await
            .map_err(|_| self.failure(method, RpcFailureKind::Transport, "RPC transport failed"))?;
        if !response.status().is_success() {
            let mut e = self.failure(
                method,
                RpcFailureKind::Http,
                "RPC returned a non-success HTTP status",
            );
            if let ErrorDetails::Rpc(r) = &mut e.details {
                r.http_status = Some(response.status().as_u16());
            }
            return Err(e);
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE as u64)
        {
            return Err(self.failure(
                method,
                RpcFailureKind::ResponseLimit,
                "RPC response exceeds the code-acquisition limit",
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| self.failure(method, RpcFailureKind::Read, "RPC response read failed"))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
                return Err(self.failure(
                    method,
                    RpcFailureKind::ResponseLimit,
                    "RPC response exceeds the code-acquisition limit",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice::<Version>(&bytes).map_err(|_| {
            self.failure(
                method,
                RpcFailureKind::Response,
                "RPC response must declare JSON-RPC 2.0",
            )
        })?;
        let response: Response<Option<T>> = serde_json::from_slice(&bytes).map_err(|e| {
            let mut error = self.failure(
                method,
                RpcFailureKind::Json,
                "RPC response has invalid JSON or typed fields",
            );
            if let ErrorDetails::Rpc(r) = &mut error.details {
                r.json_line = Some(e.line());
                r.json_column = Some(e.column());
            }
            error
        })?;
        if response.id != Id::Number(id) {
            return Err(self.failure(
                method,
                RpcFailureKind::Response,
                "RPC response ID does not match its request",
            ));
        }
        match response.payload {
            ResponsePayload::Success(Some(result)) => Ok(result),
            ResponsePayload::Success(None) => Err(self.failure(
                method,
                RpcFailureKind::MissingResult,
                "RPC returned a null result",
            )),
            ResponsePayload::Failure(error) => {
                let mut failure = self.failure(
                    method,
                    RpcFailureKind::Remote,
                    "RPC rejected the requested method or pinned state",
                );
                if let ErrorDetails::Rpc(r) = &mut failure.details {
                    r.rpc_code = Some(error.code);
                }
                Err(failure)
            }
        }
    }
}

pub(super) async fn resolve(
    input: DecompileAddressInput,
    providers: &crate::rpc_providers::Registry,
) -> Result<(Vec<u8>, DecompileAddressSource), ApiError> {
    let provider = input
        .provider_id
        .or_else(|| providers.default_id().map(str::to_owned))
        .ok_or_else(|| invalid("provider_id", "no RPC provider is configured"))?;
    let endpoint = providers.endpoint(&provider)?;
    let address = Address::from_str(&input.address)
        .map_err(|_| invalid("address", "expected a 20-byte hexadecimal address"))?;
    let expected = input
        .chain_id
        .as_deref()
        .map(|s| {
            if let Some(hex) = s.strip_prefix("0x") {
                U256::from_str_radix(hex, 16)
            } else {
                U256::from_str_radix(s, 10)
            }
        })
        .transpose()
        .map_err(|_| invalid("chain_id", "expected a decimal or hexadecimal chain ID"))?;
    let requested_hash = match &input.block {
        BlockSelector::Hash(hash) => Some(
            B256::from_str(hash).map_err(|_| invalid("block", "expected a 32-byte block hash"))?,
        ),
        _ => None,
    };
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| invalid("provider_id", "RPC client could not be initialized"))?;
    let mut client = Client {
        http,
        endpoint: endpoint.into(),
        next: 0,
        account: format!("{address:#x}"),
        chain: None,
        block: requested_hash.map(|h| format!("{h:#x}")),
    };
    let chain: U256 = client.call("eth_chainId", ()).await?;
    client.chain = Some(format!("{chain:#x}"));
    if let Some(expected) = expected.filter(|expected| *expected != chain) {
        client.chain = Some(format!("{expected:#x}"));
        let mut failure = client.failure(
            "eth_chainId",
            RpcFailureKind::ChainMismatch,
            "RPC chain does not match the address link's chain ID",
        );
        if let ErrorDetails::Rpc(r) = &mut failure.details {
            r.cause = Some(api::RpcFailureCause::ChainMismatch(format!("{chain:#x}")));
        }
        return Err(failure);
    }
    let header: Header = match &input.block {
        BlockSelector::Latest => {
            client
                .call("eth_getBlockByNumber", (BlockNumberOrTag::Latest, false))
                .await?
        }
        BlockSelector::Number(number) => {
            client
                .call(
                    "eth_getBlockByNumber",
                    (BlockNumberOrTag::Number(*number), false),
                )
                .await?
        }
        BlockSelector::Hash(_) => {
            client
                .call(
                    "eth_getBlockByHash",
                    (requested_hash.expect("validated hash"), false),
                )
                .await?
        }
    };
    if matches!(input.block,BlockSelector::Number(n) if n!=header.number)
        || requested_hash.is_some_and(|hash| hash != header.hash)
    {
        return Err(client.failure(
            "block",
            RpcFailureKind::BlockMismatch,
            "RPC header does not match the requested block",
        ));
    }
    client.block = Some(format!("{:#x}", header.hash));
    let code: Bytes = client
        .call(
            "eth_getCode",
            (address, BlockId::hash_canonical(header.hash)),
        )
        .await?;
    // A changing/load-balanced endpoint must not mislabel these code bytes.
    let after_chain: U256 = client.call("eth_chainId", ()).await?;
    if after_chain != chain {
        return Err(client.failure(
            "eth_chainId",
            RpcFailureKind::ChainMismatch,
            "RPC chain changed during code acquisition",
        ));
    }
    let after: Header = client
        .call(
            "eth_getBlockByNumber",
            (BlockNumberOrTag::Number(header.number), false),
        )
        .await?;
    if after.hash != header.hash || after.number != header.number {
        return Err(client.failure(
            "eth_getBlockByNumber",
            RpcFailureKind::BlockMismatch,
            "pinned block changed during code acquisition",
        ));
    }
    if code.is_empty() {
        let mut error = client.failure(
            "eth_getCode",
            RpcFailureKind::Code,
            "address has no runtime bytecode at the selected block",
        );
        error.code = ApiErrorCode::InvalidBytecode;
        return Err(error);
    }
    if code.len() > api::MAX_REQUEST_BYTES / 2 {
        let mut error = client.failure(
            "eth_getCode",
            RpcFailureKind::ResponseLimit,
            "runtime bytecode exceeds the decompiler request limit",
        );
        error.code = ApiErrorCode::RequestTooLarge;
        if let ErrorDetails::Rpc(r) = &mut error.details {
            r.limit = Some(api::MAX_REQUEST_BYTES / 2);
        }
        return Err(error);
    }
    Ok((
        code.to_vec(),
        DecompileAddressSource {
            provider_id: provider,
            address: format!("{address:#x}"),
            chain_id: chain.to_string(),
            block_hash: format!("{:#x}", header.hash),
            block_number: api::DecimalU64(header.number),
        },
    ))
}

struct Version;
impl<'de> serde::Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Version;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON-RPC 2.0 object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Version, M::Error> {
                let mut version = false;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "jsonrpc" {
                        if version || map.next_value::<String>()? != "2.0" {
                            return Err(serde::de::Error::custom("invalid JSON-RPC version"));
                        }
                        version = true;
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                if !version {
                    return Err(serde::de::Error::missing_field("jsonrpc"));
                }
                Ok(Version)
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}
