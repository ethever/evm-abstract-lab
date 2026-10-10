//! Shared presentation of analysis identities. Numeric IDs remain unchanged;
//! Unicode text works in exports and accessibility, while a UI can lay out
//! the same indices with its own fonts and baseline metrics.

use std::fmt;

#[cfg(test)]
mod tests;

/// An indexed identity with an explicit presentation namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symbol {
    /// Native machine state.
    State(usize),
    /// State in a single-program projection, distinct from the native ID.
    ProjectedState(usize),
    /// Captured executable program.
    Program(usize),
    /// Program-local basic block.
    Block(usize),
    /// Native control-flow edge.
    Edge(usize),
    /// SSA machine effect token.
    Effect(usize),
    /// Frame within a native state.
    Frame(usize),
    /// Transition in an SSA export.
    Transition(usize),
    /// Transaction outcome.
    Outcome(usize),
    /// Captured code alias in a world export.
    Code(usize),
    /// Unresolved analysis frontier.
    Frontier(usize),
    /// Account alias in a world export.
    Account(usize),
    /// Store snapshot.
    Store(usize),
    /// Reusable analysis summary.
    Summary(usize),
    /// Summary certificate, distinct from captured code.
    Certificate(usize),
    /// Content-hash alias.
    Hash(usize),
    /// Recorded log.
    Log(usize),
    /// Byte-array snapshot.
    ByteArray(usize),
}

/// Identity namespaces whose indices stay 64-bit on a 32-bit browser target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WideSymbol {
    /// Symbolic expression in a report-local expression graph.
    Expression(u64),
    /// Fresh symbolic identity.
    Fresh(u64),
    /// Input or analysis identity scope.
    Scope(u64),
    /// Backend analysis task.
    Task(u64),
}

impl fmt::Display for WideSymbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (base, id) = match self {
            Self::Expression(id) => ("expr", id),
            Self::Fresh(id) => ("fresh", id),
            Self::Scope(id) => ("scope", id),
            Self::Task(id) => ("task", id),
        };
        write!(f, "{base}{}", decimal_subscript(&id.to_string()))
    }
}

impl Symbol {
    /// The namespace marker, without the numeric index.
    pub const fn base(self) -> &'static str {
        match self {
            Self::State(_) => "σ",
            Self::ProjectedState(_) => "σᵖ",
            Self::Program(_) => "P",
            Self::Block(_) => "B",
            Self::Edge(_) => "e",
            Self::Effect(_) => "μ",
            Self::Frame(_) => "f",
            Self::Transition(_) => "T",
            Self::Outcome(_) => "O",
            Self::Code(_) => "C",
            Self::Frontier(_) => "U",
            Self::Account(_) => "A",
            Self::Store(_) => "store",
            Self::Summary(_) => "summary",
            Self::Certificate(_) => "cert",
            Self::Hash(_) => "H",
            Self::Log(_) => "L",
            Self::ByteArray(_) => "bytes",
        }
    }

    /// The original numeric ID; formatting never renumbers it.
    pub const fn index(self) -> usize {
        match self {
            Self::State(id)
            | Self::ProjectedState(id)
            | Self::Program(id)
            | Self::Block(id)
            | Self::Edge(id)
            | Self::Effect(id)
            | Self::Frame(id)
            | Self::Transition(id)
            | Self::Outcome(id)
            | Self::Code(id)
            | Self::Frontier(id)
            | Self::Account(id)
            | Self::Store(id)
            | Self::Summary(id)
            | Self::Certificate(id)
            | Self::Hash(id)
            | Self::Log(id)
            | Self::ByteArray(id) => id,
        }
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.base(), subscript(self.index()))
    }
}

/// Format every decimal digit of an index as a Unicode subscript.
pub fn subscript(index: usize) -> String {
    decimal_subscript(&index.to_string())
}

fn decimal_subscript(index: &str) -> String {
    const DIGITS: [char; 10] = ['₀', '₁', '₂', '₃', '₄', '₅', '₆', '₇', '₈', '₉'];
    index
        .bytes()
        .map(|digit| DIGITS[usize::from(digit - b'0')])
        .collect()
}

/// The ordinary digit represented by a subscript glyph, if any.
pub const fn subscript_digit(character: char) -> Option<char> {
    match character {
        '₀' => Some('0'),
        '₁' => Some('1'),
        '₂' => Some('2'),
        '₃' => Some('3'),
        '₄' => Some('4'),
        '₅' => Some('5'),
        '₆' => Some('6'),
        '₇' => Some('7'),
        '₈' => Some('8'),
        '₉' => Some('9'),
        _ => None,
    }
}

/// Replace subscript digits with ordinary digits, preserving all other text.
pub fn normalize_subscripts(text: &str) -> String {
    text.chars()
        .map(|character| subscript_digit(character).unwrap_or(character))
        .collect()
}

/// Read an original native state ID from a numeric or symbolic search query.
/// Canonical Unicode, `σ128`, `sigma_128`, `\sigma_{128}`, and previous `S128`
/// labels are accepted; no input can select a different namespace implicitly.
pub fn parse_state(text: &str) -> Option<usize> {
    let text = text.trim();
    let text = text
        .strip_prefix("\\sigma")
        .or_else(|| text.strip_prefix("sigma"))
        .or_else(|| text.strip_prefix('σ'))
        .or_else(|| text.strip_prefix('S'))
        .or_else(|| text.strip_prefix('s'))
        .unwrap_or(text);
    let text = text.strip_prefix('_').unwrap_or(text);
    let text = if let Some(text) = text.strip_prefix('{') {
        text.strip_suffix('}')?
    } else {
        text
    };
    if text.is_empty() {
        return None;
    }
    let digits = normalize_subscripts(text);
    digits
        .bytes()
        .all(|digit| digit.is_ascii_digit())
        .then_some(())?;
    digits.parse().ok()
}
