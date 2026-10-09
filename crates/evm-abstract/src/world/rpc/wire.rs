//! Concrete JSON-RPC messages. Unknown block-header extension fields are
//! consumed without retaining an untyped JSON tree; known fields stay strict.

#[cfg(test)]
mod tests;

use alloy_primitives::{Address, B256, U256, hex};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{MapAccess, Visitor},
};
use std::{fmt, marker::PhantomData};

#[derive(Clone, Copy, Debug, Serialize)]
pub(super) enum Method {
    #[serde(rename = "eth_chainId")]
    ChainId,
    #[serde(rename = "eth_getBlockByNumber")]
    BlockByNumber,
    #[serde(rename = "eth_getBlockByHash")]
    BlockByHash,
    #[serde(rename = "eth_getCode")]
    Code,
    #[serde(rename = "eth_getBalance")]
    Balance,
    #[serde(rename = "eth_getTransactionCount")]
    Nonce,
    #[serde(rename = "eth_getStorageAt")]
    Storage,
    #[serde(rename = "eth_feeHistory")]
    FeeHistory,
}

impl Method {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::ChainId => "eth_chainId",
            Self::BlockByNumber => "eth_getBlockByNumber",
            Self::BlockByHash => "eth_getBlockByHash",
            Self::Code => "eth_getCode",
            Self::Balance => "eth_getBalance",
            Self::Nonce => "eth_getTransactionCount",
            Self::Storage => "eth_getStorageAt",
            Self::FeeHistory => "eth_feeHistory",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Quantity(pub U256);

impl Serialize for Quantity {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{:#x}", self.0))
    }
}

impl<'de> Deserialize<'de> for Quantity {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let input = String::deserialize(deserializer)?;
        let digits = input
            .strip_prefix("0x")
            .filter(|digits| !digits.is_empty() && digits.len() <= 64)
            .ok_or_else(|| {
                serde::de::Error::custom("quantity requires 0x and 1..=64 hex digits")
            })?;
        if !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
            || (digits.len() > 1 && digits.starts_with('0'))
        {
            return Err(serde::de::Error::custom(
                "noncanonical hexadecimal quantity",
            ));
        }
        U256::from_str_radix(digits, 16)
            .map(Self)
            .map_err(|_| serde::de::Error::custom("invalid 256-bit quantity"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Data(pub Vec<u8>);

impl<'de> Deserialize<'de> for Data {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let input = String::deserialize(deserializer)?;
        let digits = input
            .strip_prefix("0x")
            .ok_or_else(|| serde::de::Error::custom("byte data requires 0x prefix"))?;
        hex::decode(digits)
            .map(Self)
            .map_err(|_| serde::de::Error::custom("invalid hexadecimal byte data"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Hash(pub B256);
impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Data::deserialize(deserializer)?;
        B256::try_from(value.0.as_slice())
            .map(Self)
            .map_err(|_| serde::de::Error::custom("hash must be exactly 32 bytes"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AccountAddress(pub Address);
impl<'de> Deserialize<'de> for AccountAddress {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Data::deserialize(deserializer)?;
        if value.0.len() != 20 {
            return Err(serde::de::Error::custom("address must be exactly 20 bytes"));
        }
        Ok(Self(Address::from_slice(&value.0)))
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(super) struct Selector {
    #[serde(rename = "blockHash")]
    pub block_hash: B256,
    #[serde(rename = "requireCanonical")]
    pub require_canonical: bool,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum Params {
    Empty([(); 0]),
    Latest(&'static str, bool),
    Number(Quantity, bool),
    Hash(B256, bool),
    Account(Address, Selector),
    Storage(Address, Quantity, Selector),
    FeeHistory(Quantity, Quantity, [(); 0]),
}

#[derive(Serialize)]
pub(super) struct Request<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: Method,
    pub params: &'a Params,
}

// Derive-generated Deserialize error paths erase `Expected` behind dyn. These
// concrete visitors use custom errors, preserving this crate's no_dyn boundary.
macro_rules! record {
    ($name:ident { $($field:ident: $ty:ty => $wire:literal),* $(,)? } optional { $($optional:ident: $optional_ty:ty => $optional_wire:literal),* $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub(super) struct $name { $(pub $field: $ty,)* $(pub $optional: Option<$optional_ty>,)* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct RecordVisitor;
                impl<'de> Visitor<'de> for RecordVisitor {
                    type Value = $name;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(stringify!($name)) }
                    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                        $(let mut $field: Option<$ty> = None;)*
                        $(let mut $optional: Option<Option<$optional_ty>> = None;)*
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $($wire => read_field(&mut map, &mut $field, $wire)?,)*
                                $($optional_wire => read_field(&mut map, &mut $optional, $optional_wire)?,)*
                                _ => { let _ = map.next_value::<serde::de::IgnoredAny>()?; }
                            }
                        }
                        Ok($name {
                            $($field: $field.ok_or_else(|| serde::de::Error::custom(concat!("missing field ", $wire)))?,)*
                            $($optional: $optional.unwrap_or(None),)*
                        })
                    }
                }
                deserializer.deserialize_map(RecordVisitor)
            }
        }
    };
}

record!(Header {
    hash: Hash => "hash",
    parent_hash: Hash => "parentHash",
    number: Quantity => "number",
    timestamp: Quantity => "timestamp",
    miner: AccountAddress => "miner",
    mix_hash: Hash => "mixHash",
    gas_limit: Quantity => "gasLimit",
} optional {
    base_fee: Quantity => "baseFeePerGas",
    excess_blob_gas: Quantity => "excessBlobGas",
    blob_gas_used: Quantity => "blobGasUsed",
});

record!(FeeHistory {
    oldest_block: Quantity => "oldestBlock",
    base_fee_per_blob_gas: Vec<Quantity> => "baseFeePerBlobGas",
} optional {});

record!(RemoteError {
    code: i64 => "code",
    message: String => "message",
} optional {});

fn read_field<'de, M: MapAccess<'de>, T: Deserialize<'de>>(
    map: &mut M,
    field: &mut Option<T>,
    name: &str,
) -> Result<(), M::Error> {
    if field.is_some() {
        return Err(serde::de::Error::custom(format!("duplicate field {name}")));
    }
    *field = Some(map.next_value()?);
    Ok(())
}

pub(super) struct Reply<T> {
    pub version: Option<String>,
    pub id: Option<u64>,
    pub result: Option<Option<T>>,
    pub error: Option<Option<RemoteError>>,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Reply<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReplyVisitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for ReplyVisitor<T> {
            type Value = Reply<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON-RPC reply")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let (mut version, mut id, mut result, mut error) = (None, None, None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "jsonrpc" => read_field(&mut map, &mut version, "jsonrpc")?,
                        "id" => read_field(&mut map, &mut id, "id")?,
                        "result" => read_field(&mut map, &mut result, "result")?,
                        "error" => read_field(&mut map, &mut error, "error")?,
                        _ => {
                            let _ = map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(Reply {
                    version,
                    id,
                    result,
                    error,
                })
            }
        }
        deserializer.deserialize_map(ReplyVisitor(PhantomData))
    }
}
