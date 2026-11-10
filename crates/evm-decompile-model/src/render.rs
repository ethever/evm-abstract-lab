//! Monotone output accounting; ordinary branch layouts are planned separately
//! from typed semantic metadata before rendering, never inferred from this count.

/// Monotone summary of substantive rendered content. Structural annotations
/// are context; operations, diagnostics and control constructs are content.
/// A copy is a checkpoint: discarding a span never refunds prior output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderSummary {
    content: usize,
}

impl RenderSummary {
    /// One operation, diagnostic or control construct was displayed.
    pub const CONTENT: Self = Self { content: 1 };

    /// Summarize a renderer span containing only substantive rows.
    pub fn from_content(has_content: bool) -> Self {
        if has_content {
            Self::CONTENT
        } else {
            Self::default()
        }
    }

    /// Include a subtree without revisiting its rows.
    pub fn include(&mut self, other: Self) {
        self.content = self.content.saturating_add(other.content);
    }

    /// Content emitted after a checkpoint, excluding contextual rows.
    pub fn since(self, checkpoint: Self) -> Self {
        // An exhausted counter cannot certify an empty arm. Current renderers
        // are bounded well below this, but the shared summary fails closed.
        if self.content == usize::MAX {
            return Self::CONTENT;
        }
        Self {
            content: self.content.saturating_sub(checkpoint.content),
        }
    }

    /// Whether this output span contains substantive rows.
    pub const fn has_content(self) -> bool {
        self.content != 0
    }
}

#[cfg(test)]
mod tests {
    use super::RenderSummary;

    #[test]
    fn saturated_content_cannot_certify_an_empty_arm() {
        let mut summary = RenderSummary {
            content: usize::MAX - 1,
        };
        summary.include(RenderSummary::CONTENT);
        let checkpoint = summary;
        summary.include(RenderSummary::CONTENT);
        assert!(summary.since(checkpoint).has_content());
    }
}
