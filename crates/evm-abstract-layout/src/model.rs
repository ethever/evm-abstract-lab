//! Typed ownership boundary between layout and an interactive renderer.

use std::collections::BTreeMap;

use crate::{Point, Rect, Size};

/// A displayed leaf; its ID remains the caller's original identity.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// Original display/native state ID, never an arena offset.
    pub id: usize,
    /// Actual high-detail card dimensions.
    pub size: Size,
    /// Captured program, when known.
    pub program: Option<usize>,
    /// Program-local basic block.
    pub block: usize,
    /// Frame depth; spatial chains do not hide call-frame boundaries.
    pub frame_depth: Option<usize>,
    /// A frontier or hidden incident edge prevents asserting a closed chain.
    pub boundary: bool,
}

/// Control-flow edge kind, retained independently of geometric routing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// Sequential flow.
    Fallthrough,
    /// Unconditional jump.
    Jump,
    /// True branch.
    BranchTrue,
    /// False branch.
    BranchFalse,
    /// Enter another call frame.
    Call,
    /// Return to a caller.
    Return,
    /// Exceptional failure.
    Failure,
    /// Reverted call.
    Revert,
}

impl EdgeKind {
    pub(crate) const fn local_flow(self) -> bool {
        matches!(
            self,
            Self::Fallthrough | Self::Jump | Self::BranchTrue | Self::BranchFalse
        )
    }
}

/// Text measured by the caller's actual font system.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// Canonical text, including original visible edge identities.
    pub text: String,
    /// Actual text bounds with a small routing clearance.
    pub size: Size,
}

/// One original edge; parallel edges are separate entries.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    /// Original edge identity.
    pub id: usize,
    /// Original source leaf ID.
    pub from: usize,
    /// Original target leaf ID.
    pub to: usize,
    /// Original control-flow kind.
    pub kind: EdgeKind,
    /// Measured edge annotation.
    pub label: Label,
}

/// Complete owned input for a single synchronous layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Input {
    /// Leaves, in a stable caller order.
    pub nodes: Vec<Node>,
    /// All visible original edges; never rewritten to group endpoints.
    pub edges: Vec<Edge>,
}

/// Identity of a spatial group, disjoint from native node and edge IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroupId(pub usize);

/// Why displayed leaves belong to a spatial group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    /// Nodes captured from the same executable program.
    Program(usize),
    /// A maximal ordinary chain in the displayed graph.
    Chain,
    /// A strongly connected region in one program.
    Cycle,
}

/// A spatial container; leaves and their SSA identities remain separate.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// Internal spatial ID.
    pub id: GroupId,
    /// Optional enclosing program group.
    pub parent: Option<GroupId>,
    /// Grouping rule.
    pub kind: GroupKind,
    /// Original member leaf IDs.
    pub members: Vec<usize>,
}

/// The full ordered polyline and label of one original edge.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeGeometry {
    /// Original source leaf.
    pub from: usize,
    /// Original target leaf.
    pub to: usize,
    /// Original kind.
    pub kind: EdgeKind,
    /// Complete source-to-target path, including cross-hierarchy sections.
    pub path: Vec<Point>,
    /// Jointly laid out annotation rectangle.
    pub label: Rect,
}

/// Bounds and caption of a spatial container.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupGeometry {
    /// Group membership and hierarchy.
    pub group: Group,
    /// World-space outer rectangle.
    pub bounds: Rect,
    /// Caption rectangle, kept clear of leaf cards and routed edges.
    pub caption: Rect,
    /// Short annotation caption.
    pub title: String,
}

/// Immutable geometry; pan, zoom and viewport are exclusively renderer state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// Original leaf IDs to their world-space rectangles.
    pub nodes: BTreeMap<usize, Rect>,
    /// Original edge IDs to complete routes.
    pub edges: BTreeMap<usize, EdgeGeometry>,
    /// Containers in parent-before-child order.
    pub groups: Vec<GroupGeometry>,
    /// Entire content, including labels and groups; absent for an empty graph.
    pub bounds: Option<Rect>,
}

/// Invalid inputs or a failed layout cannot silently create a different CFG.
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    /// Repeated original node identity.
    #[error("duplicate node ID {0}")]
    DuplicateNode(usize),
    /// Repeated original edge identity.
    #[error("duplicate edge ID {0}")]
    DuplicateEdge(usize),
    /// An edge refers to a leaf absent from the input.
    #[error("edge {edge} refers to missing node {node}")]
    MissingNode {
        /// Original edge ID.
        edge: usize,
        /// Missing endpoint ID.
        node: usize,
    },
    /// Measured dimensions must be finite and positive.
    #[error("invalid dimensions for {element} {id}")]
    InvalidSize {
        /// Card or edge label.
        element: &'static str,
        /// Original identity.
        id: usize,
    },
    /// ELK's upstream API currently supplies a string diagnostic.
    #[error("layout engine failed: {message}")]
    Engine {
        /// Original upstream diagnostic, retained without inventing geometry.
        message: String,
    },
    /// A result is missing or invalid for an original identity.
    #[error("invalid {element} geometry for {id}: {reason}")]
    Geometry {
        /// Node, edge, label or group.
        element: &'static str,
        /// Original/spatial identity.
        id: usize,
        /// Specific failed output invariant.
        reason: &'static str,
    },
    /// ELK arenas use 32-bit indices independently of original IDs.
    #[error("layout input is too large for the engine arenas")]
    TooLarge,
}
