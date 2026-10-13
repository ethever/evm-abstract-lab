//! Report-local mixed symbolic DAG. References retain their semantic sort.
//! Dependencies point backwards; logical ranges never expand into byte cells.
record! {
    /// Reference to a Word256 expression within AnalysisReport.symbolic.
    #[derive(std::hash::Hash, Copy)]
    pub struct ExpressionRef {
        /// Index in the owning report node directory.
        pub node: usize,
    }
}
record! {
    /// Reference to a immutable byte state within AnalysisReport.symbolic.
    #[derive(std::hash::Hash, Copy)]
    pub struct ByteStateRef {
        /// Index in the owning report node directory.
        pub node: usize,
    }
}
record! {
    /// Reference to a immutable slot state within AnalysisReport.symbolic.
    #[derive(std::hash::Hash, Copy)]
    pub struct SlotStateRef {
        /// Index in the owning report node directory.
        pub node: usize,
    }
}
record! {
    /// Reference to a bounded byte view within AnalysisReport.symbolic.
    #[derive(std::hash::Hash, Copy)]
    pub struct ByteViewRef {
        /// Index in the owning report node directory.
        pub node: usize,
    }
}
record! {
    /// Reference to a relation path formula within AnalysisReport.symbolic.
    #[derive(std::hash::Hash, Copy)]
    pub struct PathRef {
        /// Index in the owning report node directory.
        pub node: usize,
    }
}
record! {
    /// Immutable source identity, separate from its contents and length.
    #[derive(std::hash::Hash)]
    pub struct SymbolicSource {
        /// Stable identity expression shared by source snapshots.
        pub token: ExpressionRef,
        /// Word256 account, plane or input-role namespace.
        pub namespace: String,
    }
}
record! {
    /// Observed initial cell; unlisted cells keep the source default.
    #[derive(std::hash::Hash)]
    pub struct SymbolicBinding {
        /// Exact Word256 source cell index.
        pub index: String,
        /// Expression bound to this cell.
        pub value: ExpressionRef,
        /// Complete observed numeric facts; no model assignment.
        pub numeric: crate::ValueInfo,
    }
}
record! {
    /// Pure EVM operation in pop-argument order.
    #[derive(std::hash::Hash)]
    pub struct ExpressionOperation {
        /// Raw opcode.
        pub opcode: u8,
        /// Earlier scalar operands in EVM pop order.
        pub arguments: Vec<ExpressionRef>,
    }
}
record! {
    /// EVM nonzero selects the first paired word.
    #[derive(std::hash::Hash)]
    pub struct ExpressionIte {
        /// Nonzero branch selector.
        pub condition: ExpressionRef,
        /// Word selected by nonzero.
        pub then_value: ExpressionRef,
        /// Word selected by zero.
        pub else_value: ExpressionRef,
    }
}
record! {
    /// Byte read, zero extended to Word256.
    #[derive(std::hash::Hash)]
    pub struct ByteRead {
        /// Immutable backing state.
        pub state: ByteStateRef,
        /// Mathematical nonwrapping byte index.
        pub index: ExpressionRef,
    }
}
record! {
    /// Big-endian word read through a zero-padded bounded view.
    #[derive(std::hash::Hash)]
    pub struct WordRead {
        /// Captured finite view.
        pub view: ByteViewRef,
        /// Relative byte offset.
        pub offset: ExpressionRef,
    }
}
record! {
    /// Slot read bound to an immutable account and storage plane.
    #[derive(std::hash::Hash)]
    pub struct SlotRead {
        /// Captured slot state.
        pub state: SlotStateRef,
        /// Word256 key.
        pub key: ExpressionRef,
    }
}
record! {
    /// Immutable finite byte source with complete observations.
    #[derive(std::hash::Hash)]
    pub struct BytesInput {
        /// Source identity.
        pub source: SymbolicSource,
        /// Logical length.
        pub length: ExpressionRef,
        /// Fixed observed cells, including non-singleton facts.
        pub bindings: Vec<SymbolicBinding>,
    }
}
record! {
    /// Ordered byte or big-endian word update.
    #[derive(std::hash::Hash)]
    pub struct ByteWrite {
        /// State before this update.
        pub previous: ByteStateRef,
        /// First byte index.
        pub offset: ExpressionRef,
        /// Written word expression.
        pub value: ExpressionRef,
    }
}
record! {
    /// One lazy range update regardless of logical range size.
    #[derive(std::hash::Hash)]
    pub struct ByteCopy {
        /// Destination before the update.
        pub previous: ByteStateRef,
        /// First destination byte index.
        pub destination: ExpressionRef,
        /// Captured immutable source view.
        pub source: ByteViewRef,
        /// Requested logical copy length.
        pub length: ExpressionRef,
    }
}
record! {
    /// One guard selects the paired byte-state alternative.
    #[derive(std::hash::Hash)]
    pub struct BytesIte {
        /// Nonzero branch selector.
        pub condition: ExpressionRef,
        /// State selected by nonzero.
        pub then_state: ByteStateRef,
        /// State selected by zero.
        pub else_state: ByteStateRef,
    }
}
record! {
    /// Fixed storage source and its complete observations.
    #[derive(std::hash::Hash)]
    pub struct SlotsInput {
        /// Account and plane identity.
        pub source: SymbolicSource,
        /// Fixed observed cells, including non-singleton facts.
        pub bindings: Vec<SymbolicBinding>,
    }
}
record! {
    /// Ordered storage update, including symbolic keys.
    #[derive(std::hash::Hash)]
    pub struct SlotWrite {
        /// State before the update.
        pub previous: SlotStateRef,
        /// Word256 key.
        pub key: ExpressionRef,
        /// Written value.
        pub value: ExpressionRef,
    }
}
record! {
    /// One guard selects the paired storage-state alternative.
    #[derive(std::hash::Hash)]
    pub struct SlotsIte {
        /// Nonzero branch selector.
        pub condition: ExpressionRef,
        /// State selected by nonzero.
        pub then_state: SlotStateRef,
        /// State selected by zero.
        pub else_state: SlotStateRef,
    }
}
record! {
    /// Bounded zero-padded view over an immutable backing state.
    #[derive(std::hash::Hash)]
    pub struct ByteView {
        /// Captured backing state.
        pub state: ByteStateRef,
        /// First backing byte index.
        pub offset: ExpressionRef,
        /// Logical length including any zero padding.
        pub length: ExpressionRef,
    }
}
record! {
    /// Retained zero or nonzero guarantee.
    #[derive(std::hash::Hash)]
    pub struct PathTruth {
        /// Constrained word.
        pub expression: ExpressionRef,
        /// True requires nonzero; false requires zero.
        pub nonzero: bool,
    }
}
record! {
    /// Inclusive bounds in unsigned or sign-bit-flipped order.
    #[derive(std::hash::Hash)]
    pub struct PathRange {
        /// Constrained word.
        pub expression: ExpressionRef,
        /// Lower Word256 endpoint.
        pub lower: String,
        /// Upper Word256 endpoint.
        pub upper: String,
    }
}
record! {
    /// Retained unsigned congruence.
    #[derive(std::hash::Hash)]
    pub struct PathModulo {
        /// Constrained word.
        pub expression: ExpressionRef,
        /// Positive Word256 modulus.
        pub modulus: String,
        /// Canonical residue.
        pub residue: String,
    }
}
record! {
    /// Retained bit guarantees.
    #[derive(std::hash::Hash)]
    pub struct PathBits {
        /// Constrained word.
        pub expression: ExpressionRef,
        /// Bits guaranteed zero.
        pub zero: String,
        /// Bits guaranteed one.
        pub one: String,
    }
}
record! {
    /// Retained word equality.
    #[derive(std::hash::Hash)]
    pub struct PathEquality {
        /// First word.
        pub left: ExpressionRef,
        /// Second word.
        pub right: ExpressionRef,
    }
}
variant! {
    /// Typed retained relation guarantee.
    #[derive(std::hash::Hash)]
    pub enum PathConstraint {
        /// Zero or nonzero.
        Truth(PathTruth),
        /// Unsigned bounds.
        Range(PathRange),
        /// Bounds after XOR with bit 255.
        SignedRange(PathRange),
        /// Unsigned congruence.
        Modulo(PathModulo),
        /// Bit guarantees.
        Bits(PathBits),
        /// Word equality.
        Equal(PathEquality),
        /// Membership in a complete finite set.
        Members(PathMembers),
    }
}
record! {
    /// Guarded union retaining complete path alternatives.
    #[derive(std::hash::Hash)]
    pub struct PathIte {
        /// Shared nonzero selector.
        pub selector: ExpressionRef,
        /// Formula selected by nonzero.
        pub then_path: PathRef,
        /// Formula selected by zero.
        pub else_path: PathRef,
    }
}
variant! {
    /// Flat mixed symbolic node; every child precedes this node.
    #[derive(std::hash::Hash)]
    pub enum Expression {
        /// Exact word.
        Constant(String),
        /// Immutable scoped environment input.
        Input(crate::InputIdentity),
        /// Independent identity named only within this report.
        Fresh(u64),
        /// Pure EVM operation.
        Operation(ExpressionOperation),
        /// Paired word alternatives.
        Ite(ExpressionIte),
        /// Immutable byte read.
        ByteRead(ByteRead),
        /// Immutable big-endian word read.
        WordRead(WordRead),
        /// Immutable slot read.
        SlotRead(SlotRead),
        /// Hash of the complete bounded content.
        Keccak(ByteViewRef),
        /// Unbounded zero backing bytes.
        BytesZero,
        /// Finite immutable source.
        BytesInput(BytesInput),
        /// Exact bounded bytes encoded as hexadecimal.
        BytesExact(String),
        /// Low eight-bit update.
        WriteByte(ByteWrite),
        /// Big-endian word update.
        WriteWord(ByteWrite),
        /// Lazy captured range copy.
        Copy(ByteCopy),
        /// Zero-based state over a bounded view.
        Slice(ByteViewRef),
        /// Guarded byte-state alternatives.
        BytesIte(BytesIte),
        /// Known zero initial slots.
        SlotsZero,
        /// Fixed source observations.
        SlotsInput(SlotsInput),
        /// Ordered slot write.
        Store(SlotWrite),
        /// Guarded slot-state alternatives.
        SlotsIte(SlotsIte),
        /// Bounded zero-padded byte sequence.
        View(ByteView),
        /// Unconstrained path.
        PathTrue,
        /// Proved impossible path.
        PathFalse,
        /// Conjunction of retained guarantees.
        PathAtoms(Vec<PathConstraint>),
        /// Conjunction of shared formulas.
        PathAnd(Vec<PathRef>),
        /// Guarded union of complete formulas.
        PathIte(PathIte),
    }
}
record! {
    /// One dependency-ordered directory shared by all report consumers.
    #[derive(std::hash::Hash)]
    pub struct SymbolicGraph {
        /// Scalar, byte, slot, view and path nodes.
        pub nodes: Vec<Expression>,
    }
}
record! {
    /// Report-export policy, independent of execution and logical byte ranges.
    #[derive(std::hash::Hash, Copy)]
    pub struct ExportLimits {
        /// Maximum actual retained DAG nodes.
        pub max_nodes: usize,
        /// Maximum dependency depth.
        pub max_depth: usize,
        /// Maximum exact external byte payload.
        pub max_exact_bytes: usize,
        /// Maximum cumulative traversal and pool-intern work.
        pub max_work: usize,
        /// Maximum complete encoded HTTP report bytes, including numeric facts.
        pub max_output_bytes: usize,
    }
}
record! {
    /// Measured report projection work; no logical-range byte expansion.
    #[derive(std::hash::Hash, Copy)]
    pub struct ExportUsage {
        /// Actual retained mixed DAG nodes.
        pub nodes: usize,
        /// Actual exact byte payload.
        pub exact_bytes: usize,
        /// Charged traversals, scalar facts and interning candidates.
        pub work: usize,
    }
}
choice! {
    /// Export rejection category, independent of analysis convergence.
    pub enum ExportFailureKind {
        /// Actual node count bound.
        Nodes,
        /// Dependency depth bound.
        Depth,
        /// Exact byte payload bound.
        ExactBytes,
        /// Cumulative projection work bound.
        Work,
        /// Complete JSON encoding exceeded its declared byte policy.
        OutputBytes,
        /// Cooperative cancellation.
        Cancelled,
        /// A native state root could not be recovered.
        StateRoot,
    }
}
record! {
    /// Finite diagnostic metadata for a rejected projection.
    pub struct ExportFailure {
        /// Exact rejected resource.
        pub kind: ExportFailureKind,
        /// Required amount at the rejection point.
        pub required: usize,
        /// Frozen export policy.
        pub limits: ExportLimits,
        /// Work retained before rejection.
        pub usage: ExportUsage,
        /// Encoded bytes accepted before an output/cancellation failure.
        pub written: Option<usize>,
    }
}
record! {
    /// Original checked EVM range operands.
    pub struct RangeRequest {
        /// Mathematical bound being checked.
        pub kind: RangeKind,
        /// Original Word256 byte offset.
        pub offset: crate::ValueInfo,
        /// Original requested size, including zero.
        pub size: crate::ValueInfo,
        /// Logical memory bound or complete returndata length.
        pub limit: crate::ValueInfo,
    }
}
choice! {
    /// Distinguishes resource policy from returndata semantics.
    pub enum RangeKind {
        /// Declared word-rounded logical memory footprint.
        MemoryPolicy,
        /// Complete returned sequence length.
        ReturnData,
    }
}
record! {
    /// Unexpanded inputs before this instruction applies its effect.
    pub struct RangeWitness {
        /// Original access, without enumeration.
        pub request: RangeRequest,
        /// Nonzero satisfies the mathematical bound.
        pub condition: crate::ValueInfo,
        /// Selected side; None retains the entire pre-effect input.
        pub nonzero: Option<bool>,
        /// Index in AnalysisReport.pending_machines.
        pub payload: usize,
    }
}

record! {
    /// Retained complete finite set.
    #[derive(std::hash::Hash)]
    pub struct PathMembers {
        /// Constrained word.
        pub expression: ExpressionRef,
        /// All permitted Word256 values.
        pub values: Vec<String>,
    }
}

choice! {
    /// Semantic sort of one node in the mixed graph.
    #[derive(std::hash::Hash)]
    pub enum SymbolicSort {
        /// Word256 scalar.
        Word,
        /// Immutable byte state.
        Bytes,
        /// Immutable slot state.
        Slots,
        /// Bounded byte view.
        View,
        /// Boolean relation formula.
        Path,
    }
}
record! {
    /// Typed dependency used for validation and bounded inspection.
    #[derive(Copy, std::hash::Hash)]
    pub struct SymbolicReference {
        /// Semantic sort required by this edge.
        pub sort: SymbolicSort,
        /// Index in the report directory.
        pub node: usize,
    }
}
record! {
    /// Invalid reference detected before a report is used by the UI.
    pub struct SymbolicReferenceFailure {
        /// Parent node; None denotes a snapshot or value root.
        pub owner: Option<usize>,
        /// Rejected directory index.
        pub node: usize,
        /// Required sort.
        pub expected: SymbolicSort,
        /// Actual sort, or None for an unavailable node.
        pub actual: Option<SymbolicSort>,
        /// Whether the dependency-order rule also failed.
        pub forward: bool,
    }
}
impl std::fmt::Display for SymbolicReferenceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "symbolic reference {} expects {:?}, found {:?}, owner {:?}, forward={}",
            self.node, self.expected, self.actual, self.owner, self.forward
        )
    }
}
impl std::error::Error for SymbolicReferenceFailure {}

mod references;
mod report;
choice! {
    /// Shared report directory containing a rejected reference.
    pub enum PoolReferenceKind {
        /// Captured byte arrays.
        ByteArray,
        /// Complete scalar byte facts.
        ByteValue,
        /// Transaction or rollback stores.
        Store,
        /// Captured programs.
        Program,
        /// Pending pre-effect or incoming machines.
        PendingMachine,
        /// Native machine-state identity.
        MachineState,
        /// A recursive relation in its containing abstract summary.
        AbstractProcedure,
    }
}
record! {
    /// Rejected reference into a finite report-local directory.
    pub struct PoolReferenceFailure {
        /// Directory kind.
        pub kind: PoolReferenceKind,
        /// Rejected index.
        pub index: usize,
        /// Actual directory size.
        pub length: usize,
    }
}
variant! {
    /// Compatibility failure before consuming a report.
    pub enum ReportValidationFailure {
        /// Unsupported schema version.
        Schema(u16),
        /// Mixed graph sort or dependency-order mismatch.
        Symbolic(SymbolicReferenceFailure),
        /// Missing pool entry.
        Pool(PoolReferenceFailure),
    }
}
impl std::fmt::Display for ReportValidationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Schema(version) => write!(
                f,
                "report schema {version} is unsupported; expected {}",
                crate::SCHEMA_VERSION
            ),
            Self::Symbolic(failure) => write!(f, "{failure}"),
            Self::Pool(failure) => write!(
                f,
                "{:?} reference {} exceeds pool length {}",
                failure.kind, failure.index, failure.length
            ),
        }
    }
}
impl std::error::Error for ReportValidationFailure {}

record! {
    /// Complete scalar facts bound to an expression within its owning snapshot.
    pub struct SymbolicFact {
        /// Immutable word expression to which these facts apply.
        pub expression: ExpressionRef,
        /// Complete numeric, provenance and identity guarantees.
        pub value: crate::ValueInfo,
    }
}
