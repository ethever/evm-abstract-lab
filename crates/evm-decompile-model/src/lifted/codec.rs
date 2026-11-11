//! Object-only enum payloads and bounded flat arena admission.
use super::{
    LiftedBlock, LiftedConstruction, LiftedControl, LiftedErrorPayload, LiftedExpression,
    LiftedExpressionKind, LiftedFailure, LiftedProgram, LiftedRelation, LiftedRevert,
    LiftedRevertData, LiftedTransfer, MAX_LIFTED_RECORDS,
};
use crate::{EntityId, U256};
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

strict_record! { pub struct Compare { pub relation: LiftedRelation, pub left: EntityId, pub right: EntityId } }
strict_record! { pub struct WordOp { pub opcode: u8, pub arguments: Vec<EntityId> } }
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum KindWire {
    Word(U256),
    Predicate(bool),
    Value(EntityId),
    WordFromPredicate(EntityId),
    NonZero(EntityId),
    Not(EntityId),
    Compare(Compare),
    WordOp(WordOp),
}
impl<'de> Deserialize<'de> for LiftedExpressionKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match KindWire::deserialize(deserializer)? {
            KindWire::Word(v) => Self::Word(v),
            KindWire::Predicate(v) => Self::Predicate(v),
            KindWire::Value(v) => Self::Value(v),
            KindWire::WordFromPredicate(v) => Self::WordFromPredicate(v),
            KindWire::NonZero(v) => Self::NonZero(v),
            KindWire::Not(v) => Self::Not(v),
            KindWire::Compare(Compare {
                relation,
                left,
                right,
            }) => Self::Compare {
                relation,
                left,
                right,
            },
            KindWire::WordOp(WordOp { opcode, arguments }) => Self::WordOp { opcode, arguments },
        })
    }
}
#[derive(Serialize)]
pub struct Bounded<T>(pub(crate) Vec<T>);
impl<T> Default for Bounded<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Bounded<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Items<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Items<T> {
            type Value = Bounded<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bounded lifted record array")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    if items.len() == MAX_LIFTED_RECORDS {
                        return Err(serde::de::Error::custom("lifted record limit exceeded"));
                    }
                    items.push(item);
                }
                Ok(Bounded(items))
            }
        }
        deserializer.deserialize_seq(Items(std::marker::PhantomData))
    }
}

/// New variable-size arenas are charged while reading, before the complete
/// program's cross-arena validation. One bounded item may be decoded to learn
/// its weight; an oversized prefix never allocates the remaining arena.
#[derive(Serialize)]
pub(crate) struct Weighted<T>(pub(crate) Vec<T>);
impl<T> Default for Weighted<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}
pub(crate) trait WireWeight {
    fn wire_weight(&self) -> usize;
}
impl WireWeight for LiftedErrorPayload {
    fn wire_weight(&self) -> usize {
        1_usize
            .saturating_add(self.message.len())
            .saturating_add(self.bytes.len())
    }
}
impl WireWeight for LiftedRevert {
    fn wire_weight(&self) -> usize {
        10_usize
            .saturating_add(self.writes.len())
            .saturating_add(usize::from(self.code_hash.is_some()).saturating_mul(32))
    }
}
impl WireWeight for LiftedConstruction {
    fn wire_weight(&self) -> usize {
        6
    }
}
impl<'de, T: Deserialize<'de> + WireWeight> Deserialize<'de> for Weighted<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Items<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de> + WireWeight> Visitor<'de> for Items<T> {
            type Value = Weighted<T>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a bounded weighted semantic arena")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                let mut used = 0_usize;
                while let Some(item) = access.next_element::<T>()? {
                    used = used.saturating_add(item.wire_weight());
                    if used > MAX_LIFTED_RECORDS {
                        return Err(serde::de::Error::custom("weighted semantic arena limit"));
                    }
                    items.push(item);
                }
                Ok(Weighted(items))
            }
        }
        deserializer.deserialize_seq(Items(std::marker::PhantomData))
    }
}
strict_record! { pub struct ProgramWire {
    pub expressions: Bounded<LiftedExpression>, pub blocks: Bounded<LiftedBlock>,
    pub transfers: Bounded<LiftedTransfer>, pub controls: Bounded<LiftedControl>,
    pub failures: Bounded<LiftedFailure> = Bounded::default(),
    pub payloads: Weighted<LiftedErrorPayload> = Weighted::default(),
    pub reverts: Weighted<LiftedRevert> = Weighted::default(),
    pub revert_data: Weighted<LiftedRevertData> = Weighted::default(),
    pub constructions: Weighted<LiftedConstruction> = Weighted::default(),
    pub procedures: Weighted<crate::SharedProcedure> = Weighted::default(),
    pub invocations: Weighted<crate::SharedInvocation> = Weighted::default(),
} }
impl<'de> Deserialize<'de> for LiftedProgram {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ProgramWire::deserialize(deserializer)?;
        let program = Self {
            expressions: wire.expressions.0,
            blocks: wire.blocks.0,
            transfers: wire.transfers.0,
            controls: wire.controls.0,
            failures: wire.failures.0,
            payloads: wire.payloads.0,
            reverts: wire.reverts.0,
            revert_data: wire.revert_data.0,
            constructions: wire.constructions.0,
            procedures: wire.procedures.0,
            invocations: wire.invocations.0,
        };
        program.validate().map_err(serde::de::Error::custom)?;
        Ok(program)
    }
}

#[cfg(test)]
mod tests;

impl WireWeight for crate::SharedProcedure {
    fn wire_weight(&self) -> usize {
        3 + self.interface.record_count()
    }
}
impl WireWeight for crate::SharedInvocation {
    fn wire_weight(&self) -> usize {
        6_usize.saturating_add(self.bindings.len().saturating_mul(2))
    }
}
