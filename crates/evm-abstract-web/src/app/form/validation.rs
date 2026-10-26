//! Draft syntax checks only. The server remains authoritative for execution,
//! host representability, bytecode decoding and admission policy.

use std::collections::BTreeSet;

use evm_abstract_protocol::{AddressSetting, BlockSelector, CalldataInput, WordInput};

use super::{AnalysisForm, Source};

#[derive(Debug, thiserror::Error)]
#[error("{field}: {expected}")]
pub(super) struct InputError {
    field: &'static str,
    expected: &'static str,
}

impl AnalysisForm {
    pub(super) fn validation_error(&self) -> Option<InputError> {
        self.validate_draft().err()
    }

    fn validate_draft(&self) -> Result<(), InputError> {
        match self.source {
            Source::Bytecode => {
                let compact: String = self
                    .bytecode
                    .bytecode
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                check(
                    raw_bytes(compact.strip_prefix("0x").unwrap_or(&compact)),
                    "Runtime bytecode",
                    "enter pairs of hexadecimal digits; whitespace and 0x are accepted",
                )?;
                if let Some(value) = &self.bytecode.address {
                    check(
                        fixed_hex(value, 20),
                        "Root address",
                        "enter a 20-byte hexadecimal address",
                    )?;
                }
            }
            Source::Rpc => {
                check(
                    fixed_hex(&self.rpc.address, 20),
                    "Root account",
                    "enter a 20-byte hexadecimal address",
                )?;
                if let BlockSelector::Hash(hash) = &self.rpc.block {
                    check(
                        fixed_hex(hash, 32),
                        "Snapshot block",
                        "enter a 32-byte hexadecimal hash",
                    )?;
                }
                for account in &self.rpc.accounts {
                    check(
                        fixed_hex(&account.address, 20),
                        "Initial account",
                        "enter a 20-byte hexadecimal address",
                    )?;
                    for slot in &account.slots {
                        check(
                            word(slot).is_some(),
                            "Storage slot",
                            "enter an unsigned 256-bit word",
                        )?;
                    }
                }
            }
        }
        let env = &self.environment;
        for (field, setting) in [
            ("Caller", Some(&env.caller)),
            ("Origin", env.origin.as_ref()),
        ] {
            if let Some(AddressSetting::Concrete(value)) = setting {
                check(
                    fixed_hex(value, 20),
                    field,
                    "enter a 20-byte hexadecimal address or choose Unknown",
                )?;
            }
        }
        for (field, setting) in [
            ("Call value", &env.call_value),
            ("Gas price", &env.gas_price),
            ("Blob count", &env.blob_count),
        ] {
            if let WordInput::Concrete(value) = setting {
                check(
                    word(value).is_some(),
                    field,
                    "enter an unsigned 256-bit word or choose Unknown",
                )?;
            }
        }
        if let CalldataInput::Exact(value) = &env.calldata {
            check(
                bytes(value),
                "Calldata",
                "enter hexadecimal bytes; exact empty text means no calldata",
            )?;
        }
        if let Some(value) = &env.coinbase {
            check(
                fixed_hex(value, 20),
                "Beneficiary",
                "enter a 20-byte hexadecimal address",
            )?;
        }
        for (field, value) in [
            ("Initial gas upper bound", &env.gas_upper_bound),
            ("Block number override", &env.number),
            ("Timestamp", &env.timestamp),
            ("PREVRANDAO", &env.prevrandao),
            ("Block gas limit", &env.gas_limit),
            ("Execution chain ID", &env.chain_id),
            ("Base fee", &env.base_fee),
            ("Blob base fee", &env.blob_base_fee),
        ] {
            if let Some(value) = value {
                check(
                    word(value).is_some(),
                    field,
                    "enter an unsigned 256-bit word or disable the override",
                )?;
            }
        }
        for (field, values) in [
            ("Block hashes", &env.block_hashes),
            ("Blob hashes", &env.blob_hashes),
        ] {
            let mut seen = BTreeSet::new();
            for value in values {
                let index = word(&value.index).ok_or(InputError {
                    field,
                    expected: "enter an unsigned 256-bit index",
                })?;
                check(
                    fixed_hex(&value.hash, 32),
                    field,
                    "enter a 32-byte hexadecimal hash",
                )?;
                check(
                    seen.insert(index),
                    field,
                    "each observed index must be unique",
                )?;
            }
        }
        Ok(())
    }
}

fn check(valid: bool, field: &'static str, expected: &'static str) -> Result<(), InputError> {
    if valid {
        Ok(())
    } else {
        Err(InputError { field, expected })
    }
}

fn digits(value: &str) -> &str {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value)
}

fn bytes(value: &str) -> bool {
    raw_bytes(digits(value))
}

fn raw_bytes(value: &str) -> bool {
    value.len().is_multiple_of(2) && value.bytes().all(|c| c.is_ascii_hexdigit())
}

fn fixed_hex(value: &str, len: usize) -> bool {
    let value = digits(value);
    value.len() == len * 2 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

/// Lossless scalar syntax, including the radix prefixes and separators accepted
/// by the server's U256 parser. Empty exact words require an explicit value.
fn word(value: &str) -> Option<[u64; 4]> {
    if value.is_empty() {
        return None;
    }
    let (value, radix) = match value.get(..2) {
        Some("0x" | "0X") => (&value[2..], 16_u64),
        Some("0o" | "0O") => (&value[2..], 8),
        Some("0b" | "0B") => (&value[2..], 2),
        _ => (value, 10),
    };
    let mut result = [0_u64; 4];
    let mut count = 0;
    for character in value.chars().filter(|c| *c != '_') {
        let mut carry = u128::from(character.to_digit(radix as u32)?);
        for limb in &mut result {
            let next = u128::from(*limb) * u128::from(radix) + carry;
            *limb = next as u64;
            carry = next >> 64;
        }
        if carry != 0 {
            return None;
        }
        count += 1;
    }
    (count > 0).then_some(result)
}
