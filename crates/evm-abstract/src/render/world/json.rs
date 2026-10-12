//! Streaming augmentation of the native evidence shape, without to_value trees.
use super::observation;
use crate::{
    analysis::{MachineOutcome, WorldAnalysis},
    world::{FingerprintError, Store, World},
};
use alloy_primitives::B256;
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq, SerializeStruct},
};

/// Borrowed native report plus computed account observations and snapshot hashes.
pub struct JsonEvidence<'a> {
    analysis: &'a WorldAnalysis,
    fingerprint: B256,
}
/// Capture the same native JSON fields and evidence without allocating a JSON tree.
/// The exact snapshot fingerprint is computed before any report output is emitted.
pub fn evidence(analysis: &WorldAnalysis) -> Result<JsonEvidence<'_>, FingerprintError> {
    Ok(JsonEvidence {
        analysis,
        fingerprint: analysis.world().fingerprint()?,
    })
}
#[derive(Clone, Copy)]
enum Extra<'a> {
    Analysis(&'a WorldAnalysis, B256),
    World(&'a World, B256),
    Outcome(&'a MachineOutcome),
    Store(&'a Store),
}
struct Augment<'a, S> {
    inner: S,
    extra: Extra<'a>,
}
struct Fields<'a, S> {
    inner: S,
    extra: Extra<'a>,
}
struct Evidence<'a>(Extra<'a>);
impl Serialize for JsonEvidence<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Evidence(Extra::Analysis(self.analysis, self.fingerprint)).serialize(serializer)
    }
}
impl Serialize for Evidence<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let serializer = Augment {
            inner: serializer,
            extra: self.0,
        };
        match self.0 {
            Extra::Analysis(v, _) => v.serialize(serializer),
            Extra::World(v, _) => v.serialize(serializer),
            Extra::Outcome(v) => v.serialize(serializer),
            Extra::Store(v) => v.serialize(serializer),
        }
    }
}
struct Outcomes<'a>(&'a WorldAnalysis);
impl Serialize for Outcomes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.outcomes().len()))?;
        for outcome in self.0.outcomes() {
            seq.serialize_element(&Evidence(Extra::Outcome(outcome)))?;
        }
        seq.end()
    }
}
struct CodeHashes<'a>(&'a World);
impl Serialize for CodeHashes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.accounts().len()))?;
        for address in self.0.accounts().keys() {
            map.serialize_entry(address, &self.0.code_hash(*address))?;
        }
        map.end()
    }
}
struct Observations<'a>(&'a Store);
impl Serialize for Observations<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let addresses = self.0.addresses();
        let mut seq = serializer.serialize_seq(Some(addresses.len()))?;
        for address in addresses {
            let observation = observation(self.0, address);
            seq.serialize_element(&observation)?;
        }
        seq.end()
    }
}
impl<S: SerializeStruct> SerializeStruct for Fields<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        match (self.extra, key) {
            (Extra::Analysis(analysis, fingerprint), "world") => self
                .inner
                .serialize_field(key, &Evidence(Extra::World(analysis.world(), fingerprint))),
            (Extra::Analysis(analysis, _), "outcomes") => {
                self.inner.serialize_field(key, &Outcomes(analysis))
            }
            (Extra::Outcome(outcome), "store") => self
                .inner
                .serialize_field(key, &Evidence(Extra::Store(&outcome.store))),
            _ => self.inner.serialize_field(key, value),
        }
    }
    fn end(mut self) -> Result<Self::Ok, Self::Error> {
        match self.extra {
            Extra::World(world, fingerprint) => {
                self.inner.serialize_field("fingerprint", &fingerprint)?;
                self.inner
                    .serialize_field("code_hashes", &CodeHashes(world))?;
            }
            Extra::Store(store) => self
                .inner
                .serialize_field("account_observations", &Observations(store))?,
            Extra::Analysis(_, _) | Extra::Outcome(_) => {}
        }
        self.inner.end()
    }
}
impl<'a, S: Serializer> Serializer for Augment<'a, S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = S::SerializeSeq;
    type SerializeTuple = S::SerializeTuple;
    type SerializeTupleStruct = S::SerializeTupleStruct;
    type SerializeTupleVariant = S::SerializeTupleVariant;
    type SerializeMap = S::SerializeMap;
    type SerializeStruct = Fields<'a, S::SerializeStruct>;
    type SerializeStructVariant = S::SerializeStructVariant;
    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_bool(v)
    }
    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i8(v)
    }
    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i16(v)
    }
    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i32(v)
    }
    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i64(v)
    }
    fn serialize_i128(self, v: i128) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i128(v)
    }
    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u8(v)
    }
    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u16(v)
    }
    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u32(v)
    }
    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u64(v)
    }
    fn serialize_u128(self, v: u128) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u128(v)
    }
    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_f32(v)
    }
    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_f64(v)
    }
    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_char(v)
    }
    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_str(v)
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_bytes(v)
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_none()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_some(v)
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_unit()
    }
    fn serialize_unit_struct(self, n: &'static str) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_unit_struct(n)
    }
    fn serialize_unit_variant(
        self,
        n: &'static str,
        i: u32,
        v: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_unit_variant(n, i, v)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        n: &'static str,
        v: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_newtype_struct(n, v)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        n: &'static str,
        i: u32,
        k: &'static str,
        v: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_newtype_variant(n, i, k, v)
    }
    fn serialize_seq(self, n: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.inner.serialize_seq(n)
    }
    fn serialize_tuple(self, n: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.inner.serialize_tuple(n)
    }
    fn serialize_tuple_struct(
        self,
        k: &'static str,
        n: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.inner.serialize_tuple_struct(k, n)
    }
    fn serialize_tuple_variant(
        self,
        k: &'static str,
        i: u32,
        v: &'static str,
        n: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.inner.serialize_tuple_variant(k, i, v, n)
    }
    fn serialize_map(self, n: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.inner.serialize_map(n)
    }
    fn serialize_struct(
        self,
        n: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        let extra = match self.extra {
            Extra::World(_, _) => 2,
            Extra::Store(_) => 1,
            _ => 0,
        };
        Ok(Fields {
            inner: self.inner.serialize_struct(n, len.saturating_add(extra))?,
            extra: self.extra,
        })
    }
    fn serialize_struct_variant(
        self,
        k: &'static str,
        i: u32,
        v: &'static str,
        n: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.inner.serialize_struct_variant(k, i, v, n)
    }
    fn collect_str<T: std::fmt::Display + ?Sized>(self, v: &T) -> Result<Self::Ok, Self::Error> {
        self.inner.collect_str(v)
    }
    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}
