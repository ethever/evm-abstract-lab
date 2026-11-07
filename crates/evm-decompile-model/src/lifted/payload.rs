//! Bounded byte identity and lazy source construction are separate certificates.
use crate::EntityId;
mod codec;
mod construction;
mod failures;
mod sources;
pub(in crate::lifted) use failures::check_failure_payloads;
pub(super) use sources::check as check_sources;

/// Transport ceiling for one complete ABI Error(string) payload.
pub const MAX_ERROR_PAYLOAD_BYTES: usize = 4_164;

/// One canonical, valid UTF-8 Error(string), shared across source constructors.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LiftedErrorPayload {
    /// Semantic arena identity, independent of all source memory identities.
    pub id: EntityId,
    /// Exact string bytes decoded without replacement characters.
    pub message: String,
    /// Selector, ABI header, string bytes and explicit zero padding.
    pub bytes: Vec<u8>,
}
/// Terminal-on-reaching byte equality; alone this cannot hide earlier faults.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LiftedRevert {
    /// Semantic certificate identity.
    pub id: EntityId,
    /// Source terminal block.
    pub block: EntityId,
    /// Original REVERT primitive.
    pub operation: EntityId,
    /// Exact memory input version of REVERT.
    pub memory: EntityId,
    /// Original raw offset word identity.
    pub offset: EntityId,
    /// Original raw size word identity.
    pub size: EntityId,
    /// Canonical byte identity, never a constructor selection.
    pub payload: EntityId,
    /// Original contributing writes, ordered exactly as executed.
    pub writes: Vec<EntityId>,
}
strict_record! {
/// A reusable block-body capsule over unchanged statements and primitive faults.
/// Every materialization executes its own original source range lazily; a shared
/// plan does not select another constructor or redirect structured control.
/// Interpreting the compact display executes this exact original range first;
/// it never allocates an eager message on a successful guard route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftedConstruction {
    /// Semantic plan identity.
    pub id: EntityId,
    /// Source block owning the statement interval.
    pub block: EntityId,
    /// Inclusive semantic statement position.
    pub start: usize,
    /// Exclusive semantic statement position.
    pub end: usize,
    /// Terminal byte certificate associated with this actual source route.
    pub revert: EntityId,
}
}

impl LiftedErrorPayload {
    /// Check complete canonical ABI bytes, not just the visible string prefix.
    pub fn is_canonical(&self) -> bool {
        let len = self.message.len();
        let Some(padded) = len.checked_add(31).map(|n| n / 32 * 32) else {
            return false;
        };
        if self.bytes.len() > MAX_ERROR_PAYLOAD_BYTES || self.bytes.len() != 68 + padded {
            return false;
        }
        let mut offset = [0_u8; 32];
        offset[31] = 32;
        self.bytes[..4] == [0x08, 0xc3, 0x79, 0xa0]
            && self.bytes[4..36] == offset
            && self.bytes[36..68] == crate::U256::from(len).to_be_bytes::<32>()
            && self.bytes[68..68 + len] == *self.message.as_bytes()
            && self.bytes[68 + len..].iter().all(|b| *b == 0)
    }
}
