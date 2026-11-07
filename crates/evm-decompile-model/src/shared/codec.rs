//! Check nested map/interface sizes while consuming the hostile prefix.
use super::{
    MAX_SHARED_REGION_ENTITIES, SharedBinding, SharedContinuation, SharedInterface,
    SharedInvocation,
};
use crate::EntityId;
use serde::{
    Deserialize,
    de::{SeqAccess, Visitor},
};
pub struct Small<T>(Vec<T>);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Small<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Items<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Items<T> {
            type Value = Small<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bounded shared source array")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    if items.len() == MAX_SHARED_REGION_ENTITIES {
                        return Err(serde::de::Error::custom("shared source array limit"));
                    }
                    items.push(item);
                }
                Ok(Small(items))
            }
        }
        d.deserialize_seq(Items(std::marker::PhantomData))
    }
}
impl<T: serde::Serialize> serde::Serialize for Small<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}
strict_record! { pub struct InterfaceWire {
    pub inputs: Small<EntityId>, pub outputs: Small<EntityId>,
    pub memory_inputs: Small<EntityId>, pub memory_outputs: Small<EntityId>,
    pub effect_inputs: Small<EntityId>, pub effect_outputs: Small<EntityId>,
    pub continuations: Small<SharedContinuation>,
} }
impl<'de> Deserialize<'de> for SharedInterface {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = InterfaceWire::deserialize(d)?;
        let v = Self {
            inputs: w.inputs.0,
            outputs: w.outputs.0,
            memory_inputs: w.memory_inputs.0,
            memory_outputs: w.memory_outputs.0,
            effect_inputs: w.effect_inputs.0,
            effect_outputs: w.effect_outputs.0,
            continuations: w.continuations.0,
        };
        if v.record_count() > MAX_SHARED_REGION_ENTITIES {
            return Err(serde::de::Error::custom("shared interface limit"));
        }
        Ok(v)
    }
}
strict_record! { pub struct InvocationWire {
    pub id:EntityId,pub procedure:EntityId,pub source:EntityId,pub owner:Option<EntityId>,pub position:usize,
    pub continuation:Option<EntityId>,pub bindings:Small<SharedBinding>,
} }
impl<'de> Deserialize<'de> for SharedInvocation {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = InvocationWire::deserialize(d)?;
        Ok(Self {
            id: w.id,
            procedure: w.procedure,
            source: w.source,
            owner: w.owner,
            position: w.position,
            continuation: w.continuation,
            bindings: w.bindings.0,
        })
    }
}
