//! Occurrence-local byte data, independent of its decoded error format.
//! SourceValue always names a raw SSA word. Only record IDs are semantic arena
//! identities; raw references must never pass through expression remapping.
use crate::{EntityId, U256};
use serde::{Deserialize, Serialize};
mod codec;
mod format;
pub(in crate::lifted) mod prepared;
mod sources;
pub use format::LiftedRevertFormat;
pub(super) use sources::check as check_sources;
/// Maximum literal materialization for one demand-driven byte query.
pub const MAX_REVERT_DATA_BYTES: usize = 4_164;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// A byte-producing word, with raw identity distinct from lifted arena IDs.
pub enum LiftedDataWord {
    /// A proved modulo-256-bit constant.
    Constant(U256),
    /// The actual raw writer operand; consumers resolve its lifted binding.
    SourceValue(EntityId),
}
impl LiftedDataWord {
    /// Raw SSA reference, when this is not a literal.
    pub fn source_reference(self) -> Option<EntityId> {
        match self {
            Self::SourceValue(id) => Some(id),
            Self::Constant(_) => None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Source buffers whose identity is independent of destination memory.
pub enum LiftedByteSource {
    /// Calldata of the current source frame.
    Calldata,
    /// Raw source CALL-family operation which produced this returndata epoch.
    ReturnData(EntityId),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
/// One contiguous component of a complete byte result.
pub enum LiftedByteSegment {
    /// Exact source bytes, without an ABI or UTF-8 interpretation.
    Bytes(Vec<u8>),
    /// Big-endian bytes [offset, offset + size) of one raw source word.
    Word {
        /// Actual source operand or a proved literal.
        value: LiftedDataWord,
        /// First byte in big-endian word order.
        offset: u8,
        /// Nonzero byte count within that same word.
        size: u8,
    },
    /// An unchanged interval of a separately identified source buffer.
    Slice {
        /// Buffer identity, including a producing returndata epoch.
        source: LiftedByteSource,
        /// Raw source interval offset.
        offset: LiftedDataWord,
        /// Raw source interval length.
        size: LiftedDataWord,
    },
}
impl LiftedByteSegment {
    /// Retained wire/history units, including literal bytes and references.
    pub fn record_count(&self) -> usize {
        match self {
            Self::Bytes(bytes) => 1 + bytes.len(),
            Self::Word { .. } => 5,
            Self::Slice { .. } => 6,
        }
    }
    /// Raw source references; none are semantic arena IDs.
    pub fn source_references(&self) -> impl Iterator<Item = EntityId> + '_ {
        let refs = match self {
            Self::Bytes(_) => [None, None, None],
            Self::Word { value, .. } => [value.source_reference(), None, None],
            Self::Slice {
                source,
                offset,
                size,
            } => [
                match source {
                    LiftedByteSource::ReturnData(id) => Some(*id),
                    _ => None,
                },
                offset.source_reference(),
                size.source_reference(),
            ],
        };
        refs.into_iter().flatten()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Recovery result, separate from any decoded format.
pub enum LiftedRevertDataKind {
    /// Complete, ordered, contiguous concatenation. An empty vector is known
    /// empty data, unlike Raw, whose original memory slice remains opaque.
    Segments(Vec<LiftedByteSegment>),
    /// No structured byte equality was established; keep the original slice.
    Raw,
}
impl LiftedRevertDataKind {
    /// Complete segments, or an empty borrowed view for opaque data.
    pub fn segments(&self) -> &[LiftedByteSegment] {
        match self {
            Self::Segments(parts) => parts,
            Self::Raw => &[],
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
/// Terminal-on-reaching byte evidence bound to one original REVERT occurrence.
pub struct LiftedRevertData {
    /// Independent semantic record identity.
    pub id: EntityId,
    #[serde(skip_serializing_if = "crate::ExecutionDomain::is_generic")]
    /// Execution assumptions qualifying source address bounds.
    pub execution_domain: crate::ExecutionDomain,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Fork qualifying a non-generic execution domain.
    pub execution_fork: Option<crate::Fork>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Captured runtime identity when code contributes bytes.
    pub code_hash: Option<alloy_primitives::B256>,
    /// Original terminal block, never a materialized caller occurrence.
    pub block: EntityId,
    /// Original REVERT primitive.
    pub operation: EntityId,
    /// Exact raw memory version consumed by REVERT.
    pub memory: EntityId,
    /// Original raw offset operand.
    pub offset: EntityId,
    /// Original raw size operand.
    pub size: EntityId,
    /// Recovered byte expression, independent of decoding.
    pub content: LiftedRevertDataKind,
    /// Contributing original writes in execution order.
    pub writes: Vec<EntityId>,
}
impl LiftedRevertData {
    /// Retained wire/history units, including literal bytes and references.
    pub fn record_count(&self) -> usize {
        self.content.segments().iter().fold(
            11_usize
                .saturating_add(self.writes.len())
                .saturating_add(usize::from(self.code_hash.is_some()) * 32),
            |count, part| count.saturating_add(part.record_count()),
        )
    }
    /// Raw source references; none are semantic arena IDs.
    pub fn source_references(&self) -> impl Iterator<Item = EntityId> + '_ {
        [
            self.block,
            self.operation,
            self.memory,
            self.offset,
            self.size,
        ]
        .into_iter()
        .chain(self.writes.iter().copied())
        .chain(
            self.content
                .segments()
                .iter()
                .flat_map(LiftedByteSegment::source_references),
        )
    }
    /// Flat record admission; numeric source equality belongs to core replay.
    pub fn validate(&self) -> Result<(), crate::LiftedModelError> {
        let fail = || crate::LiftedModelError::Payload(self.id);
        if self.id.0 == 0
            || self.record_count() > super::MAX_LIFTED_RECORDS
            || self.execution_domain.is_generic() != self.execution_fork.is_none()
            || self.source_references().any(|id| id.0 == 0)
        {
            return Err(fail());
        }
        for part in self.content.segments() {
            match part {
                LiftedByteSegment::Bytes(bytes)
                    if bytes.is_empty() || bytes.len() > MAX_REVERT_DATA_BYTES =>
                {
                    return Err(fail());
                }
                LiftedByteSegment::Word { offset, size, .. }
                    if *size == 0 || u16::from(*offset) + u16::from(*size) > 32 =>
                {
                    return Err(fail());
                }
                LiftedByteSegment::Slice {
                    size: LiftedDataWord::Constant(size),
                    ..
                } if size.is_zero() => return Err(fail()),
                _ => {}
            }
        }
        Ok(())
    }
}
