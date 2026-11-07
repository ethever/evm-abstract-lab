//! Operand roles belong to primitive opcodes, never their display spelling.
//! An incomplete signature must leave the entire invocation positional.

use super::{LiftedRenderError, MAX_LIFTED_EXPRESSION_TEXT};

pub(super) const LEFT_RIGHT: &[&str] = &["left", "right"];

/// EVM spellings and high-level constructs have separate naming policies.
/// Callers may still supply a qualified EVM name; qualification is idempotent.
#[derive(Clone, Copy)]
pub(super) enum CallName<'a> {
    Evm(&'a str),
    High(&'a str),
}

impl<'a> CallName<'a> {
    fn parts(self) -> (&'static str, &'a str) {
        match self {
            Self::Evm(name) => ("evm.", name.trim_start_matches("evm.")),
            Self::High(name) => ("", name),
        }
    }

    /// Expression call names are fixed small spellings, paid when emitted.
    pub(super) fn opening(self) -> String {
        let (prefix, name) = self.parts();
        format!("{prefix}{name}(")
    }

    pub(super) fn append(
        self,
        output: &mut String,
        reserve: &mut impl FnMut(usize) -> bool,
    ) -> Result<(), LiftedRenderError> {
        let supplied = match self {
            Self::Evm(name) | Self::High(name) => name,
        };
        // Qualification may scan a caller-supplied name before emitting it.
        if supplied.len() > MAX_LIFTED_EXPRESSION_TEXT || !reserve(supplied.len()) {
            return Err(LiftedRenderError::Limit);
        }
        let (prefix, name) = self.parts();
        append(output, prefix, reserve)?;
        append(output, name, reserve)
    }
}

pub(crate) fn operand_names(opcode: u8, arity: usize) -> Option<&'static [&'static str]> {
    let names: &[&str] = match opcode {
        0x01..=0x03 | 0x10..=0x14 | 0x16..=0x18 => LEFT_RIGHT,
        0x04..=0x07 => &["dividend", "divisor"],
        0x08 | 0x09 => &["left", "right", "modulus"],
        0x0a => &["base", "exponent"],
        0x0b => &["byte_index", "value"],
        0x15 | 0x19 | 0x1e | 0x50 => &["value"],
        0x1a => &["index", "value"],
        0x1b..=0x1d => &["shift", "value"],
        0x20 | 0xa0 | 0xf3 | 0xfd => &["offset", "size"],
        0x31 | 0x3b | 0x3f => &["address"],
        0x35 | 0x51 => &["offset"],
        0x37 => &["memory_offset", "calldata_offset", "size"],
        0x39 => &["memory_offset", "code_offset", "size"],
        0x3c => &["address", "memory_offset", "code_offset", "size"],
        0x3e => &["memory_offset", "returndata_offset", "size"],
        0x40 => &["number"],
        0x49 => &["index"],
        0x52 | 0x53 => &["offset", "value"],
        0x54 | 0x5c => &["slot"],
        0x55 | 0x5d => &["slot", "value"],
        0x56 => &["target"],
        0x57 => &["target", "condition"],
        0x5e => &["dest_offset", "src_offset", "size"],
        0xa1 => &["offset", "size", "topic0"],
        0xa2 => &["offset", "size", "topic0", "topic1"],
        0xa3 => &["offset", "size", "topic0", "topic1", "topic2"],
        0xa4 => &["offset", "size", "topic0", "topic1", "topic2", "topic3"],
        0xf0 => &["value", "init_offset", "init_size"],
        0xf1 | 0xf2 => &[
            "gas",
            "target",
            "value",
            "input_offset",
            "input_size",
            "output_offset",
            "output_size",
        ],
        0xf4 | 0xfa => &[
            "gas",
            "target",
            "input_offset",
            "input_size",
            "output_offset",
            "output_size",
        ],
        0xf5 => &["value", "init_offset", "init_size", "salt"],
        0xff => &["beneficiary"],
        _ => return None,
    };
    (names.len() == arity).then_some(names)
}

/// Charge bytes before copying them; callers expose only a completed local row.
pub(super) fn append(
    output: &mut String,
    text: &str,
    reserve: &mut impl FnMut(usize) -> bool,
) -> Result<(), LiftedRenderError> {
    if text.len() > MAX_LIFTED_EXPRESSION_TEXT.saturating_sub(output.len()) || !reserve(text.len())
    {
        return Err(LiftedRenderError::Limit);
    }
    output.push_str(text);
    Ok(())
}
