//! 离线输入边界：JSON 中缺少的事实保持未知；解析不补查 RPC 或节点状态。

#[cfg(test)]
mod tests;

use alloy_primitives::{Address, U256, hex};
use evm_abstract::{
    Fork,
    bytecode::DecodeError,
    domain::Value,
    fork::ParseForkError,
    world::{Account, World, WorldError},
};
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path};
use thiserror::Error;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    fork: String,
    provenance: String,
    accounts: Vec<SnapshotAccount>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotAccount {
    address: String,
    code: Option<String>,
    #[serde(default)]
    storage: BTreeMap<String, String>,
    #[serde(default = "unknown_storage")]
    storage_unknown: bool,
    balance: Option<String>,
}

fn unknown_storage() -> bool {
    true
}

#[derive(Debug, Error)]
pub(crate) enum InputError {
    #[error("cannot read world snapshot {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid world JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Fork(#[from] ParseForkError),
    #[error("invalid {field} address {input:?}: {source}")]
    Address {
        field: &'static str,
        input: String,
        #[source]
        source: hex::FromHexError,
    },
    #[error("{field} must be a nonempty 0x-prefixed hexadecimal word: {input:?}")]
    WordSyntax { field: &'static str, input: String },
    #[error("{field} exceeds 256 bits: {input:?}")]
    WordWidth { field: &'static str, input: String },
    #[error("invalid {field} hex: {source}")]
    Hex {
        field: &'static str,
        #[source]
        source: hex::FromHexError,
    },
    #[error("invalid account code at {address}: {source}")]
    Code {
        address: Address,
        #[source]
        source: DecodeError,
    },
    #[error("duplicate account address {0}")]
    Duplicate(Address),
    #[error("duplicate storage slot {slot:#x} at account {address}")]
    DuplicateSlot { address: Address, slot: U256 },
    #[error(transparent)]
    World(#[from] WorldError),
}

pub(crate) fn address(input: &str, field: &'static str) -> Result<Address, InputError> {
    input.parse().map_err(|source| InputError::Address {
        field,
        input: input.to_owned(),
        source,
    })
}

pub(crate) fn word(input: &str, field: &'static str) -> Result<U256, InputError> {
    let digits = input
        .strip_prefix("0x")
        .filter(|digits| !digits.is_empty())
        .ok_or_else(|| InputError::WordSyntax {
            field,
            input: input.to_owned(),
        })?;
    if digits.len() > 64 {
        return Err(InputError::WordWidth {
            field,
            input: input.to_owned(),
        });
    }
    // JSON words are numbers, so an odd digit count has a leading zero nibble.
    let padded;
    let even = if digits.len() % 2 == 1 {
        padded = format!("0{digits}");
        padded.as_str()
    } else {
        digits
    };
    let bytes = hex::decode(even).map_err(|source| InputError::Hex { field, source })?;
    Ok(U256::from_be_slice(&bytes))
}

pub(crate) fn calldata(input: &str) -> Result<Vec<u8>, InputError> {
    hex::decode(input.strip_prefix("0x").unwrap_or(input)).map_err(|source| InputError::Hex {
        field: "calldata",
        source,
    })
}

pub(crate) fn load(path: &Path) -> Result<World, InputError> {
    let text = fs::read_to_string(path).map_err(|source| InputError::Read {
        path: path.display().to_string(),
        source,
    })?;
    parse(&text)
}

fn parse(text: &str) -> Result<World, InputError> {
    let snapshot: Snapshot = serde_json::from_str(text)?;
    let fork: Fork = snapshot.fork.parse()?;
    let mut world = World::new(fork, snapshot.provenance);
    for input in snapshot.accounts {
        let address = address(&input.address, "account")?;
        if world.account(address).is_some() {
            return Err(InputError::Duplicate(address));
        }
        let mut account = match input.code {
            Some(code) => Account::from_hex(&code, fork)
                .map_err(|source| InputError::Code { address, source })?,
            None => Account::unknown(),
        };
        account.storage_unknown = input.storage_unknown;
        for (slot, value) in input.storage {
            let slot = word(&slot, "storage slot")?;
            let value = Value::constant(word(&value, "storage value")?);
            if account.storage.insert(slot, value).is_some() {
                return Err(InputError::DuplicateSlot { address, slot });
            }
        }
        account.balance = input
            .balance
            .map(|value| word(&value, "balance").map(Value::constant))
            .transpose()?
            .unwrap_or_else(Value::top);
        world.insert(address, account)?;
    }
    Ok(world)
}
