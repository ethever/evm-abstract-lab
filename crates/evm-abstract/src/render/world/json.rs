//! Streaming augmentation of the native evidence shape, without to_value trees.
use super::observation;
use crate::{
    analysis::{
        EvmMachineOutcome, EvmRootTerminalCoverage, EvmRootTerminalOutcome, EvmWorldAnalysis,
        HybridAnalysis, RpcHybridAnalysis, Status,
    },
    domain::WordElement,
    world::{EvmStore, EvmWorld, FingerprintError},
};
use alloy_primitives::B256;
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq, SerializeStruct},
};

/// Borrowed native report plus computed account observations and snapshot hashes.
pub struct EvmJsonEvidence<'a, N: WordElement> {
    analysis: &'a EvmWorldAnalysis<N>,
    fingerprint: B256,
    totals: Option<Totals>,
}
/// Capture native evidence without converting its selected numeric carrier.
/// The snapshot fingerprint is computed before any output is emitted.
pub fn evidence<N: WordElement>(
    analysis: &EvmWorldAnalysis<N>,
) -> Result<EvmJsonEvidence<'_, N>, FingerprintError> {
    Ok(EvmJsonEvidence {
        analysis,
        fingerprint: analysis.world().fingerprint()?,
        totals: None,
    })
}

#[derive(Clone, Copy)]
struct Totals {
    status: Status,
    work: usize,
    transfers: usize,
}

/// The wrapper's actual qualification governs status/counters while the graph
/// fields retain their selected namespace. No graph or receipt is cloned.
pub fn hybrid_evidence<N: WordElement>(
    source: &HybridAnalysis<N>,
) -> Result<EvmJsonEvidence<'_, N>, FingerprintError> {
    let mut view = evidence(source.chosen())?;
    view.totals = Some(Totals {
        status: source.status(),
        work: source.work(),
        transfers: source
            .detail()
            .unwrap_or_else(|| source.baseline())
            .transfers(),
    });
    Ok(view)
}
/// Persistent-session totals remain independent of the original baseline graph.
pub fn rpc_hybrid_evidence<N: WordElement>(
    source: &RpcHybridAnalysis<N>,
) -> Result<EvmJsonEvidence<'_, N>, FingerprintError> {
    let mut view = evidence(source.chosen())?;
    view.totals = Some(Totals {
        status: source.status(),
        work: source.work(),
        transfers: source.coverage().transfers,
    });
    Ok(view)
}

enum Extra<'a, N: WordElement> {
    Analysis(&'a EvmWorldAnalysis<N>, B256, Option<Totals>),
    World(&'a EvmWorld<N>, B256),
    Outcome(&'a EvmMachineOutcome<N>),
    TerminalCoverage(&'a EvmRootTerminalCoverage<N>),
    TerminalOutcome(&'a EvmRootTerminalOutcome<N>),
    Store(&'a EvmStore<N>),
}
// Only borrowed handles are copied. Deriving Copy would require the numeric
// element itself to be Copy, excluding valid independently owned domains.
impl<N: WordElement> Copy for Extra<'_, N> {}
impl<N: WordElement> Clone for Extra<'_, N> {
    fn clone(&self) -> Self {
        *self
    }
}

struct Augment<'a, S, N: WordElement> {
    inner: S,
    extra: Extra<'a, N>,
}
struct Fields<'a, S, N: WordElement> {
    inner: S,
    extra: Extra<'a, N>,
}
struct Evidence<'a, N: WordElement>(Extra<'a, N>);
impl<N: WordElement> Serialize for EvmJsonEvidence<'_, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Evidence(Extra::Analysis(
            self.analysis,
            self.fingerprint,
            self.totals,
        ))
        .serialize(serializer)
    }
}
impl<N: WordElement> Serialize for Evidence<'_, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let serializer = Augment {
            inner: serializer,
            extra: self.0,
        };
        match self.0 {
            Extra::Analysis(v, _, _) => v.serialize(serializer),
            Extra::World(v, _) => v.serialize(serializer),
            Extra::Outcome(v) => v.serialize(serializer),
            Extra::TerminalCoverage(v) => v.serialize(serializer),
            Extra::TerminalOutcome(v) => v.serialize(serializer),
            Extra::Store(v) => v.serialize(serializer),
        }
    }
}
struct Outcomes<'a, N: WordElement>(&'a EvmWorldAnalysis<N>);
impl<N: WordElement> Serialize for Outcomes<'_, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.outcomes().len()))?;
        for outcome in self.0.outcomes() {
            seq.serialize_element(&Evidence(Extra::Outcome(outcome)))?;
        }
        seq.end()
    }
}
struct CodeHashes<'a, N: WordElement>(&'a EvmWorld<N>);
struct TerminalOutcomes<'a, N: WordElement>(&'a [EvmRootTerminalOutcome<N>; 3]);
impl<N: WordElement> Serialize for TerminalOutcomes<'_, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for outcome in self.0 {
            seq.serialize_element(&Evidence(Extra::TerminalOutcome(outcome)))?;
        }
        seq.end()
    }
}
impl<N: WordElement> Serialize for CodeHashes<'_, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.accounts().len()))?;
        for address in self.0.accounts().keys() {
            map.serialize_entry(address, &self.0.code_hash(*address))?;
        }
        map.end()
    }
}
struct Observations<'a, N: WordElement>(&'a EvmStore<N>);
impl<N: WordElement> Serialize for Observations<'_, N> {
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
impl<S: SerializeStruct, N: WordElement> SerializeStruct for Fields<'_, S, N> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        match (self.extra, key) {
            (Extra::Analysis(_, _, Some(totals)), "status") => {
                self.inner.serialize_field(key, &totals.status)
            }
            (Extra::Analysis(_, _, Some(totals)), "work") => {
                self.inner.serialize_field(key, &totals.work)
            }
            (Extra::Analysis(_, _, Some(totals)), "transfers") => {
                self.inner.serialize_field(key, &totals.transfers)
            }
            (Extra::Analysis(analysis, fingerprint, _), "world") => self
                .inner
                .serialize_field(key, &Evidence(Extra::World(analysis.world(), fingerprint))),
            (Extra::Analysis(analysis, _, _), "outcomes") => {
                self.inner.serialize_field(key, &Outcomes(analysis))
            }
            (Extra::Analysis(analysis, _, _), "root_terminal_coverage") => {
                if let Some(coverage) = analysis.root_terminal_coverage() {
                    self.inner
                        .serialize_field(key, &Evidence(Extra::TerminalCoverage(coverage)))
                } else {
                    self.inner.serialize_field(key, value)
                }
            }
            (Extra::TerminalCoverage(coverage), "outcomes") => self
                .inner
                .serialize_field(key, &TerminalOutcomes(coverage.outcomes())),
            (Extra::Outcome(outcome), "store") => self
                .inner
                .serialize_field(key, &Evidence(Extra::Store(&outcome.store))),
            (Extra::TerminalOutcome(outcome), "store") => self
                .inner
                .serialize_field(key, &Evidence(Extra::Store(outcome.store()))),
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
            Extra::Analysis(_, _, _)
            | Extra::Outcome(_)
            | Extra::TerminalCoverage(_)
            | Extra::TerminalOutcome(_) => {}
        }
        self.inner.end()
    }
}
impl<'a, S: Serializer, N: WordElement> Serializer for Augment<'a, S, N> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = S::SerializeSeq;
    type SerializeTuple = S::SerializeTuple;
    type SerializeTupleStruct = S::SerializeTupleStruct;
    type SerializeTupleVariant = S::SerializeTupleVariant;
    type SerializeMap = S::SerializeMap;
    type SerializeStruct = Fields<'a, S::SerializeStruct, N>;
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
