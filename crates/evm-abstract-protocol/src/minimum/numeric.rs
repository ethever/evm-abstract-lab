//! Canonical numerical payloads. Omitted coordinates use the declared default.

record! {
    /// Numerical scalar at a physical owner.
    pub struct MinimumOwnerValue {
        /// Physical owner, after library alias projection.
        pub owner: String,
        /// Complete supported scalar bound, without an expression pool ID.
        pub value: crate::ValueInfo,
    }
}
record! {
    /// Explicit physical owner/slot bound.
    pub struct MinimumSlot {
        /// Physical storage owner.
        pub owner: String,
        /// Exact slot word.
        pub slot: String,
        /// Canonical slot bound.
        pub value: crate::ValueInfo,
    }
}
record! {
    /// Boolean fact at one owner.
    pub struct MinimumOwnerFlag {
        /// Physical owner.
        pub owner: String,
        /// None permits both values.
        pub value: Option<bool>,
    }
}
record! {
    /// Initial-value dependency override at one explicit slot.
    pub struct MinimumSlotIndependence {
        /// Physical owner.
        pub owner: String,
        /// Exact slot word.
        pub slot: String,
        /// True proves independence from the initial snapshot slot.
        pub independent: bool,
    }
}
record! {
    /// Canonical storage plane, including its original-fact dependency bounds.
    pub struct MinimumPlane {
        /// Bound at all unlisted owners/slots.
        pub default: crate::ValueInfo,
        /// Per-owner defaults, overriding the global default.
        pub owners: Vec<MinimumOwnerValue>,
        /// Explicit cells, overriding the owner/global default.
        pub slots: Vec<MinimumSlot>,
        /// Initial-value independence at unlisted coordinates.
        pub independent_default: bool,
        /// Initial-value independence for unlisted slots at these owners.
        pub independent_owners: Vec<MinimumOwnerFlag>,
        /// Initial-value independence for explicit cells.
        pub independent_slots: Vec<MinimumSlotIndependence>,
    }
}
record! {
    /// Current physical code bound; source bytes use the shared program catalog.
    pub struct MinimumCode {
        /// Physical owner.
        pub owner: String,
        /// Runtime, delegation, empty, or unknown code.
        pub kind: crate::CodeKind,
        /// Exact source catalog row for Runtime.
        pub program: Option<usize>,
        /// Exact implementation pointer for Delegation.
        pub delegation_target: Option<String>,
    }
}
record! {
    /// Presence bound for one owner; absent entries have Unknown presence.
    pub struct MinimumExistence {
        /// Physical owner.
        pub owner: String,
        /// Canonical presence bound.
        pub value: crate::AccountExistence,
    }
}
record! {
    /// One numerical byte cell or full 32-byte word window.
    pub struct MinimumByteCell {
        /// Exact starting offset.
        pub offset: usize,
        /// Complete supported bound.
        pub value: crate::ValueInfo,
    }
}
variant! {
    /// Complete content abstraction; Cells has unknown-byte defaults.
    pub enum MinimumByteContent {
        /// All content bytes are zero.
        Zero,
        /// Complete exact content, encoded as hexadecimal bytes.
        Exact(String),
        /// Retained cell bounds; unlisted cells are unconstrained bytes.
        Cells(Vec<MinimumByteCell>),
    }
}
record! {
    /// Canonical content and independent logical length, without state history.
    pub struct MinimumByteArray {
        /// Full logical length bound.
        pub length: crate::ValueInfo,
        /// Whether EVM memory allocation rules apply.
        pub memory: bool,
        /// Complete content representation.
        pub content: MinimumByteContent,
        /// Additional full-window predicates at exact offsets.
        pub words: Vec<MinimumByteCell>,
    }
}
record! {
    /// Possible event at one exact source; order/multiplicity are abstracted.
    pub struct MinimumLog {
        /// Emitting storage owner.
        pub address: String,
        /// Supplying code account.
        pub code_address: String,
        /// Exact captured source hash.
        pub code_hash: String,
        /// Runtime or initialization source.
        pub source_kind: crate::LogSourceKind,
        /// Source LOG offset.
        pub pc: usize,
        /// Ordered topic bounds from this same event alternative.
        pub topics: Vec<crate::ValueInfo>,
        /// Canonical event data bound.
        pub data: MinimumByteArray,
    }
}
record! {
    /// Complete public store bound after required owner/alias projection.
    pub struct MinimumStore {
        /// Persistent owner/slot bounds.
        pub persistent: MinimumPlane,
        /// Transient owner/slot bounds.
        pub transient: MinimumPlane,
        /// Balance bound for all unlisted owners.
        pub balance_default: crate::ValueInfo,
        /// Explicit balance bounds.
        pub balances: Vec<MinimumOwnerValue>,
        /// Explicit nonce bounds; unlisted nonces are unconstrained words.
        pub nonces: Vec<MinimumOwnerValue>,
        /// Explicit code facts; unlisted code is unknown.
        pub codes: Vec<MinimumCode>,
        /// Explicit presence facts; unlisted presence is Unknown.
        pub existence: Vec<MinimumExistence>,
        /// Creation bound for unlisted owners.
        pub created_default: Option<bool>,
        /// Creation bounds at explicit owners.
        pub created: Vec<MinimumOwnerFlag>,
        /// Pending destruction bound for unlisted owners.
        pub pending_destruction_default: Option<bool>,
        /// Explicit pending destruction bounds.
        pub pending_destruction: Vec<MinimumOwnerFlag>,
        /// Transaction touch bound for unlisted owners.
        pub touched_default: Option<bool>,
        /// Explicit transaction touch bounds.
        pub touched: Vec<MinimumOwnerFlag>,
        /// Possible events indexed by exact captured source.
        pub logs: Vec<MinimumLog>,
        /// Additional events may occur beyond the explicit source list.
        pub logs_unknown: bool,
    }
}
