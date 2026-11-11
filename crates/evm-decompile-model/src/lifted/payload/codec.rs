//! New variable-size proof fields have individual ceilings before admission.
use super::{LiftedErrorPayload, LiftedRevert, MAX_ERROR_PAYLOAD_BYTES};
use crate::EntityId;
use serde::{Deserialize, Serialize};
#[derive(Serialize)]
pub struct Items<T>(Vec<T>);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Items<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
            type Value = Items<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bounded payload array")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = a.next_element()? {
                    if items.len() == MAX_ERROR_PAYLOAD_BYTES {
                        return Err(serde::de::Error::custom("payload array limit"));
                    }
                    items.push(item);
                }
                Ok(Items(items))
            }
        }
        d.deserialize_seq(Visitor(std::marker::PhantomData))
    }
}
#[derive(Serialize)]
pub struct Text(String);
impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Text;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bounded exact payload string")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Text, E> {
                if value.len() > MAX_ERROR_PAYLOAD_BYTES - 68 {
                    return Err(E::custom("payload string limit"));
                }
                Ok(Text(value.to_owned()))
            }
        }
        d.deserialize_str(Visitor)
    }
}
strict_record! { pub struct PayloadWire { pub id:EntityId,pub message:Text,pub bytes:Items<u8> } }
strict_record! { pub struct RevertWire {
    pub execution_domain:crate::ExecutionDomain = crate::ExecutionDomain::Generic,
    pub execution_fork:Option<crate::Fork> = None,
    pub code_hash:Option<alloy_primitives::B256> = None,
    pub id:EntityId,pub block:EntityId,pub operation:EntityId,pub memory:EntityId,
    pub offset:EntityId,pub size:EntityId,pub payload:EntityId,pub writes:Items<EntityId>,
} }
impl<'de> Deserialize<'de> for LiftedErrorPayload {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = PayloadWire::deserialize(d)?;
        let result = Self {
            id: wire.id,
            message: wire.message.0,
            bytes: wire.bytes.0,
        };
        if !result.is_canonical() {
            return Err(serde::de::Error::custom("noncanonical Error(string) bytes"));
        }
        Ok(result)
    }
}
impl<'de> Deserialize<'de> for LiftedRevert {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = RevertWire::deserialize(d)?;
        if w.execution_domain.is_generic() != w.execution_fork.is_none() {
            return Err(serde::de::Error::custom(
                "inconsistent payload execution scope",
            ));
        }
        Ok(Self {
            execution_domain: w.execution_domain,
            execution_fork: w.execution_fork,
            code_hash: w.code_hash,
            id: w.id,
            block: w.block,
            operation: w.operation,
            memory: w.memory,
            offset: w.offset,
            size: w.size,
            payload: w.payload,
            writes: w.writes.0,
        })
    }
}
