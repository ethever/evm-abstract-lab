use super::{
    LiftedByteSegment, LiftedByteSource, LiftedDataWord, LiftedRevertData, LiftedRevertDataKind,
};
use crate::EntityId;
use crate::lifted::codec::{Bounded, Weighted, WireWeight};
use serde::Deserialize;
use serde::de::{SeqAccess, Visitor};

struct Bytes(Vec<u8>);
impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ByteVisitor;
        impl<'de> Visitor<'de> for ByteVisitor {
            type Value = Bytes;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bounded literal byte segment")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Bytes, A::Error> {
                let mut bytes = Vec::new();
                while let Some(byte) = seq.next_element()? {
                    if bytes.len() == crate::MAX_REVERT_DATA_BYTES {
                        return Err(serde::de::Error::custom("literal byte limit"));
                    }
                    bytes.push(byte);
                }
                Ok(Bytes(bytes))
            }
        }
        d.deserialize_seq(ByteVisitor)
    }
}
// Object-only named variants keep duplicate/unknown field rejection identical
// to the rest of the semantic wire format.
strict_record! { pub struct WordWire { pub value:LiftedDataWord,pub offset:u8,pub size:u8 } }
strict_record! { pub struct SliceWire { pub source:LiftedByteSource,pub offset:LiftedDataWord,pub size:LiftedDataWord } }
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum SegmentWire {
    Bytes(Bytes),
    Word(WordWire),
    Slice(SliceWire),
}
impl<'de> Deserialize<'de> for LiftedByteSegment {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match SegmentWire::deserialize(d)? {
            SegmentWire::Bytes(Bytes(bytes)) => Self::Bytes(bytes),
            SegmentWire::Word(WordWire {
                value,
                offset,
                size,
            }) => Self::Word {
                value,
                offset,
                size,
            },
            SegmentWire::Slice(SliceWire {
                source,
                offset,
                size,
            }) => Self::Slice {
                source,
                offset,
                size,
            },
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ContentWire {
    Segments(Weighted<LiftedByteSegment>),
    Raw,
}
impl<'de> Deserialize<'de> for LiftedRevertDataKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match ContentWire::deserialize(d)? {
            ContentWire::Segments(parts) => Self::Segments(parts.0),
            ContentWire::Raw => Self::Raw,
        })
    }
}
strict_record! { pub struct DataWire {
    pub id:EntityId,
    pub execution_domain:crate::ExecutionDomain=crate::ExecutionDomain::Generic,
    pub execution_fork:Option<crate::Fork>=None,
    pub code_hash:Option<alloy_primitives::B256>=None,
    pub block:EntityId,pub operation:EntityId,pub memory:EntityId,pub offset:EntityId,pub size:EntityId,
    pub content:LiftedRevertDataKind,pub writes:Bounded<EntityId>,
} }
impl<'de> Deserialize<'de> for LiftedRevertData {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = DataWire::deserialize(d)?;
        let result = Self {
            id: wire.id,
            execution_domain: wire.execution_domain,
            execution_fork: wire.execution_fork,
            code_hash: wire.code_hash,
            block: wire.block,
            operation: wire.operation,
            memory: wire.memory,
            offset: wire.offset,
            size: wire.size,
            content: wire.content,
            writes: wire.writes.0,
        };
        result.validate().map_err(serde::de::Error::custom)?;
        Ok(result)
    }
}
impl WireWeight for LiftedByteSegment {
    fn wire_weight(&self) -> usize {
        self.record_count()
    }
}
impl WireWeight for LiftedRevertData {
    fn wire_weight(&self) -> usize {
        self.record_count()
    }
}
