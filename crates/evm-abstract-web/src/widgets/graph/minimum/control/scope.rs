//! Presentation scopes retain regional identities. A source coordinate is never
//! an executed PC, and a grouped card never becomes a new analysis state.

use evm_abstract_protocol::{
    DecimalU64, EdgeKind, ProcessAction, ProcessLocation, SummarySourceStatus,
};

pub(crate) const DEFAULT_POINT_LIMIT: usize = 12;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GraphLevel {
    #[default]
    Source,
    AbstractBoundary,
    Model,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EdgeLevel {
    Source,
    CallBoundary,
    ExceptionBoundary,
    AbstractBoundary,
    Model,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ControlOrigin {
    /// Actual regional table ID, not its position or a Native state ID.
    pub node: usize,
    /// None selects the Body; Some selects one retained MAY source coordinate.
    pub location: Option<ProcessLocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionTarget {
    pub summary: usize,
    pub procedure: usize,
    pub origin: ControlOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GraphScope {
    Entry {
        summary: usize,
    },
    Context {
        summary: usize,
        procedure: usize,
        anchor: Option<ControlOrigin>,
    },
    /// Explicit regional overview, without expanding every repeated source copy.
    Overview {
        summary: usize,
    },
}

impl GraphScope {
    pub(crate) fn summary(self) -> usize {
        match self {
            Self::Entry { summary }
            | Self::Context { summary, .. }
            | Self::Overview { summary } => summary,
        }
    }
}

impl ProjectionTarget {
    pub(crate) fn scope(self) -> GraphScope {
        GraphScope::Context {
            summary: self.summary,
            procedure: self.procedure,
            anchor: Some(self.origin),
        }
    }
}

/// The shell maps these to its single qualified selection/navigation owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GraphIntent {
    Select(ProjectionTarget),
    Open(ProjectionTarget),
    OpenSource {
        summary: usize,
        program: usize,
        location: ProcessLocation,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CardKind {
    Source {
        program: Option<usize>,
        location: ProcessLocation,
    },
    Receipt,
    Calls {
        action: Option<ProcessAction>,
    },
    Folded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OriginEvidence {
    pub target: ProjectionTarget,
    pub input_revision: DecimalU64,
    /// True only for this receipt and its certified containing source.
    pub current: bool,
    pub complete: bool,
    pub excluded: bool,
    /// A boundary obligation does not supply the boundary's effects.
    pub boundary: bool,
    pub through: bool,
    /// Actual Body transfer at this coordinate, independently of completeness.
    pub transferred: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlowOrigin {
    /// Accumulated admission membership without a retained transfer category.
    Topology {
        from: usize,
        to: usize,
    },
    /// Target-list membership only; this variant never authorizes a graph edge.
    Admission {
        node: usize,
    },
    /// Decoded directory membership without an expanded unknown-jump receipt.
    DecodedJump {
        relation: usize,
        target: usize,
    },
    Source {
        node: usize,
        relation: usize,
        from: ProcessLocation,
        to: ProcessLocation,
    },
    Regional {
        from: usize,
        to: usize,
    },
    CallSite {
        call: usize,
    },
    Call {
        call: usize,
        entry: usize,
    },
    Delivery {
        call: usize,
        delivery: usize,
        successor: usize,
    },
    Jump {
        node: usize,
        relation: usize,
        target: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    Control(ProjectionTarget),
    /// Directory-only candidates deliberately carry no fabricated regional ID.
    Source {
        program: usize,
        location: ProcessLocation,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TargetEntry {
    pub destination: Destination,
    pub via: FlowOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TargetSetKind {
    Calls { calls: Vec<usize> },
    JumpTargets { relation: usize },
    Folded,
    ContextBoundary,
    ModelRelations,
}

/// Regional relations remain inspectable independently of source adjacency.
/// An absent category is untyped topology, never an inferred fallthrough edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelRelation {
    pub from: ProjectionTarget,
    pub to: ProjectionTarget,
    pub kind: Option<EdgeKind>,
    pub origin: FlowOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TargetSet {
    pub kind: TargetSetKind,
    /// Each row keeps its call/receipt association; this is not a tuple union.
    pub entries: Vec<TargetEntry>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectionCard {
    pub kind: CardKind,
    pub origins: Vec<OriginEvidence>,
    pub target_set: Option<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectionEdge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
    pub level: EdgeLevel,
    pub origins: Vec<FlowOrigin>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScopeCounts {
    pub total_receipts: usize,
    pub context_receipts: usize,
    pub displayed_receipts: usize,
    pub hidden_receipts: usize,
    pub other_context_receipts: usize,
    pub total_points: usize,
    pub displayed_points: usize,
    pub hidden_points: usize,
    pub unexpanded_points: usize,
    pub displayed_unexpanded_points: usize,
    pub hidden_flows: usize,
    pub calls: usize,
    pub displayed_calls: usize,
    pub deliveries: usize,
    pub summarized_model_relations: usize,
    pub retained_topology: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct GraphProjection {
    pub scope: GraphScope,
    pub level: GraphLevel,
    pub source_status: SummarySourceStatus,
    pub cards: Vec<ProjectionCard>,
    pub edges: Vec<ProjectionEdge>,
    pub target_sets: Vec<TargetSet>,
    pub counts: ScopeCounts,
    /// Display card containing the exact requested entry/anchor.
    pub anchor: Option<usize>,
    /// Complete omitted-source membership, including disconnected observations.
    pub hidden_set: Option<usize>,
    pub model_relations: Vec<ModelRelation>,
    pub model_set: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ProjectionError {
    #[error("No canonical control model is attached")]
    MissingModel,
    #[error("This control observation is unavailable")]
    MissingSummary,
    #[error("This procedure context is unavailable")]
    MissingContext,
    #[error("This receipt or MAY coordinate does not belong to the requested context")]
    InvalidAnchor,
    #[error("The control directory does not match this source and epoch")]
    SourceMismatch,
}
