//! Source-certified sharing over immutable control regions and expression DAGs.
//!
//! A body is a reference into the source code arena, not another copy. Calls
//! carry finite role-checked correspondences; neither state nor computed results
//! are memoized. Normal exits return locally; EVM frame exits and external calls
//! remain in the caller. Internal loops need not terminate. Source admission is
//! independent of candidate discovery.
use crate::{EntityId, Ir, LiftedProgram};
use serde::Serialize;

mod codec;
mod render;
mod source;
pub use render::SharedRenderRow;
pub use source::{SharedRegion, SharedSourceIndex};

/// Independently verified behavior of an extracted source procedure.
///
/// The classification is separate from body shape: pure arithmetic can be a
/// scalar expression DAG or a structured region containing internal loops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedProcedureKind {
    /// Historical pre-v6 overlay. Decode losslessly, but do not use it as a
    /// certificate for the stricter local-return contract.
    #[default]
    Legacy,
    /// Locally returning control region with explicit state dependencies.
    Normal,
    /// Computation depending only on explicit scalar arguments and constants.
    PureArithmetic,
}

impl SharedProcedureKind {
    /// Whether serialization should retain the historical absent-kind spelling.
    pub fn is_legacy(&self) -> bool {
        *self == Self::Legacy
    }
}

strict_record! {
/// One role-preserving source correspondence. Boundary arguments may alias;
/// internal definitions retain distinct identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedBinding {
    /// Canonical entity in the body source domain.
    pub formal: EntityId,
    /// Entity in this exact source occurrence.
    pub actual: EntityId }
}
strict_record! {
/// Simultaneous outgoing transfer and its exact caller continuation block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedContinuation {
    /// Original simultaneous transfer identifying this exit port.
    pub edge: EntityId,
    /// Exact destination source block.
    pub target: EntityId }
}
/// Explicit boundary words and ordered state dependencies of a region.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SharedInterface {
    /// Free logical words captured at region entry. For an expression-rooted
    /// body these are typed leaf expression IDs, including parameterized literals.
    pub inputs: Vec<EntityId>,
    /// Live logical word results exported on normal completion.
    pub outputs: Vec<EntityId>,

    /// Captured source memory-version dependencies.
    pub memory_inputs: Vec<EntityId>,
    /// Live source memory-version results.
    pub memory_outputs: Vec<EntityId>,

    /// Captured ordered observable effect dependencies.
    pub effect_inputs: Vec<EntityId>,
    /// Live ordered observable effect results.
    pub effect_outputs: Vec<EntityId>,

    /// Exact outgoing transfer ports retained in the source body.
    pub continuations: Vec<SharedContinuation>,
}
strict_record! {
/// Reusable executable body anchored in the unchanged source syntax arena.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedProcedure {

    /// Independent artifact-local semantic entity identity.
    pub id: EntityId,
    /// Canonical Structured root or pure Lifted expression root. Both refer to
    /// the unchanged source arena; entity domains are disjoint.
    pub body: EntityId,
    /// Checked semantic contract, independent of the shape of the body.
    #[serde(skip_serializing_if = "SharedProcedureKind::is_legacy")]
    pub kind: SharedProcedureKind = SharedProcedureKind::Legacy,
    /// Checked logical word and state boundary.
    pub interface: SharedInterface,
}
}
/// Exact source occurrence and its independently checked expansion map.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SharedInvocation {
    /// Independent artifact-local semantic entity identity.
    pub id: EntityId,
    /// Definition invoked at this source occurrence.
    pub procedure: EntityId,
    /// Exact Structured or Lifted expression occurrence replaced by this call.
    pub source: EntityId,

    /// Immediate original source syntax owner.
    pub owner: Option<EntityId>,
    /// Child position in the original source owner.
    pub position: usize,

    /// Exact lexical source continuation after this occurrence.
    pub continuation: Option<EntityId>,
    /// Sorted nonidentity substitutions; absent source entities map to themselves.
    /// Expression template inputs map to actual typed leaves, allowing constants
    /// and repeated arguments without collapsing independent source operations.
    pub bindings: Vec<SharedBinding>,
}
/// Hard bound on one certificate, before allocating source correspondence maps.
pub const MAX_SHARED_REGION_ENTITIES: usize = 16_384;
/// Conservative structural region bound; controls aggregate candidate work.
pub const MAX_SHARED_REGION_NODES: usize = 512;

/// The finite retained structure that refused shared-source admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedCapacity {
    /// Aggregate free-value membership slots retained by expression roots.
    ExpressionReadSlots,
    /// Reads, definitions and successors retained by occurrence-flow events.
    FlowEventSlots,
    /// Incoming/outgoing liveness memberships and temporary union capacity.
    FlowLivenessSlots,
    /// Source correspondence domains retained by supplied definitions.
    DefinitionDomains,
    /// Aggregate semantic and shared-interface transport record units.
    ProgramRecords,
}

/// Direct source-certificate rejection, independent of candidate heuristics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SharedProcedureError {
    /// The owner refused work or a hard aggregate bound was reached.
    Limit,
    /// A fixed retained-capacity check refused an optional shared representation.
    Capacity {
        /// Retained structure whose independent hard bound was reached.
        resource: SharedCapacity,
        /// Required retained and temporary units at the refusing checkpoint.
        required: usize,
        /// Maximum admitted units for this structure.
        limit: usize,
    },
    /// Existing high guard, capsule or structural source admission failed.
    BaseSource(crate::LiftedModelError),
    /// An expected source or definition entity is absent.
    Missing(EntityId),
    /// The source shape or lexical escape is unsupported.
    Region(EntityId),
    /// A supplied map changes source or semantic roles.
    Correspondence(EntityId),
    /// Captured or exported words/state differ from source flow.
    Interface(EntityId),
    /// An owner, position or lexical continuation was forged.
    Site(EntityId),
    /// Two admitted omissions would overlap or repeat a source site.
    Overlap(EntityId),
}
impl std::fmt::Display for SharedProcedureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Limit => f.write_str("shared procedure payer refused work"),
            Self::Capacity {
                resource,
                required,
                limit,
            } => write!(
                f,
                "shared procedure {resource:?} capacity refused: required {required}, limit {limit}"
            ),
            Self::BaseSource(error) => write!(f, "invalid base source projection: {error}"),
            Self::Missing(id) => write!(f, "missing shared source entity {id:?}"),
            Self::Region(id) => write!(f, "unsupported or escaping shared region {id:?}"),
            Self::Correspondence(id) => {
                write!(f, "shared source correspondence differs at {id:?}")
            }
            Self::Interface(id) => write!(f, "shared boundary interface differs at {id:?}"),
            Self::Site(id) => {
                write!(
                    f,
                    "shared occurrence ownership or continuation differs at {id:?}"
                )
            }
            Self::Overlap(id) => write!(f, "overlapping or duplicate shared occurrence {id:?}"),
        }
    }
}
impl std::error::Error for SharedProcedureError {}

impl SharedInterface {
    /// History and transport weight, including every binding slot.
    pub fn record_count(&self) -> usize {
        [
            &self.inputs,
            &self.outputs,
            &self.memory_inputs,
            &self.memory_outputs,
            &self.effect_inputs,
            &self.effect_outputs,
        ]
        .into_iter()
        .fold(1_usize, |n, v| n.saturating_add(v.len()))
        .saturating_add(self.continuations.len().saturating_mul(2))
    }
    /// All directly referenced source entities, preserving interface order.
    pub fn references(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.inputs
            .iter()
            .chain(&self.outputs)
            .chain(&self.memory_inputs)
            .chain(&self.memory_outputs)
            .chain(&self.effect_inputs)
            .chain(&self.effect_outputs)
            .copied()
            .chain(self.continuations.iter().flat_map(|c| [c.edge, c.target]))
    }
}
impl SharedInvocation {
    /// Resolve one canonical body entity in this invocation's execution.
    pub fn actual(&self, formal: EntityId) -> Option<EntityId> {
        self.bindings
            .binary_search_by_key(&formal, |b| b.formal)
            .ok()
            .map(|i| self.bindings[i].actual)
            .or(Some(formal))
    }
    /// Source navigation parents; map targets retain noncanonical operations.
    pub fn source_references(&self) -> impl Iterator<Item = EntityId> + '_ {
        [Some(self.source), self.owner, self.continuation]
            .into_iter()
            .flatten()
            .chain(self.bindings.iter().map(|b| b.actual))
    }
}
impl LiftedProgram {
    /// Directly check supplied bodies/maps against immutable source, with no
    /// candidate discovery or canonical-projector reconstruction.
    pub fn validate_shared_sources(
        &self,
        ir: &Ir,
        mut reserve: impl FnMut(usize) -> bool,
    ) -> Result<(), SharedProcedureError> {
        if self.procedures.is_empty() && self.invocations.is_empty() {
            return Ok(());
        }
        if !reserve(self.record_count()) {
            return Err(SharedProcedureError::Limit);
        }
        SharedSourceIndex::for_program(ir, Some(self), &mut reserve)?.validate(self, &mut reserve)
    }
}
impl LiftedProgram {
    pub(crate) fn validate_shared_model(&self) -> Result<(), crate::LiftedModelError> {
        use std::collections::BTreeSet;
        let bad = crate::LiftedModelError::Procedure;
        let mut definitions = BTreeSet::new();
        for p in &self.procedures {
            if p.body.0 == 0
                || !definitions.insert(p.id)
                || p.interface.record_count() > MAX_SHARED_REGION_ENTITIES
            {
                return Err(bad(p.id));
            }
            for list in [
                &p.interface.inputs,
                &p.interface.outputs,
                &p.interface.memory_inputs,
                &p.interface.memory_outputs,
                &p.interface.effect_inputs,
                &p.interface.effect_outputs,
            ] {
                if list.iter().any(|id| id.0 == 0) || list.windows(2).any(|w| w[0] >= w[1]) {
                    return Err(bad(p.id));
                }
            }
            let mut edges = BTreeSet::new();
            for c in &p.interface.continuations {
                if c.edge.0 == 0 || c.target.0 == 0 || !edges.insert(c.edge) {
                    return Err(bad(p.id));
                }
            }
        }
        let mut sites = BTreeSet::new();
        for i in &self.invocations {
            if !definitions.contains(&i.procedure)
                || i.source.0 == 0
                || !sites.insert(i.source)
                || i.owner.is_some_and(|id| id.0 == 0)
                || i.continuation.is_some_and(|id| id.0 == 0)
                || i.position > crate::MAX_STRUCTURED_WIRE_NODES.max(crate::MAX_LIFTED_RECORDS)
                || i.bindings.len() > MAX_SHARED_REGION_ENTITIES
                || i.bindings
                    .iter()
                    .any(|b| b.formal.0 == 0 || b.actual.0 == 0)
                || i.bindings.windows(2).any(|w| w[0].formal >= w[1].formal)
            {
                return Err(bad(i.id));
            }
        }
        Ok(())
    }
}
