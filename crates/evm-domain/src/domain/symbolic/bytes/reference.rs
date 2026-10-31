//! Reference byte-copy constructor shared by cross-crate differential tests.

use super::{ByteStateId, ByteStateKind, ByteView, ExprId, StateError, StateLimits, U256, state};

impl ByteStateId {
    /// Differential oracle that canonicalizes again at every target occurrence.
    /// Its guard and error order deliberately preserve the original algorithm.
    pub fn copy_recanonicalizing(
        &self,
        dst: ExprId,
        source: ByteView,
        len: ExprId,
        limits: StateLimits,
    ) -> Result<Self, StateError> {
        if len.as_constant() == Some(U256::ZERO) {
            state::check(self.nodes(), self.depth(), limits)?;
            return Ok(self.clone());
        }
        let source = source.canonicalize();
        if source.state == *self && source.offset == dst && source.length == len {
            state::check(self.nodes(), self.depth(), limits)?;
            return Ok(self.clone());
        }
        if let ByteStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = self.kind()
        {
            let yes = then_state.copy_recanonicalizing(
                dst.clone(),
                source.clone(),
                len.clone(),
                limits,
            )?;
            let no = else_state.copy_recanonicalizing(dst, source, len, limits)?;
            return yes.join_guarded(&no, condition.clone(), limits);
        }
        let previous = match self.kind() {
            ByteStateKind::Copy {
                previous,
                destination,
                source: old_source,
                length,
            } if *destination == dst && *length == len => {
                if *old_source == source {
                    state::check(self.nodes(), self.depth(), limits)?;
                    return Ok(self.clone());
                }
                previous
            }
            _ => self,
        };
        let children = [
            (previous.nodes(), previous.depth()),
            (dst.nodes(), dst.depth()),
            (source.nodes(), source.depth()),
            (len.nodes(), len.depth()),
        ];
        Self::build(
            ByteStateKind::Copy {
                previous: previous.clone(),
                destination: dst,
                source,
                length: len,
            },
            children,
            limits,
        )
    }
}
