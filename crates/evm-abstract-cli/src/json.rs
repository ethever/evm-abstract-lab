//! Borrowed application envelopes retain native payloads. Metadata records the
//! command's actual postprocessing ledger; it does not meter serialization.

use crate::{defaults::DEFAULT_POSTPROCESS_WORK, numeric::NumericModel};
use evm_abstract::{
    analysis::{self, EvmAnalysis, EvmWorldAnalysis},
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

/// The application status is independent of the selected graph's raw status.
/// Stream both graphs once and explicitly retain current native prefix receipts.
pub(crate) fn hybrid<N: WordElement>(
    source: &analysis::HybridAnalysis<N>,
    ir: Option<&NormalizedSsa<N>>,
    detail_ir: Option<&NormalizedSsa<N>>,
    numeric_model: NumericModel,
    budget: &WorkBudget,
) -> Result<String, render::json::JsonError> {
    let native_chosen = source.coverage().chosen == analysis::HybridInterpretation::Native;
    render::json::pretty(&Report {
        metadata: Metadata::captured(numeric_model, budget),
        native: HybridEnvelope {
            schema_version: analysis::SCHEMA_VERSION,
            selected_view: if native_chosen {
                evm_abstract_protocol::AnalysisNamespace::Native
            } else {
                evm_abstract_protocol::AnalysisNamespace::Baseline
            },
            status: source.status(),
            work: source.work(),
            transfers: source.transfers(),
            global: NativeGraph {
                analysis: render::world::evidence(source.baseline())?,
                ir: None,
            },
            native: source
                .detail()
                .map(|graph| {
                    Ok::<_, render::json::JsonError>(NativeGraph {
                        analysis: render::world::evidence(graph)?,
                        ir: if native_chosen { ir } else { detail_ir },
                    })
                })
                .transpose()?,
            hybrid: HybridEvidence {
                coverage: source.coverage(),
                local_detail: local_detail(source),
                precision: None,
                rpc: None,
            },
        },
    })
}

pub(crate) fn rpc_hybrid<N: WordElement>(
    source: &analysis::RpcHybridAnalysis<N>,
    ir: Option<&NormalizedSsa<N>>,
    detail_ir: Option<&NormalizedSsa<N>>,
    numeric_model: NumericModel,
    budget: &WorkBudget,
) -> Result<String, render::json::JsonError> {
    render::json::pretty(&Report {
        metadata: Metadata::captured(numeric_model, budget),
        native: HybridEnvelope {
            schema_version: analysis::SCHEMA_VERSION,
            selected_view: evm_abstract_protocol::AnalysisNamespace::Baseline,
            status: source.status(),
            work: source.work(),
            transfers: source.coverage().transfers,
            global: NativeGraph {
                analysis: render::world::evidence(source.baseline().baseline())?,
                ir,
            },
            native: source
                .detail()
                .map(|graph| {
                    Ok::<_, render::json::JsonError>(NativeGraph {
                        analysis: render::world::evidence(graph)?,
                        ir: detail_ir,
                    })
                })
                .transpose()?,
            hybrid: HybridEvidence {
                coverage: source.baseline().coverage(),
                local_detail: source.detail_report().map(local_detail).unwrap_or_default(),
                precision: source
                    .detail_report()
                    .map(analysis::HybridAnalysis::coverage),
                rpc: Some(RpcEvidence {
                    coverage: source.coverage(),
                    failures: source
                        .failures()
                        .iter()
                        .map(|error| error.failure())
                        .collect(),
                    preparation_failure: source.preparation_failure().map(ToString::to_string),
                }),
            },
        },
    })
}

fn local_detail<N: WordElement>(source: &analysis::HybridAnalysis<N>) -> Vec<LocalPrefix<'_, N>> {
    source
        .local_detail()
        .map(|prefix| {
            let frontiers = source
                .detail()
                .into_iter()
                .flat_map(|detail| detail.frontiers().iter().enumerate())
                .filter(|(_, frontier)| frontier.from == Some(prefix.state_id))
                .map(|(index, _)| index)
                .collect();
            LocalPrefix {
                namespace: "Native",
                prefix,
                frontiers,
            }
        })
        .collect()
}

#[derive(Serialize)]
struct RpcEvidence<'a> {
    #[serde(flatten)]
    coverage: &'a analysis::RpcHybridCoverage,
    failures: Vec<evm_abstract::world::rpc::Failure>,
    /// Native precision outcome is typed; this supplements its preparation cause.
    preparation_failure: Option<String>,
}

#[derive(Serialize)]
struct LocalPrefix<'a, N: WordElement> {
    namespace: &'static str,
    #[serde(flatten)]
    prefix: analysis::HybridLocalDetail<'a, N>,
    /// Native.frontiers indexes, independent of the process graph namespace.
    frontiers: Vec<usize>,
}

#[derive(Serialize)]
struct HybridEvidence<'a, N: WordElement> {
    coverage: &'a analysis::HybridCoverage,
    local_detail: Vec<LocalPrefix<'a, N>>,
    precision: Option<&'a analysis::HybridCoverage>,
    rpc: Option<RpcEvidence<'a>>,
}

#[derive(Serialize)]
struct HybridEnvelope<'a, N: WordElement> {
    schema_version: u16,
    selected_view: evm_abstract_protocol::AnalysisNamespace,
    status: analysis::Status,
    work: usize,
    transfers: usize,
    global: NativeGraph<'a, N>,
    native: Option<NativeGraph<'a, N>>,
    hybrid: HybridEvidence<'a, N>,
}

// Native evidence remains its own intentional format. A graph has the same
// placement whether SSA is requested and whether the chosen view changes.
struct NativeGraph<'a, N: WordElement> {
    analysis: render::world::EvmJsonEvidence<'a, N>,
    ir: Option<&'a NormalizedSsa<N>>,
}
impl<N: WordElement> Serialize for NativeGraph<'_, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1 + usize::from(self.ir.is_some())))?;
        map.serialize_entry("analysis", &self.analysis)?;
        if let Some(ir) = self.ir {
            map.serialize_entry(if ir.complete() { "ssa" } else { "partial_ssa" }, ir)?;
        }
        map.end()
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
