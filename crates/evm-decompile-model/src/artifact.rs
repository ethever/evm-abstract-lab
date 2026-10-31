//! Immutable stages, complete occurrence origins and transactional adoption.

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Overall result; structural strategy does not determine recovery completeness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion { Complete, Partial, Cancelled, Limited, Failed }
/// Independently inspectable immutable phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageKind { Decoded, Baseline, Simplified, ControlFlow, Structured }
/// One owned historical snapshot. Failed candidates never replace current.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    /// Stable artifact-local stage ID.
    pub id: StageId,
    /// Prior adopted stage, if present.
    pub parent: Option<StageId>,
    /// Phase boundary.
    pub kind: StageKind,
    /// Complete semantic representation at this boundary.
    pub ir: Ir,
    /// Actual invariant-check results.
    pub validation: Validation,
}
/// Whether advertised invariant checks were executed and passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus { Passed, Failed, NotRun }
/// An individual executable invariant check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationCheck {
    /// Stable check name.
    pub name: String,
    /// Actual predicate result.
    pub passed: bool,
    /// Concrete explanation or failure context.
    pub message: String,
}
/// Validation is evidence about specified invariants, not a universal proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validation {
    /// Aggregate result.
    pub status: ValidationStatus,
    /// Checks run at this snapshot.
    pub checks: Vec<ValidationCheck>,
}
/// Diagnostic severity, independent of partial recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity { Info, Warning, Error }
/// Stable diagnostic classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    Decode, UnknownJump, InvalidJump, StackUnderflow, StackOverflow,
    StackHeightMismatch, Limit, Cancelled, Validation, Unsupported,
}
/// Located, machine-inspectable explanation retained on partial results.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Severity for presentation.
    pub severity: Severity,
    /// Typed reason.
    pub code: DiagnosticCode,
    /// Relevant immutable stage.
    pub stage: Option<StageId>,
    /// Relevant entity, if known.
    pub entity: Option<EntityId>,
    /// Original instruction span, never a fabricated PC.
    pub source: Option<SourceSpan>,
    /// Human-readable details.
    pub message: String,
}
/// One real entity copy, with independent IDs for operations/results/edges too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneRelation {
    /// Original occurrence.
    pub original: EntityRef,
    /// New copied occurrence.
    pub copy: EntityRef,
}
/// A control-flow redirection, including the old and new destinations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeRedirect {
    /// Identity of the changed edge.
    pub edge: EntityId,
    /// Previous target.
    pub before: EntityId,
    /// New target.
    pub after: EntityId,
}
/// Complete record of a phase rewrite and its validation outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformEvent {
    /// Artifact-local event identity.
    pub id: u64,
    /// Previous adopted snapshot.
    pub from_stage: Option<StageId>,
    /// Candidate snapshot, including failed candidates.
    pub to_stage: StageId,
    /// Machine-stable rule name.
    pub rule: String,
    /// Why this rewrite was introduced.
    pub reason: String,
    /// Consumed/consulted prior occurrences.
    pub inputs: Vec<EntityRef>,
    /// Produced candidate occurrences.
    pub outputs: Vec<EntityRef>,
    /// Independently allocated entities.
    pub created: Vec<EntityId>,
    /// Removed entities; their historical origins remain inspectable.
    pub deleted: Vec<EntityId>,
    /// Actual copies across the stage boundary.
    pub clones: Vec<CloneRelation>,
    /// All redirected retained edges.
    pub redirects: Vec<EdgeRedirect>,
    /// Candidate validation, including non-adoption on failure.
    pub validation: Validation,
}
/// One entity occurrence's ancestry, materialized by core at transformation time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    /// Entity occurrence.
    pub entity: EntityRef,
    /// Original spans reached by this occurrence's ancestry.
    pub source: Vec<SourceSpan>,
    /// Prior immutable occurrences; no synthetic PC is invented.
    pub parents: Vec<EntityRef>,
    /// Responsible event.
    pub introduced_by: u64,
    /// Specific introduction or carry-forward rule.
    pub reason: String,
}
/// Materialized bidirectional ancestry database; helpers never parse text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMappings { /// Origins for every retained historical occurrence.
    pub origins: Vec<Origin>,
}
/// Differences of actual entities, not rendered strings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageDiff {
    /// IDs newly introduced.
    pub added: Vec<EntityId>,
    /// IDs absent from the candidate.
    pub removed: Vec<EntityId>,
    /// Retained IDs whose typed contents changed.
    pub changed: Vec<EntityId>,
}
/// Work and final retained sizes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statistics {
    /// Successful logical-work reservation.
    pub work: usize,
    /// Number of stable block transfer evaluations.
    pub evaluations: usize,
    /// Retained blocks in current IR.
    pub blocks: usize,
    /// Retained instructions in current IR.
    pub operations: usize,
    /// Retained value identities.
    pub values: usize,
    /// Retained known edges.
    pub edges: usize,
    /// Real copied blocks, excluding synthetic dispatcher controls.
    pub cloned_blocks: usize,
}
/// Complete offline result, including interrupted and failed attempts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Wire schema version.
    pub schema_version: u32,
    /// Bytecode and fork identity namespace.
    pub id: ArtifactId,
    /// Captured original input.
    pub input: Input,
    /// Captured frozen configuration.
    pub config: Config,
    /// Recovery and transformation outcome.
    pub completion: Completion,
    /// All diagnosed partial paths/refusals.
    pub diagnostics: Vec<Diagnostic>,
    /// Owned immutable historical stages.
    pub stages: Vec<Stage>,
    /// Complete transformation history.
    pub events: Vec<TransformEvent>,
    /// Core-owned forward/reverse origins.
    pub mappings: SourceMappings,
    /// Last validated adopted stage.
    pub current_stage: Option<StageId>,
    /// Actual paid work and retained sizes.
    pub statistics: Statistics,
}
impl Artifact {
    /// Retrieve a historical snapshot.
    pub fn stage(&self, id: StageId) -> Option<&Stage> { self.stages.iter().find(|s| s.id == id) }
    /// Last validated snapshot, which survives later transform failures.
    pub fn current(&self) -> Option<&Stage> { self.current_stage.and_then(|id| self.stage(id)) }
    /// Trace a stage entity to exact original spans and parents.
    pub fn origin(&self, entity: EntityRef) -> Option<&Origin> {
        self.mappings.origins.iter().find(|o| o.entity == entity)
    }
    /// Every occurrence derived from an original PC, including copied entities.
    pub fn descendants(&self, source_pc: usize) -> Vec<EntityRef> {
        self.mappings.origins.iter().filter(|o| o.source.iter().any(|s| s.pc <= source_pc && source_pc < s.end_pc)).map(|o| o.entity).collect()
    }
    /// All later occurrences transitively descended from an entity occurrence.
    pub fn derived_from(&self, original: EntityRef) -> Vec<EntityRef> {
        let mut found = BTreeSet::from([original]);
        for origin in &self.mappings.origins {
            if origin.parents.iter().any(|p| found.contains(p)) { found.insert(origin.entity); }
        }
        found.remove(&original);
        found.into_iter().collect()
    }
    /// Compare typed snapshots before and after a transformation.
    pub fn diff(&self, before: StageId, after: StageId) -> Option<StageDiff> {
        Some(diff_ir(&self.stage(before)?.ir, &self.stage(after)?.ir))
    }
}

#[derive(PartialEq, Eq)]
#[doc(hidden)]
pub enum Record<'a> {
    Block(&'a Block), Operation(&'a Operation), Value(&'a Value), Edge(&'a Edge),
    Node(&'a StructuredNode), Transition(&'a DispatchTransition),
}
#[doc(hidden)]
pub fn records(ir: &Ir) -> BTreeMap<EntityId, Record<'_>> {
    let mut map = BTreeMap::new();
    for b in &ir.blocks {
        map.insert(b.id, Record::Block(b));
        for o in &b.operations { map.insert(o.id, Record::Operation(o)); }
    }
    for v in &ir.values { map.insert(v.id, Record::Value(v)); }
    for e in &ir.edges { map.insert(e.id, Record::Edge(e)); }
    if let Some(s) = &ir.structured { node_records(&s.root, &mut map); }
    map
}
fn node_records<'a>(node: &'a StructuredNode, map: &mut BTreeMap<EntityId, Record<'a>>) {
    map.insert(node.id, Record::Node(node));
    match &node.kind {
        StructuredKind::Sequence(nodes) => for n in nodes { node_records(n, map); },
        StructuredKind::If { then_branch, else_branch, .. } => { node_records(then_branch, map); node_records(else_branch, map); }
        StructuredKind::While { header, body, .. } => { node_records(header, map); node_records(body, map); }
        StructuredKind::Indirect { arms, .. } => for a in arms { node_records(&a.body, map); },
        StructuredKind::Dispatcher { cases, .. } => for c in cases { for t in &c.transitions { map.insert(t.id, Record::Transition(t)); } },
        _ => {},
    }
}
#[doc(hidden)]
pub fn diff_ir(old: &Ir, new: &Ir) -> StageDiff {
    let old = records(old); let new = records(new);
    StageDiff {
        added: new.keys().filter(|id| !old.contains_key(id)).copied().collect(),
        removed: old.keys().filter(|id| !new.contains_key(id)).copied().collect(),
        changed: new.iter().filter(|(id,r)| old.get(id).is_some_and(|o| o != *r)).map(|(id,_)| *id).collect(),
    }
}

