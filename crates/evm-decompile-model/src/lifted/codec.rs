//! Object-only enum payloads and bounded flat arena admission.
use super::{
    LiftedBlock, LiftedControl, LiftedExpression, LiftedExpressionKind, LiftedFailure,
    LiftedProgram, LiftedRelation, LiftedTransfer, MAX_LIFTED_RECORDS,
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
pub struct Bounded<T>(Vec<T>);
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
strict_record! { pub struct ProgramWire {
    pub expressions: Bounded<LiftedExpression>, pub blocks: Bounded<LiftedBlock>,
    pub transfers: Bounded<LiftedTransfer>, pub controls: Bounded<LiftedControl>,
    pub failures: Bounded<LiftedFailure> = Bounded::default(),
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
        };
        program.validate().map_err(serde::de::Error::custom)?;
        Ok(program)
    }
}
