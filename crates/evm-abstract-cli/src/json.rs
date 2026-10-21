//! Borrowed application envelopes retain native payloads. Metadata records the
//! command's actual postprocessing ledger; it does not meter serialization.

use crate::{defaults::DEFAULT_POSTPROCESS_WORK, numeric::NumericModel};
use evm_abstract::{
    analysis::{EvmAnalysis, EvmWorldAnalysis},
    domain::WordElement,
    render,
    resource::WorkBudget,
    ssa::NormalizedSsa,
};
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};

#[derive(Serialize)]
struct Metadata {
    numeric_model: NumericModel,
    postprocessing_max_work: usize,
    postprocessing_work: usize,
}

impl Metadata {
    /// Every command constructs one default-policy ledger before projection or
    /// SSA. Capture its consumed prefix only after requested computations finish.
    fn captured(numeric_model: NumericModel, budget: &WorkBudget) -> Self {
        Self {
            numeric_model,
            postprocessing_max_work: DEFAULT_POSTPROCESS_WORK,
            postprocessing_work: budget.used(),
        }
    }
}

#[derive(Serialize)]
struct Report<T> {
    #[serde(flatten)]
    metadata: Metadata,
    #[serde(flatten)]
    native: T,
}

pub(crate) fn world<N: WordElement>(
    analysis: &EvmWorldAnalysis<N>,
    ir: Option<&NormalizedSsa<N>>,
    numeric_model: NumericModel,
    budget: &WorkBudget,
) -> Result<String, render::json::JsonError> {
    let analysis = render::world::evidence(analysis)?;
    let metadata = Metadata::captured(numeric_model, budget);
    match ir {
        Some(ir) => render::json::pretty(&Report {
            metadata,
            native: WorldEnvelope { analysis, ir },
        }),
        None => render::json::pretty(&Report {
            metadata,
            native: analysis,
        }),
    }
}

pub(crate) fn cfg<N: WordElement>(
    analysis: &EvmAnalysis<N>,
    numeric_model: NumericModel,
    budget: &WorkBudget,
) -> Result<String, render::json::JsonError> {
    render::json::pretty(&Report {
        metadata: Metadata::captured(numeric_model, budget),
        native: CfgEnvelope {
            analysis,
            machine_analysis: analysis.execution(),
        },
    })
}

pub(crate) fn program<N: WordElement>(
    analysis: &EvmAnalysis<N>,
    ir: &NormalizedSsa<N>,
    numeric_model: NumericModel,
    budget: &WorkBudget,
) -> Result<String, render::json::JsonError> {
    render::json::pretty(&Report {
        metadata: Metadata::captured(numeric_model, budget),
        native: ProgramEnvelope { analysis, ir },
    })
}

#[derive(Serialize)]
struct CfgEnvelope<'a, N: WordElement> {
    #[serde(flatten)]
    analysis: &'a EvmAnalysis<N>,
    // Native boundary endpoints can lie outside the root-local projection.
    machine_analysis: &'a EvmWorldAnalysis<N>,
}

struct WorldEnvelope<'a, N: WordElement> {
    analysis: render::world::EvmJsonEvidence<'a, N>,
    ir: &'a NormalizedSsa<N>,
}
impl<N: WordElement> Serialize for WorldEnvelope<'_, N> {
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

struct ProgramEnvelope<'a, N: WordElement> {
    analysis: &'a EvmAnalysis<N>,
    ir: &'a NormalizedSsa<N>,
}
impl<N: WordElement> Serialize for ProgramEnvelope<'_, N> {
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

struct StateMapping<'a, N: WordElement>(&'a EvmAnalysis<N>);
#[derive(Serialize)]
struct StateIdentity {
    local_state: usize,
    machine_state: usize,
}
impl<N: WordElement> Serialize for StateMapping<'_, N> {
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
