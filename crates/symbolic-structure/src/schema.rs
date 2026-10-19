//! The structural contract shared by native kinds, flat exports and wire nodes.

/// Apply a consumer to the mixed symbolic node schema. Native variant names,
/// wire record fields, edge sorts and their order are declared together here.
/// Payload policy and smart construction deliberately belong to consumers.
/// Groups are emitted in declaration order: units, prefix records, tuple leaves,
/// then records. BytesInput must precede BytesExact to retain native serde and
/// hash discriminants; this ordering is also used by flat graph serialization.
#[macro_export]
macro_rules! with_nodes {
    ($consumer:ident) => {
        $consumer! {
            Word ExprKind {
                units {}
                prefix {}
                leaves {
                    /// Exact Word256 literal.
                    Constant => Constant: Constant [];
                    /// Scoped immutable input.
                    Input => Input: Input [];
                    /// Runtime leaf identity within its producer namespace.
                    Fresh => Fresh: Fresh [#[serde(skip)]];
                }
                records {
                    /// Pure EVM operation in pop-argument order.
                    Operation => Operation (record ExpressionOperation) {
                        opcode: Opcode => opcode, args: Words => arguments
                    }
                    /// Nonzero condition selects the first word.
                    Ite => Ite (record ExpressionIte) {
                        condition: Word => condition, then_value: Word => then_value,
                        else_value: Word => else_value
                    }
                    /// Byte read at a mathematical nonwrapping index, zero extended to Word256.
                    ByteRead => ByteRead (record ByteRead) {
                        state: Bytes => state, index: Word => index
                    }
                    /// Big-endian word read through a bounded zero-padded view. Offsets and
                    /// the 32-byte range use mathematical nonwrapping addresses.
                    WordRead => WordRead (record WordRead) {
                        view: View => view, offset: Word => offset
                    }
                    /// Read an immutable storage snapshot.
                    SlotRead => SlotRead (record SlotRead) {
                        state: Slots => state, key: Word => key
                    }
                    /// Hash the complete bounded byte sequence.
                    Keccak => Keccak (tuple input) { input: View => input }
                }
            }
            Bytes ByteStateKind {
                units {
                    /// Unbounded zero backing bytes.
                    Zero => BytesZero;
                }
                prefix {
                    /// Finite immutable source with observed initial cells.
                    Input => BytesInput (record BytesInput) {
                        source: Source => source, length: Word => length,
                        bindings: Bindings => bindings
                    }
                }
                leaves {
                    /// Concrete finite bytes, padded with zero.
                    Exact => BytesExact: Exact [];
                }
                records {
                    /// Ordered low-eight-bit update at a mathematical nonwrapping byte offset.
                    WriteByte => WriteByte (record ByteWrite) {
                        previous: Bytes => previous, offset: Word => offset, value: Word => value
                    }
                    /// Ordered big-endian 32-byte update; its offset and entire range must
                    /// fit mathematical nonwrapping byte addresses.
                    WriteWord => WriteWord (reuse ByteWrite) {
                        previous: Bytes => previous, offset: Word => offset, value: Word => value
                    }
                    /// Lazy range copy from a captured immutable view. Destination and length
                    /// describe a nonwrapping byte range; source padding stays bounded.
                    Copy => Copy (record ByteCopy) {
                        previous: Bytes => previous, destination: Word => destination,
                        source: View => source, length: Word => length
                    }
                    /// Zero-based backing state preserving a view boundary.
                    Slice => Slice (tuple source) { source: View => source }
                    /// Nonzero condition selects the first byte snapshot.
                    Ite => BytesIte (record BytesIte) {
                        condition: Word => condition, then_state: Bytes => then_state,
                        else_state: Bytes => else_state
                    }
                }
            }
            Slots SlotStateKind {
                units {
                    /// Known zero initial storage.
                    Zero => SlotsZero;
                }
                prefix {}
                leaves {}
                records {
                    /// Fixed storage source with observed initial cells.
                    Input => SlotsInput (record SlotsInput) {
                        source: Source => source, bindings: Bindings => bindings
                    }
                    /// Ordered Word256 slot update. Keys are EVM words, distinct from the
                    /// mathematical nonwrapping addresses of byte-state operations.
                    Store => Store (record SlotWrite) {
                        previous: Slots => previous, key: Word => key, value: Word => value
                    }
                    /// Nonzero condition selects the first storage snapshot.
                    Ite => SlotsIte (record SlotsIte) {
                        condition: Word => condition, then_state: Slots => then_state,
                        else_state: Slots => else_state
                    }
                }
            }
            View ByteView {
                units {}
                prefix {}
                leaves {}
                records {
                    /// Bounded zero-padded sequence over an immutable backing state.
                    View => View (record ByteView) {
                        state: Bytes => state, offset: Word => offset, length: Word => length
                    }
                }
            }
        }
    };
}

/// Borrow a native/wire field without copying payloads or numeric facts.
#[doc(hidden)]
#[macro_export]
macro_rules! borrow_field {
    (Words, $value:expr) => {
        $value.iter()
    };
    (Bindings, $value:expr) => {
        $value.iter()
    };
    (Opcode, $value:expr) => {
        *$value
    };
    (Fresh, $value:expr) => {
        *$value
    };
    ($role:ident, $value:expr) => {
        $value
    };
}
