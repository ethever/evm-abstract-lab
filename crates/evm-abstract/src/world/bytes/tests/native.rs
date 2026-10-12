//! Native JSON keeps numeric guarantees and the original cross-graph leaves.

use super::super::*;
use crate::domain::relational::RelationLimits;

#[test]
fn native_byte_operand_entries_preserve_bounds_and_root_leaf_identity() {
    let domain = Domain::default();
    let offset = AbstractValue::unsigned_range(U256::ZERO, U256::from(64))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let value = AbstractValue::unsigned_range(U256::from(7), U256::from(8))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let mut array = ByteArray::memory();
    array.write_word(&offset, &value, 96, domain).unwrap();
    let json = serde_json::to_value(&array).unwrap();
    let entries = json["operands"].as_array().unwrap();
    assert_eq!(entries.len(), array.symbolic_facts().count());
    for (expression, fact) in array.symbolic_facts() {
        let encoded = serde_json::to_value(expression).unwrap();
        let entry = entries.iter().find(|e| e["expression"] == encoded).unwrap();
        assert_eq!(entry["value"], serde_json::to_value(fact).unwrap());
        assert_eq!(
            entry["expression"]["identity_scope"],
            json["root"]["identity_scope"]
        );
        let root = encoded["root"].as_u64().unwrap() as usize;
        assert!(
            json["root"]["nodes"]
                .as_array()
                .unwrap()
                .contains(&encoded["nodes"][root])
        );
    }
    assert!(json["failure"].is_null());
}

#[test]
fn native_byte_failures_are_visible_and_fact_export_is_bounded_and_cancellable() {
    let low = Domain::from_spec(Domain::default().spec().with_relations(RelationLimits {
        max_depth: 1,
        ..RelationLimits::default()
    }));
    let failed = ByteArray::exact(&[1]).join(&ByteArray::exact(&[2]), low);
    let json = serde_json::to_value(&failed).unwrap();
    assert_eq!(
        json["failure"].as_str(),
        Some(failed.failure.as_ref().unwrap().to_string().as_str())
    );
    let mut oversized = ByteArray::empty();
    for _ in 0..5000 {
        let e = ExprId::fresh().unwrap();
        oversized
            .operands
            .insert(e.clone(), AbstractValue::top().with_expression(e));
    }
    let error = serde_json::to_vec(&oversized).unwrap_err().to_string();
    assert!(
        error.contains("metadata policy") || error.contains("exceeding its policy"),
        "{error}"
    );
    let cancellation = embedded_smt::Cancellation::new();
    cancellation.cancel();
    embedded_smt::with_cancellation(&cancellation, || {
        assert!(serde_json::to_value(ByteArray::unknown()).is_err());
    });
}

#[test]
fn native_fact_list_charges_repeated_embedded_observations_not_only_flat_nodes() {
    use crate::domain::symbolic::{GraphLimits, SlotStateId, StateBinding, SymbolicGraph};
    #[derive(serde::Serialize)]
    struct Record<'a> {
        #[serde(serialize_with = "crate::world::bytes::native::serialize_facts")]
        facts: &'a BTreeMap<ExprId, AbstractValue>,
    }
    let limits = ExprLimits {
        max_nodes: 100_000,
        max_depth: 256,
    };
    let shared = ExprId::fresh().unwrap();
    let observed = SlotStateId::observed(
        SourceId::fresh().unwrap(),
        (0..800usize)
            .map(|i| StateBinding {
                index: U256::from(i),
                value: shared.clone(),
                numeric: AbstractValue::constant(U256::ZERO).numeric().clone(),
            })
            .collect(),
        limits.into(),
    )
    .unwrap();
    let mut facts = BTreeMap::new();
    for i in 800..810usize {
        let read =
            ExprId::slot_read(observed.clone(), ExprId::constant(U256::from(i)), limits).unwrap();
        let graph = SymbolicGraph::expression(&read, GraphLimits::default()).unwrap();
        assert!(read.nodes() > graph.nodes.len() * 100);
        facts.insert(read, AbstractValue::top());
    }
    assert!(serde_json::to_vec(&Record { facts: &facts }).is_err());
    let one = facts.into_iter().take(1).collect();
    assert!(serde_json::to_vec(&Record { facts: &one }).is_ok());
}
