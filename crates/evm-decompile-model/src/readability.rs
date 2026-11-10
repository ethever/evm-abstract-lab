//! Reading preferences are separate from semantic admission and execution limits.
//! Region anchors identify deterministic recovered inputs, never transient AST IDs.

use crate::{ArtifactId, Config, EntityId, Input};
use serde::{Deserialize, Serialize};

/// Relative priorities for selecting a verified presentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadabilityProfile {
    /// Keep natural control while avoiding substantial repeated output.
    #[default]
    Balanced,
    /// Favor a continuous mainline, shallow control and visible state effects.
    Linear,
    /// Favor shared representations of repeated computation and control.
    Compact,
}

/// Whether optional source-certified procedure extraction is desired.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedurePreference {
    /// Compare the reading cost of definitions and their call sites.
    #[default]
    Auto,
    /// Retain source occurrences without introducing optional procedures.
    Inline,
    /// Prefer useful, source-certified shared bodies.
    Shared,
}

/// A preference cannot authorize a semantically invalid or unrepresentable plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructurePreference {
    /// Use the selected reading profile.
    #[default]
    Auto,
    /// Prefer ordinary conditionals and loops when admissible.
    Natural,
    /// Prefer an explicit local state machine for this exact source region.
    Dispatcher,
}

strict_record! {
/// Recovery identity and a complete relative region boundary.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegionAnchor {
    /// Input/fork/recovery-policy identity, independent of reading preferences.
    pub source: ArtifactId,
    /// Recovered source block, not a materialized syntax occurrence.
    pub entry: EntityId,
    /// The source continuation remains outside this region.
    pub continuation: Option<EntityId>,
    /// Active source loop context, if any.
    pub loop_header: Option<EntityId>,
}
}
strict_record! {
/// One explicit structural preference for a complete source region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionPreference {
    /// Exact source region to which the choice applies.
    pub region: RegionAnchor,
    /// Desired representation, subject to semantic and resource admission.
    pub structure: StructurePreference,
}
}
strict_record! {
/// Frozen per-attempt preferences; local choices override only their own region.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadabilityConfig {
    /// Default tradeoffs for structural choices.
    pub profile: ReadabilityProfile,
    /// Policy for optional procedure extraction.
    pub procedures: ProcedurePreference,
    /// Explicit choices for regions from a matching recovery source.
    pub regions: Vec<RegionPreference>,
}
}
impl ReadabilityConfig {
    /// Whether omitting this optional field preserves its meaning.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

strict_record! {
/// Structural reading proxies, not measurements of human understanding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadabilityMetrics {
    /// Source-operation occurrences in the candidate, including stack plumbing.
    pub operations: u64,
    /// Conditional decisions in the represented control structure.
    pub conditions: u64,
    /// Estimated control depth, discounting proved failure continuations and
    /// excluding Sequence wrappers; this is not a measured rendered line depth.
    pub max_nesting: u64,
    /// Scalar parameters at the source region entry.
    pub entry_values: u64,
    /// Operation occurrences beyond their unique source occurrences.
    pub duplicated_operations: u64,
    /// Source cases retained inside dispatchers, including repeated occurrences.
    pub dispatcher_blocks: u64,
    /// Explicit dispatcher transitions, including repeated occurrences.
    pub dispatcher_transitions: u64,
    /// Expanded syntax-node occurrences.
    pub syntax_nodes: u64,
}
}

/// The root representation of a selected source region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructureForm {
    /// An ordinary structured root, possibly containing local dispatchers.
    Natural,
    /// A dispatcher at this region's root.
    Dispatcher,
}

/// Why the chosen representation was retained. This is not an optimality proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadabilityReason {
    /// An admissible explicit region preference was used.
    UserPreference,
    /// Existing natural control was retained.
    NaturalControl,
    /// Reading proxies favored this candidate.
    LowerReadingCost,
    /// Residual cyclic control requires explicit dispatch in this plan.
    MultiEntryControl,
    /// Natural representation would repeat substantial source content.
    Expansion,
    /// Bounded planning retained a compact source representation.
    PlanningFallback,
    /// A compact representation avoids excessive recursive structuring.
    DepthFallback,
    /// The requested choice was unavailable; a valid alternative was retained.
    PreferenceFallback,
}

strict_record! {
/// A retained choice with actual candidate costs; absent costs mean not measured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionDecision {
    /// Source and control boundary of the retained choice.
    pub region: RegionAnchor,
    /// Effective explicit preference, or automatic selection.
    pub preference: StructurePreference,
    /// Root form in the published plan.
    pub selected: StructureForm,
    /// Selection rationale, independent of semantic validation.
    pub reason: ReadabilityReason,
    /// Measured natural candidate, if constructed.
    pub natural: Option<ReadabilityMetrics>,
    /// Measured compact candidate, if considered.
    pub dispatcher: Option<ReadabilityMetrics>,
}
}

/// Bind preferences to the same recovery policies while allowing their own change.
/// This does not certify a region; the planner must still match its full boundary.
pub fn readability_source(input: &Input, config: &Config) -> ArtifactId {
    versioned_source(input, config, crate::SCHEMA_VERSION)
}

impl crate::Artifact {
    /// Source namespace of this stored publication. Existing schema-7 reading
    /// decisions retain their original hash; new attempts use the current free
    /// function and must not transplant older region identities by relabeling.
    pub fn readability_source(&self) -> ArtifactId {
        versioned_source(&self.input, &self.config, self.schema_version)
    }
}

fn versioned_source(input: &Input, config: &Config, schema: u32) -> ArtifactId {
    let mut bytes = input.fork.to_string().into_bytes();
    bytes.push(0);
    bytes.extend_from_slice(&input.runtime_bytecode);
    let mut policy = format!(
        "recovery-schema={schema};strategy={:?};limits={:?}",
        config.strategy, config.limits
    );
    if schema >= 8 {
        policy.push_str(&format!(";execution={:?}", config.execution_domain));
    }
    bytes.extend_from_slice(policy.as_bytes());
    ArtifactId(format!("{:x}", alloy_primitives::keccak256(bytes)))
}
