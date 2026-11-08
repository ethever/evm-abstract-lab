//! Opcode display policy shared by inline and retained primitive expressions.
//! Only exact arities select operators or named operands; diagnostic fallbacks
//! preserve every input. This policy never authorizes an algebraic rewrite.

use super::{LiftedRenderError, MAX_LIFTED_EXPRESSION_TEXT};

/// A fully parenthesized operator cannot inherit a consumer's precedence rules.
/// Comparisons producing EVM words retain their explicit 0/1 conversion.
#[derive(Clone, Copy)]
pub(super) enum PrimitiveFormat {
    Call(Option<&'static [&'static str]>),
    Operator {
        opening: &'static str,
        separator: &'static str,
        closing: &'static str,
        reverse: bool,
    },
}

impl PrimitiveFormat {
    pub(super) const POSITIONAL: Self = Self::Call(None);

    pub(super) fn for_opcode(opcode: u8, arity: usize) -> Self {
        let symbol = match (opcode, arity) {
            (0x01, 2) => "+",
            (0x02, 2) => "*",
            (0x03, 2) => "-",
            (0x04, 2) => "/",
            (0x06, 2) => "%",
            (0x0a, 2) => "**",
            (0x10, 2) => "<",
            (0x11, 2) => ">",
            (0x14, 2) => "==",
            (0x16, 2) => "&",
            (0x17, 2) => "|",
            (0x18, 2) => "^",
            (0x1b, 2) => "<<",
            (0x1c, 2) => ">>",
            (0x15, 1) => {
                return Self::Operator {
                    opening: "word((",
                    separator: "",
                    closing: " == 0))",
                    reverse: false,
                };
            }
            (0x19, 1) => {
                return Self::Operator {
                    opening: "(~",
                    separator: "",
                    closing: ")",
                    reverse: false,
                };
            }
            _ => return Self::Call(operand_names(opcode, arity)),
        };
        let word_comparison = matches!(opcode, 0x10 | 0x11 | 0x14);
        Self::Operator {
            opening: if word_comparison { "word((" } else { "(" },
            separator: symbol,
            closing: if word_comparison { "))" } else { ")" },
            reverse: matches!(opcode, 0x1b | 0x1c),
        }
    }

    pub(super) fn names(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Call(names) => names,
            Self::Operator { .. } => None,
        }
    }

    pub(super) fn argument(self, position: usize, arity: usize) -> usize {
        if matches!(self, Self::Operator { reverse: true, .. }) {
            arity - 1 - position
        } else {
            position
        }
    }

    pub(super) fn opening(self, name: CallName<'_>) -> String {
        match self {
            Self::Call(_) => name.opening(),
            Self::Operator { opening, .. } => opening.to_owned(),
        }
    }

    pub(super) fn append_opening(
        self,
        name: CallName<'_>,
        output: &mut String,
        reserve: &mut impl FnMut(usize) -> bool,
    ) -> Result<(), LiftedRenderError> {
        match self {
            Self::Call(_) => {
                name.append(output, reserve)?;
                append(output, "(", reserve)
            }
            Self::Operator { opening, .. } => append(output, opening, reserve),
        }
    }

    pub(super) fn separator(self) -> String {
        match self {
            Self::Call(_) => ", ".to_owned(),
            Self::Operator { separator, .. } => format!(" {separator} "),
        }
    }

    pub(super) fn closing(self) -> &'static str {
        match self {
            Self::Call(_) => ")",
            Self::Operator { closing, .. } => closing,
        }
    }
}

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

fn operand_names(opcode: u8, arity: usize) -> Option<&'static [&'static str]> {
    let names: &[&str] = match opcode {
        0xa0 | 0xf3 | 0xfd => &["offset", "size"],
        0x35 | 0x51 => &["offset"],
        0x37 => &["memory_offset", "calldata_offset", "size"],
        0x39 => &["memory_offset", "code_offset", "size"],
        0x3c => &["address", "memory_offset", "code_offset", "size"],
        0x3e => &["memory_offset", "returndata_offset", "size"],
        0x52 | 0x53 => &["offset", "value"],
        0x54 | 0x5c => &["slot"],
        0x55 | 0x5d => &["slot", "value"],
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
