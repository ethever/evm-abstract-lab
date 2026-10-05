//! CLI quantity syntax: nonnegative ASCII decimal or 0x/0X-prefixed hexadecimal.
//!
//! These numbers are validated by Clap before file or RPC acquisition. Snapshot
//! words and RPC quantities keep their separate hexadecimal input contracts.

#[cfg(test)]
mod tests;

use alloy_primitives::{U256, ruint::ParseError};
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum NumberError {
    #[error("expected nonempty ASCII decimal digits or 0x/0X-prefixed hexadecimal digits")]
    Syntax,
    #[error("integer exceeds the maximum unsigned 256-bit value (2^256 - 1)")]
    Overflow {
        #[source]
        source: ParseError,
    },
}

pub(crate) fn parse(input: &str) -> Result<U256, NumberError> {
    let (digits, radix) = match input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
    {
        Some(digits) => (digits, 16),
        None => (input, 10),
    };
    // The library also accepts empty strings and separators; CLI syntax does not.
    let valid = !digits.is_empty()
        && if radix == 16 {
            digits.bytes().all(|digit| digit.is_ascii_hexdigit())
        } else {
            digits.bytes().all(|digit| digit.is_ascii_digit())
        };
    if !valid {
        return Err(NumberError::Syntax);
    }
    // After syntax validation, only numeric overflow can fail for these radices.
    // Leading zeros do not reduce the representable range or imply octal input.
    U256::from_str_radix(digits, radix).map_err(|source| NumberError::Overflow { source })
}
