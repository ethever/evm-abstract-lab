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
// Retain the strict local record codec while deriving core node field types
// from the same schema used by native kinds, structural maps and validation.
macro_rules! wire_record_definition {
    (record $name:ident, [$(#[$meta:meta])*], { $($field:ident: $role:ident => $wire:ident),* }) => {
        record! {
            $(#[$meta])*
            #[derive(std::hash::Hash)]
            pub struct $name {
                $(#[doc = stringify!($wire)] pub $wire: symbolic_structure::$role<structure::Wire>),*
            }
        }
    };
    (reuse $name:ident, $meta:tt, $fields:tt) => {};
    (tuple $name:ident, $meta:tt, $fields:tt) => {};
}
macro_rules! wire_records {
    ($($sort:ident $native:ident {
        units $units:tt
        prefix { $($(#[$pm:meta])* $prefix:ident => $pflat:ident ($($pw:tt)*) $pfields:tt)* }
        leaves $leaves:tt
        records { $($(#[$meta:meta])* $record:ident => $flat:ident ($($wire:tt)*) $fields:tt)* }
    })*) => {
        $($(wire_record_definition!($($pw)*, [$(#[$pm])*], $pfields);)*)*
        $($(wire_record_definition!($($wire)*, [$(#[$meta])*], $fields);)*)*
    };
}
symbolic_structure::with_nodes!(wire_records);
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
        /// Maximum retained DAG nodes, or scalar rows in the minimum profile.
        pub max_nodes: usize,
        /// Maximum dependency depth.
        pub max_depth: usize,
        /// Maximum exact external byte payload.
        pub max_exact_bytes: usize,
    }
}
record! {
    /// Measured projection usage; the minimum profile also pays work from the
    /// independent persistent application ledger.
    #[derive(std::hash::Hash, Copy)]
    pub struct ExportUsage {
        /// Actual retained DAG nodes, or scalar rows in the minimum profile.
        pub nodes: usize,
        /// Actual exact byte payload.
        pub exact_bytes: usize,
        /// Saturating diagnostic count, independent of the browser's pointer width.
        pub work: u64,
    }
}
choice! {
    /// Export rejection category, independent of analysis convergence.
    pub enum ExportFailureKind {
        /// Mandatory application projection could not be funded.
        Work,
        /// This scalar's custom predicates have no exact public representation.
        Numeric,
        /// Actual node count bound.
        Nodes,
        /// Dependency depth bound.
        Depth,
        /// Exact byte payload bound.
        ExactBytes,
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
        /// Required amount at rejection; cancellation records projection work
        /// or accepted JSON bytes when `written` is present.
        /// A Work refusal uses zero when its native enclosing operation does
        /// not expose the rejected quote.
        pub required: u64,
        /// Frozen export policy.
        pub limits: ExportLimits,
        /// Work retained before rejection.
        pub usage: ExportUsage,
        /// Encoded bytes accepted before cancellation during report encoding.
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
        /// Historical memory policy, representable logical end, or complete returndata length.
        pub limit: crate::ValueInfo,
    }
}
choice! {
    /// Distinguishes logical representability, host policy, and returndata semantics.
    pub enum RangeKind {
        /// Historical configured word-rounded memory footprint policy.
        MemoryPolicy,
        /// Nonempty access requires a representable, word-rounded U256 end.
        /// Its limit is U256::MAX & !31, independent of host materialization policy.
        LogicalMemory,
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
pub mod structure;
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
        /// A body in the containing summary's regional table.
        RegionalNode,
        /// A caller continuation in the containing summary's regional table.
        RegionalCall,
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
record! {
    /// Invalid reported postprocessing allowance or usage.
    pub struct PostprocessingWorkFailure {
        /// Reported application allowance, which must be positive.
        pub max_work: u64,
        /// Reported usage, which must not exceed the allowance.
        pub work: u64,
    }
}
variant! {
    /// Compatibility failure before consuming a report.
    pub enum ReportValidationFailure {
        /// Inconsistent mandatory numerical/control publication.
        Minimum(crate::MinimumValidationKind),
        /// Inconsistent immutable catalog or source-qualified reference.
        Source(crate::SourceValidationKind),
        /// Inconsistent Hybrid namespace, source, receipt or coverage facts.
        Hybrid(crate::HybridValidationKind),
        /// Process identity, private-state or constructor representation failure.
        Process(crate::ProcessValidationKind),
        /// Unsupported schema version.
        Schema(u16),
        /// Incoherent application postprocessing allowance and usage.
        Postprocessing(PostprocessingWorkFailure),
        /// Mixed graph sort or dependency-order mismatch.
        Symbolic(SymbolicReferenceFailure),
        /// Missing pool entry.
        Pool(PoolReferenceFailure),
        /// Inconsistent independent terminal relation or original input binding.
        RootTerminal(crate::RootTerminalValidationFailure),
        /// Inconsistent regional identities, relationships or currentness.
        Regional(crate::RegionalValidationFailure),
    }
}
impl std::fmt::Display for ReportValidationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Minimum(kind) => write!(f, "invalid minimum model: {kind:?}"),
            Self::Source(kind) => write!(f, "invalid source reference: {kind:?}"),
            Self::Hybrid(kind) => write!(f, "invalid hybrid evidence: {kind:?}"),
            Self::Process(kind) => write!(f, "invalid process evidence: {kind:?}"),
            Self::Schema(version) => write!(
                f,
                "report schema {version} is unsupported; expected {}",
                crate::SCHEMA_VERSION
            ),
            Self::Postprocessing(failure) => write!(
                f,
                "postprocessing work {} requires a positive allowance of at least that amount; reported {}",
                failure.work, failure.max_work
            ),
            Self::Symbolic(failure) => write!(f, "{failure}"),
            Self::Pool(failure) => write!(
                f,
                "{:?} reference {} exceeds pool length {}",
                failure.kind, failure.index, failure.length
            ),
            Self::RootTerminal(failure) => write!(f, "root terminal binding {failure:?}"),
            Self::Regional(failure) => write!(f, "regional evidence {failure:?}"),
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
