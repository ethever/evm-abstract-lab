//! Typed initial-input session failures, independent of execution or RPC transport.

record! {
    /// Captured code identity, independent of account state.
    pub struct SessionCodeIdentity {
        /// Execution rules used for decoding.
        pub fork: crate::Fork,
        /// Runtime or initialization mode.
        pub kind: crate::CodeKind,
        /// Complete captured bytecode hash.
        pub hash: String,
    }
}
record! {
    /// Program rules disagree with the fixed code catalog.
    pub struct CatalogForkMismatch {
        /// Catalog rules.
        pub catalog: crate::Fork,
        /// Rejected program rules.
        pub program: crate::Fork,
    }
}
record! {
    /// Exact anchored snapshot coordinates.
    pub struct SessionChainSnapshot {
        /// Complete 256-bit chain ID.
        pub chain_id: String,
        /// Pinned block hash.
        pub block_hash: String,
    }
}
variant! {
    /// Public snapshot description. Equal offline labels never establish identity.
    pub enum SessionSnapshotDescription {
        /// Descriptive offline label only; it must never be used as an input binding.
        Offline(String),
        /// Immutable chain snapshot.
        Chain(SessionChainSnapshot),
    }
}
record! {
    /// Distinct initial-state identities; descriptions may have equal offline labels.
    pub struct SessionSnapshotMismatch {
        /// Session snapshot description.
        pub expected: SessionSnapshotDescription,
        /// Rejected fact batch description.
        pub observed: SessionSnapshotDescription,
    }
}
variant! {
    /// Exact initial-state location whose observation was contradicted.
    pub enum InitialFactLocation {
        /// Account code.
        Code(String),
        /// Account balance.
        Balance(String),
        /// Account nonce.
        Nonce(String),
        /// Account presence.
        Existence(String),
        /// Persistent storage owner and full-width key.
        Storage(crate::StorageLocation),
        /// Complete initial storage map.
        StorageComplete(String),
    }
}
variant! {
    /// Rejected session inputs; cancellation/configuration/world use existing causes.
    pub enum SessionFailure {
        /// Mixed execution rules in a catalog.
        CatalogFork(CatalogForkMismatch),
        /// Distinct bytecode contents shared a catalog hash identity.
        CatalogHashCollision(SessionCodeIdentity),
        /// Fact batch belongs to a distinct initial snapshot.
        Snapshot(SessionSnapshotMismatch),
        /// An immutable initial observation was contradicted.
        FactConflict(InitialFactLocation),
        /// Referenced code was not supplied.
        MissingCode(SessionCodeIdentity),
        /// Initialization code was bound as installed runtime.
        InitcodeBinding(SessionCodeIdentity),
        /// Entry source contradicts this account's observed code.
        EntryCode(String),
        /// Adapter seed did not match the declared empty initial snapshot/catalog.
        Seed,
    }
}
