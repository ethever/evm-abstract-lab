//! Factor an identical ordered update above its selected previous snapshots.

use super::{ByteStateId, ByteStateKind, ExprId, StateError, StateLimits};

impl ByteStateId {
    pub(super) fn join_update(
        &self,
        other: &Self,
        selector: ExprId,
        limits: StateLimits,
    ) -> Result<Option<Self>, StateError> {
        match (self.kind(), other.kind()) {
            (
                ByteStateKind::WriteByte {
                    previous: yes,
                    offset,
                    value,
                },
                ByteStateKind::WriteByte {
                    previous: no,
                    offset: other_offset,
                    value: other_value,
                },
            )
            | (
                ByteStateKind::WriteWord {
                    previous: yes,
                    offset,
                    value,
                },
                ByteStateKind::WriteWord {
                    previous: no,
                    offset: other_offset,
                    value: other_value,
                },
            ) if offset == other_offset && value == other_value => {
                // Both branches perform the same update, including any guards
                // inside its operands. Retain that history once above the join.
                let previous = yes.join_guarded(no, selector, limits)?;
                let kind = if matches!(self.kind(), ByteStateKind::WriteByte { .. }) {
                    ByteStateKind::WriteByte {
                        previous: previous.clone(),
                        offset: offset.clone(),
                        value: value.clone(),
                    }
                } else {
                    ByteStateKind::WriteWord {
                        previous: previous.clone(),
                        offset: offset.clone(),
                        value: value.clone(),
                    }
                };
                return Self::build(
                    kind,
                    [
                        (previous.nodes(), previous.depth()),
                        (offset.nodes(), offset.depth()),
                        (value.nodes(), value.depth()),
                    ],
                    limits,
                )
                .map(Some);
            }
            (
                ByteStateKind::Copy {
                    previous: yes,
                    destination,
                    source,
                    length,
                },
                ByteStateKind::Copy {
                    previous: no,
                    destination: other_destination,
                    source: other_source,
                    length: other_length,
                },
            ) if destination == other_destination
                && source == other_source
                && length == other_length =>
            {
                // A captured copy source is immutable and shared only when its
                // complete view matches; equal lengths alone never suffice.
                let previous = yes.join_guarded(no, selector, limits)?;
                return Self::build(
                    ByteStateKind::Copy {
                        previous: previous.clone(),
                        destination: destination.clone(),
                        source: source.clone(),
                        length: length.clone(),
                    },
                    [
                        (previous.nodes(), previous.depth()),
                        (destination.nodes(), destination.depth()),
                        (source.nodes(), source.depth()),
                        (length.nodes(), length.depth()),
                    ],
                    limits,
                )
                .map(Some);
            }
            _ => {}
        }
        Ok(None)
    }
}
