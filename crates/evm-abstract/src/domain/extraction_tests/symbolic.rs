//! Execution adapters retain byte-array quotation and graph identity regressions.

use crate::{
    U256,
    domain::{
        EvmValue, NumericValue, ProductAlgebra, ValueAlgebra,
        symbolic::{ByteView, ExprError, ExprId, ExprLimits, GraphLimits, SourceId, SymbolicGraph},
    },
    resource::WorkBudget,
    world::EvmByteArray,
};
use serde::Serialize;
use serde_json::{Value, json};

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}

fn child<'a>(graph: &'a Value, index: &Value) -> &'a Value {
    &graph["nodes"][usize::try_from(index.as_u64().expect("graph-local index")).unwrap()]
}

fn root(graph: &Value) -> &Value {
    child(graph, &graph["root"])
}

fn node<'a>(graph: &'a Value, name: &str) -> &'a Value {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|node| node.get(name))
        .unwrap_or_else(|| panic!("missing {name} in {graph}"))
}

fn leaf_identity(graph: &Value, leaf: &Value) -> Value {
    assert!(leaf.get("Fresh").is_some() || leaf.get("Input").is_some());
    let scope = graph["identity_scope"].as_str().expect("native namespace");
    json!([scope, leaf])
}

fn source_identity(graph: &Value, source: &Value) -> Value {
    json!([
        leaf_identity(graph, child(graph, &source["token"])),
        source["namespace"]
    ])
}

#[test]
fn cold_and_warm_world_quotes_keep_the_same_payment_and_mutable_metadata() {
    type Bytes = EvmByteArray<NumericValue>;
    let value = |n: u64| EvmValue::<NumericValue>::constant(U256::from(n));
    let domain = ValueAlgebra::new(ProductAlgebra::default());
    let mut memory = Bytes::memory();
    memory
        .copy_from(
            &value(0),
            &Bytes::unknown(),
            &value(0),
            &value(4),
            domain.borrowed(),
        )
        .unwrap();
    memory
        .copy_from(
            &value(8),
            &Bytes::exact(&[1, 0, 2, 0]),
            &value(0),
            &value(4),
            domain.borrowed(),
        )
        .unwrap();
    let root = memory.state_root().clone();
    let cloned = memory.clone();
    assert!(root.cached_exact_payload_bytes().is_none());
    let encoded = serde_json::to_value(&memory).unwrap();
    let cold = memory.work_size();
    assert_eq!(root.cached_exact_payload_bytes(), Some(4));
    for array in [&memory, &cloned] {
        assert_eq!(array.work_size(), cold);
        let mut exact_budget = WorkBudget::new(cold);
        assert!(exact_budget.charge(array.work_size()));
        assert_eq!(exact_budget.used(), cold);
        let mut insufficient = WorkBudget::new(cold - 1);
        assert!(!insufficient.charge(array.work_size()));
        assert_eq!(insufficient.used(), 0);
    }
    assert_eq!(encoded, serde_json::to_value(&memory).unwrap());
    let mut expanded = cloned;
    expanded
        .expand(&value(64), &value(32), domain.borrowed())
        .unwrap();
    assert_eq!(expanded.state_root(), &root);
    assert_eq!(root.exact_payload_bytes(), 4);
    assert!(expanded.work_size() > cold);
}

#[test]
fn native_byte_array_view_and_word_read_share_source_and_length() {
    #[derive(Serialize)]
    struct Report<'a> {
        bytes: &'a EvmByteArray<NumericValue>,
        view: ByteView,
        word: EvmValue<NumericValue>,
        source: &'a SourceId,
        independent: EvmByteArray<NumericValue>,
    }
    let bytes = EvmByteArray::<NumericValue>::unknown();
    let source = match bytes.state_root().kind() {
        crate::domain::symbolic::ByteStateKind::Input { source, .. } => source,
        other => panic!("unexpected input {other:?}"),
    };
    let report = serde_json::to_value(Report {
        bytes: &bytes,
        view: bytes.view(),
        word: bytes
            .read_word_checked(
                &EvmValue::<NumericValue>::constant(U256::ZERO),
                crate::domain::ValueAlgebra::new(ProductAlgebra::default()).borrowed(),
            )
            .unwrap(),
        source,
        independent: EvmByteArray::<NumericValue>::unknown(),
    })
    .unwrap();
    let graph = &report["bytes"]["root"];
    let input = node(graph, "BytesInput");
    let source_key = source_identity(graph, &input["source"]);
    let length_key = leaf_identity(graph, child(graph, &input["length"]));
    for other in [&report["view"]["state"], &report["word"]["expression"]] {
        let input = node(other, "BytesInput");
        assert_eq!(source_identity(other, &input["source"]), source_key);
        assert_eq!(
            leaf_identity(other, child(other, &input["length"])),
            length_key
        );
    }
    for other in [
        &report["bytes"]["length"]["expression"],
        &report["view"]["length"],
    ] {
        assert_eq!(leaf_identity(other, root(other)), length_key);
    }
    let token_graph = &report["source"]["token"];
    assert_eq!(
        json!([
            leaf_identity(token_graph, root(token_graph)),
            report["source"]["namespace"]
        ]),
        source_key
    );
    let independent = &report["independent"]["root"];
    let input = node(independent, "BytesInput");
    assert_ne!(source_identity(independent, &input["source"]), source_key);
    assert_ne!(
        leaf_identity(independent, child(independent, &input["length"])),
        length_key
    );
}

#[test]
fn native_outer_serialization_propagates_cancellation() {
    let bytes = EvmByteArray::<NumericValue>::unknown();
    let view = bytes.view();
    let read = ExprId::word_read(view.clone(), c(0), ExprLimits::default()).unwrap();
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert!(matches!(
            SymbolicGraph::expression(&read, GraphLimits::default()),
            Err(ExprError::Cancelled)
        ));
        assert!(matches!(
            SymbolicGraph::view(&view, GraphLimits::default()),
            Err(ExprError::Cancelled)
        ));
        assert!(
            serde_json::to_value(&bytes)
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
    });
    assert!(serde_json::to_value(bytes).is_ok());
}
