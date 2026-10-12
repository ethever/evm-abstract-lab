//! Borrowed JSON envelopes keep native export streaming and cancellable.
use evm_abstract::{
    analysis::{Analysis, WorldAnalysis},
    render,
    ssa::NormalizedSsa,
};
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};

pub(crate) fn world(
    analysis: &WorldAnalysis,
    ir: Option<&NormalizedSsa>,
) -> Result<String, render::json::JsonError> {
    let analysis = render::world::evidence(analysis)?;
    match ir {
        Some(ir) => render::json::pretty(&WorldEnvelope { analysis, ir }),
        None => render::json::pretty(&analysis),
    }
}

pub(crate) fn program(
    analysis: &Analysis,
    ir: &NormalizedSsa,
) -> Result<String, render::json::JsonError> {
    render::json::pretty(&ProgramEnvelope { analysis, ir })
}

struct WorldEnvelope<'a> {
    analysis: render::world::JsonEvidence<'a>,
    ir: &'a NormalizedSsa,
}
impl Serialize for WorldEnvelope<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("analysis", &self.analysis)?;
        map.serialize_entry(
            if self.ir.complete() {
                "ssa"
            } else {
                "partial_ssa"
            },
            self.ir,
        )?;
        map.end()
    }
}

struct ProgramEnvelope<'a> {
    analysis: &'a Analysis,
    ir: &'a NormalizedSsa,
}
impl Serialize for ProgramEnvelope<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("analysis", self.analysis)?;
        map.serialize_entry("machine_analysis", self.analysis.execution())?;
        map.serialize_entry("state_mapping", &StateMapping(self.analysis))?;
        map.serialize_entry(
            if self.ir.complete() {
                "ssa"
            } else {
                "partial_ssa"
            },
            self.ir,
        )?;
        map.end()
    }
}

struct StateMapping<'a>(&'a Analysis);
#[derive(Serialize)]
struct StateIdentity {
    local_state: usize,
    machine_state: usize,
}
impl Serialize for StateMapping<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.states().len()))?;
        for state in self.0.states() {
            sequence.serialize_element(&StateIdentity {
                local_state: state.id,
                machine_state: self
                    .0
                    .execution_state_id(state.id)
                    .expect("each projected state retains its machine identity"),
            })?;
        }
        sequence.end()
    }
}
