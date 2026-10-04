//! 离线输入边界：JSON 中缺少的事实保持未知；解析不补查 RPC 或节点状态。

#[cfg(test)]
mod tests;

use alloy_primitives::{Address, B256, U256, hex};
use evm_abstract::{
    Fork,
    bytecode::DecodeError,
    domain::Value,
    fork::ParseForkError,
    world::{Account, Existence, World, WorldError},
};
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path};
use thiserror::Error;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    fork: String,
    provenance: String,
    identity: Option<Identity>,
    fingerprint: Option<String>,
    accounts: Vec<SnapshotAccount>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Identity {
    Offline {
        label: String,
    },
    Chain {
        chain_id: String,
        block_hash: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotAccount {
    address: String,
    code: Option<String>,
    #[serde(default, deserialize_with = "deserialize_storage")]
    storage: BTreeMap<String, String>,
    #[serde(default = "unknown_storage")]
    storage_unknown: bool,
    balance: Option<String>,
    nonce: Option<String>,
    existence: Option<String>,
    code_hash: Option<String>,
}

fn unknown_storage() -> bool {
    true
}

fn deserialize_storage<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Slots;
    impl<'de> serde::de::Visitor<'de> for Slots {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a storage object with unique slot keys")
        }
        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: serde::de::MapAccess<'de>,
        {
            let mut slots = BTreeMap::new();
            while let Some((slot, value)) = map.next_entry::<String, String>()? {
                if slots.insert(slot.clone(), value).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate storage key {slot}"
                    )));
                }
            }
            Ok(slots)
        }
    }
    deserializer.deserialize_map(Slots)
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
    #[error("invalid 32-byte {field} hash: {input:?}")]
    Hash { field: &'static str, input: String },
    #[error(
        "invalid account existence {input:?} at {address}; expected unknown, present or absent"
    )]
    Existence { address: Address, input: String },
    #[error("anchored snapshot code at {0} requires code_hash")]
    MissingCodeHash(Address),
    #[error("snapshot fingerprint mismatch: expected {expected}, observed {observed}")]
    Fingerprint { expected: B256, observed: B256 },
    #[error(transparent)]
    World(#[from] WorldError),
}

pub(crate) fn hash(input: &str, field: &'static str) -> Result<B256, InputError> {
    input
        .strip_prefix("0x")
        .filter(|digits| digits.len() == 64)
        .and_then(|digits| hex::decode(digits).ok())
        .and_then(|bytes| B256::try_from(bytes.as_slice()).ok())
        .ok_or_else(|| InputError::Hash {
            field,
            input: input.to_owned(),
        })
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
    let anchored = matches!(snapshot.identity, Some(Identity::Chain { .. }));
    let mut world = match snapshot.identity {
        None => World::new(fork, snapshot.provenance),
        Some(Identity::Offline { label }) => {
            // The old provenance field remains a description; identity is explicit.
            World::offline(fork, label, snapshot.provenance)
        }
        Some(Identity::Chain {
            chain_id,
            block_hash,
        }) => World::anchored(
            fork,
            word(&chain_id, "chain id")?,
            hash(&block_hash, "block")?,
            snapshot.provenance,
        ),
    };
    for input in snapshot.accounts {
        let address = address(&input.address, "account")?;
        if world.account(address).is_some() {
            return Err(InputError::Duplicate(address));
        }
        let has_code = input.code.is_some();
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
        account.nonce = input
            .nonce
            .map(|value| word(&value, "nonce").map(Value::constant))
            .transpose()?
            .unwrap_or_else(Value::top);
        account.existence = match input.existence.as_deref() {
            None | Some("unknown") => Existence::Unknown,
            Some("present") => Existence::Present,
            Some("absent") => Existence::Absent,
            Some(input) => {
                return Err(InputError::Existence {
                    address,
                    input: input.to_owned(),
                });
            }
        };
        match input.code_hash {
            Some(declared) => {
                world.insert_with_code_hash(address, account, hash(&declared, "code")?)?;
            }
            None if anchored && has_code => return Err(InputError::MissingCodeHash(address)),
            None => {
                world.insert(address, account)?;
            }
        }
    }
    if let Some(expected) = snapshot.fingerprint {
        let expected = hash(&expected, "fingerprint")?;
        let observed = world.fingerprint();
        if expected != observed {
            return Err(InputError::Fingerprint { expected, observed });
        }
    }
    Ok(world)
}
