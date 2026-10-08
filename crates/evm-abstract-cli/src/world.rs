//! 离线输入边界：JSON 中缺少的事实保持未知；解析不补查 RPC 或节点状态。

mod input;

#[cfg(test)]
mod tests;

use alloy_primitives::{Address, B256, U256, hex};
use evm_abstract::{
    Fork,
    bytecode::DecodeError,
    domain::AbstractValue,
    fork::ParseForkError,
    world::{Account, Existence, World, WorldError},
};
use std::{fs, path::Path};
use thiserror::Error;

use input::{Identity, Snapshot};

#[derive(Debug, Error)]
pub(crate) enum InputError {
    #[error("cannot read world snapshot {path}: {error}")]
    Read { path: String, error: std::io::Error },
    #[error("invalid world JSON: {0}")]
    Json(serde_json::Error),
    #[error("{0}")]
    Fork(ParseForkError),
    #[error("invalid {field} address {input:?}: {error}")]
    Address {
        field: &'static str,
        input: String,
        error: hex::FromHexError,
    },
    #[error("{field} must be a nonempty 0x-prefixed hexadecimal word: {input:?}")]
    WordSyntax { field: &'static str, input: String },
    #[error("{field} exceeds 256 bits: {input:?}")]
    WordWidth { field: &'static str, input: String },
    #[error("invalid {field} hex: {error}")]
    Hex {
        field: &'static str,
        error: hex::FromHexError,
    },
    #[error("invalid account code at {address}: {error}")]
    Code {
        address: Address,
        error: DecodeError,
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
    #[error("{0}")]
    World(WorldError),
}

impl From<serde_json::Error> for InputError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<ParseForkError> for InputError {
    fn from(error: ParseForkError) -> Self {
        Self::Fork(error)
    }
}

impl From<WorldError> for InputError {
    fn from(error: WorldError) -> Self {
        Self::World(error)
    }
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
    input.parse().map_err(|error| InputError::Address {
        field,
        input: input.to_owned(),
        error,
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
    let bytes = hex::decode(even).map_err(|error| InputError::Hex { field, error })?;
    Ok(U256::from_be_slice(&bytes))
}

pub(crate) fn calldata(input: &str) -> Result<Vec<u8>, InputError> {
    hex::decode(input.strip_prefix("0x").unwrap_or(input)).map_err(|error| InputError::Hex {
        field: "calldata",
        error,
    })
}

pub(crate) fn load(path: &Path) -> Result<World, InputError> {
    let text = fs::read_to_string(path).map_err(|error| InputError::Read {
        path: path.display().to_string(),
        error,
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
                .map_err(|error| InputError::Code { address, error })?,
            None => Account::unknown(),
        };
        account.storage_unknown = input.storage_unknown;
        for (slot, value) in input.storage {
            let slot = word(&slot, "storage slot")?;
            let value = AbstractValue::constant(word(&value, "storage value")?);
            if account.storage.insert(slot, value).is_some() {
                return Err(InputError::DuplicateSlot { address, slot });
            }
        }
        account.balance = input
            .balance
            .map(|value| word(&value, "balance").map(AbstractValue::constant))
            .transpose()?
            .unwrap_or_else(AbstractValue::top);
        account.nonce = input
            .nonce
            .map(|value| word(&value, "nonce").map(AbstractValue::constant))
            .transpose()?
            .unwrap_or_else(AbstractValue::top);
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
