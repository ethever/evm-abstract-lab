//! Freeze serde's complete enum metadata, which JSON alone cannot observe.
use super::*;
use crate::domain::{
    NumericValue,
    symbolic::{
        ByteStateKind, ExprKind, InputAtom, SlotStateKind, StateBinding, SymbolicGraphNode,
    },
};
use serde::ser::{Impossible, SerializeStructVariant, SerializeTupleVariant};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Unit,
    Newtype,
    Tuple(usize),
    Struct(Vec<&'static str>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Metadata {
    name: &'static str,
    index: u32,
    variant: &'static str,
    kind: Kind,
}
#[derive(Debug)]
struct Unsupported;
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("expected enum variant")
    }
}
impl std::error::Error for Unsupported {}
impl serde::ser::Error for Unsupported {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Self
    }
}
struct Capture;
struct Compound {
    metadata: Metadata,
    expected: usize,
    seen: usize,
}
macro_rules! reject_scalars {
    ($($method:ident($ty:ty)),*) => { $(
        fn $method(self, _: $ty) -> Result<Metadata, Unsupported> { Err(Unsupported) }
    )* };
}
impl serde::Serializer for Capture {
    type Ok = Metadata;
    type Error = Unsupported;
    type SerializeSeq = Impossible<Metadata, Unsupported>;
    type SerializeTuple = Impossible<Metadata, Unsupported>;
    type SerializeTupleStruct = Impossible<Metadata, Unsupported>;
    type SerializeTupleVariant = Compound;
    type SerializeMap = Impossible<Metadata, Unsupported>;
    type SerializeStruct = Impossible<Metadata, Unsupported>;
    type SerializeStructVariant = Compound;
    reject_scalars!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8])
    );
    fn serialize_none(self) -> Result<Metadata, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, _: &T) -> Result<Metadata, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_unit(self) -> Result<Metadata, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Metadata, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        _: &T,
    ) -> Result<Metadata, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStruct, Unsupported> {
        Err(Unsupported)
    }
    fn serialize_unit_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
    ) -> Result<Metadata, Unsupported> {
        Ok(Metadata {
            name,
            index,
            variant,
            kind: Kind::Unit,
        })
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        _: &T,
    ) -> Result<Metadata, Unsupported> {
        Ok(Metadata {
            name,
            index,
            variant,
            kind: Kind::Newtype,
        })
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Compound, Unsupported> {
        Ok(Compound {
            metadata: Metadata {
                name,
                index,
                variant,
                kind: Kind::Tuple(len),
            },
            expected: len,
            seen: 0,
        })
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Compound, Unsupported> {
        Ok(Compound {
            metadata: Metadata {
                name,
                index,
                variant,
                kind: Kind::Struct(Vec::new()),
            },
            expected: len,
            seen: 0,
        })
    }
}
impl SerializeTupleVariant for Compound {
    type Ok = Metadata;
    type Error = Unsupported;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, _: &T) -> Result<(), Unsupported> {
        self.seen += 1;
        Ok(())
    }
    fn end(self) -> Result<Metadata, Unsupported> {
        assert_eq!(self.seen, self.expected);
        Ok(self.metadata)
    }
}
impl SerializeStructVariant for Compound {
    type Ok = Metadata;
    type Error = Unsupported;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        _: &T,
    ) -> Result<(), Unsupported> {
        let Kind::Struct(fields) = &mut self.metadata.kind else {
            unreachable!()
        };
        fields.push(key);
        self.seen += 1;
        Ok(())
    }
    fn end(self) -> Result<Metadata, Unsupported> {
        assert_eq!(self.seen, self.expected);
        Ok(self.metadata)
    }
}
fn assert_variants<T: Serialize>(
    name: &'static str,
    values: &[T],
    expected: &[(&'static str, Kind)],
) {
    assert_eq!(values.len(), expected.len());
    for (index, (value, (variant, kind))) in values.iter().zip(expected).enumerate() {
        assert_eq!(
            value.serialize(Capture).unwrap(),
            Metadata {
                name,
                index: index as u32,
                variant,
                kind: kind.clone()
            }
        );
    }
}
fn record(fields: &[&'static str]) -> Kind {
    Kind::Struct(fields.to_vec())
}

#[test]
fn native_and_flat_enum_names_ordinals_fields_and_fresh_payloads_are_stable() {
    let word = ExprId::fresh().unwrap();
    let bytes = ByteStateId::zero();
    let slots = SlotStateId::zero();
    let view = ByteView::new(bytes.clone(), word.clone(), word.clone());
    let atom = InputAtom {
        scope: u64::MAX,
        symbol: Symbol::Caller,
    };
    let source = SourceId::scoped(word.clone(), U256::from(9));
    let words = vec![
        ExprKind::Constant(U256::ZERO),
        ExprKind::Input(atom),
        ExprKind::Fresh(123),
        ExprKind::Operation {
            opcode: opcode::ADD,
            args: vec![word.clone(), word.clone()],
        },
        ExprKind::Ite {
            condition: word.clone(),
            then_value: word.clone(),
            else_value: word.clone(),
        },
        ExprKind::ByteRead {
            state: bytes.clone(),
            index: word.clone(),
        },
        ExprKind::WordRead {
            view: view.clone(),
            offset: word.clone(),
        },
        ExprKind::SlotRead {
            state: slots.clone(),
            key: word.clone(),
        },
        ExprKind::Keccak {
            input: view.clone(),
        },
    ];
    let word_metadata = vec![
        ("Constant", Kind::Newtype),
        ("Input", Kind::Newtype),
        ("Fresh", Kind::Unit),
        ("Operation", record(&["opcode", "args"])),
        ("Ite", record(&["condition", "then_value", "else_value"])),
        ("ByteRead", record(&["state", "index"])),
        ("WordRead", record(&["view", "offset"])),
        ("SlotRead", record(&["state", "key"])),
        ("Keccak", record(&["input"])),
    ];
    assert_variants("ExprKind", &words, &word_metadata);
    let byte_kinds = vec![
        ByteStateKind::Zero,
        ByteStateKind::Input {
            source: source.clone(),
            length: word.clone(),
            bindings: vec![],
        },
        ByteStateKind::Exact(Arc::from([1])),
        ByteStateKind::WriteByte {
            previous: bytes.clone(),
            offset: word.clone(),
            value: word.clone(),
        },
        ByteStateKind::WriteWord {
            previous: bytes.clone(),
            offset: word.clone(),
            value: word.clone(),
        },
        ByteStateKind::Copy {
            previous: bytes.clone(),
            destination: word.clone(),
            source: view.clone(),
            length: word.clone(),
        },
        ByteStateKind::Slice {
            source: view.clone(),
        },
        ByteStateKind::Ite {
            condition: word.clone(),
            then_state: bytes.clone(),
            else_state: bytes,
        },
    ];
    let byte_metadata = vec![
        ("Zero", Kind::Unit),
        ("Input", record(&["source", "length", "bindings"])),
        ("Exact", Kind::Newtype),
        ("WriteByte", record(&["previous", "offset", "value"])),
        ("WriteWord", record(&["previous", "offset", "value"])),
        (
            "Copy",
            record(&["previous", "destination", "source", "length"]),
        ),
        ("Slice", record(&["source"])),
        ("Ite", record(&["condition", "then_state", "else_state"])),
    ];
    assert_variants("ByteStateKind", &byte_kinds, &byte_metadata);
    let slot_kinds = vec![
        SlotStateKind::Zero,
        SlotStateKind::Input {
            source,
            bindings: vec![],
        },
        SlotStateKind::Store {
            previous: slots.clone(),
            key: word.clone(),
            value: word.clone(),
        },
        SlotStateKind::Ite {
            condition: word.clone(),
            then_state: slots.clone(),
            else_state: slots,
        },
    ];
    let slot_metadata = vec![
        ("Zero", Kind::Unit),
        ("Input", record(&["source", "bindings"])),
        ("Store", record(&["previous", "key", "value"])),
        ("Ite", record(&["condition", "then_state", "else_state"])),
    ];
    assert_variants("SlotStateKind", &slot_kinds, &slot_metadata);
    let graph_source = super::super::GraphSource {
        token: 0,
        namespace: U256::from(9),
    };
    let flat = vec![
        SymbolicGraphNode::Constant(U256::ZERO),
        SymbolicGraphNode::Input(atom),
        SymbolicGraphNode::Fresh(123),
        SymbolicGraphNode::Operation {
            opcode: opcode::ADD,
            args: vec![0, 1],
        },
        SymbolicGraphNode::Ite {
            condition: 0,
            then_value: 1,
            else_value: 2,
        },
        SymbolicGraphNode::ByteRead { state: 0, index: 1 },
        SymbolicGraphNode::WordRead { view: 0, offset: 1 },
        SymbolicGraphNode::SlotRead { state: 0, key: 1 },
        SymbolicGraphNode::Keccak { input: 0 },
        SymbolicGraphNode::BytesZero,
        SymbolicGraphNode::BytesInput {
            source: graph_source.clone(),
            length: 0,
            bindings: vec![],
        },
        SymbolicGraphNode::BytesExact(Arc::from([1])),
        SymbolicGraphNode::WriteByte {
            previous: 0,
            offset: 1,
            value: 2,
        },
        SymbolicGraphNode::WriteWord {
            previous: 0,
            offset: 1,
            value: 2,
        },
        SymbolicGraphNode::Copy {
            previous: 0,
            destination: 1,
            source: 2,
            length: 3,
        },
        SymbolicGraphNode::Slice { source: 0 },
        SymbolicGraphNode::BytesIte {
            condition: 0,
            then_state: 1,
            else_state: 2,
        },
        SymbolicGraphNode::SlotsZero,
        SymbolicGraphNode::SlotsInput {
            source: graph_source,
            bindings: vec![],
        },
        SymbolicGraphNode::Store {
            previous: 0,
            key: 1,
            value: 2,
        },
        SymbolicGraphNode::SlotsIte {
            condition: 0,
            then_state: 1,
            else_state: 2,
        },
        SymbolicGraphNode::View {
            state: 0,
            offset: 1,
            length: 2,
        },
    ];
    let mut flat_metadata = word_metadata;
    flat_metadata[2].1 = Kind::Newtype;
    for ((_, kind), name) in byte_metadata.into_iter().zip([
        "BytesZero",
        "BytesInput",
        "BytesExact",
        "WriteByte",
        "WriteWord",
        "Copy",
        "Slice",
        "BytesIte",
    ]) {
        flat_metadata.push((name, kind));
    }
    for ((_, kind), name) in
        slot_metadata
            .into_iter()
            .zip(["SlotsZero", "SlotsInput", "Store", "SlotsIte"])
    {
        flat_metadata.push((name, kind));
    }
    flat_metadata.push(("View", record(&["state", "offset", "length"])));
    assert_variants("SymbolicGraphNode", &flat, &flat_metadata);
    let fresh = ExprId::fresh().unwrap();
    assert_eq!(fresh.kind().serialize(Capture).unwrap().kind, Kind::Unit);
    assert_eq!(serde_json::to_value(fresh.kind()).unwrap(), json!("Fresh"));
    assert_eq!(
        serde_json::to_value(SymbolicGraphNode::Fresh(123)).unwrap(),
        json!({"Fresh": 123})
    );
}

#[test]
fn numeric_binding_hash_collisions_keep_distinct_interned_states() {
    let source = SourceId::fresh().unwrap();
    let value = ExprId::fresh().unwrap();
    let a = StateBinding {
        index: U256::ZERO,
        value,
        numeric: NumericValue::constant(U256::from(7)),
    };
    let b = StateBinding {
        numeric: NumericValue::constant(U256::from(9)),
        ..a.clone()
    };
    assert_eq!(
        crate::domain::symbolic::state::fingerprint(&a),
        crate::domain::symbolic::state::fingerprint(&b)
    );
    assert_ne!(a, b);
    let limits = ExprLimits::default().into();
    let bytes_a = ByteStateId::observed(source.clone(), c(32), vec![a.clone()], limits).unwrap();
    let bytes_b = ByteStateId::observed(source.clone(), c(32), vec![b.clone()], limits).unwrap();
    assert_ne!(bytes_a, bytes_b);
    assert_ne!(
        SlotStateId::observed(source.clone(), vec![a], limits).unwrap(),
        SlotStateId::observed(source, vec![b], limits).unwrap()
    );
}
