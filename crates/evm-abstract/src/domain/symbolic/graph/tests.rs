use super::{GraphLimits, SymbolicGraph};
use crate::{
    domain::{
        AbstractValue, Domain,
        identity::Symbol,
        relational::{CheckResult, RelationLimits, RelationState},
        symbolic::{ByteStateId, ByteView, ExprError, ExprId, ExprLimits, SlotStateId, SourceId},
    },
    world::ByteArray,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use serde::Serialize;
use serde_json::{Value, json};
use std::{process::Command, sync::Arc};

mod serialization;
mod stream;

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
fn native_fresh_leaves_remain_bound_across_independent_graphs() {
    let x = ExprId::fresh().unwrap();
    let y = ExprId::fresh().unwrap();
    let report = serde_json::to_value([x.clone(), x, y]).unwrap();
    assert_eq!(report[0], report[1]);
    assert_ne!(report[0], report[2]);
    assert_eq!(report[0]["root"], 0);
    assert_eq!(report[2]["root"], 0);
    assert_ne!(root(&report[0])["Fresh"], root(&report[2])["Fresh"]);
    assert_eq!(report[0]["identity_scope"], report[2]["identity_scope"]);
    assert_eq!(
        leaf_identity(&report[0], root(&report[0])),
        leaf_identity(&report[1], root(&report[1]))
    );
}

#[test]
fn native_input_atoms_preserve_scope_and_symbol() {
    let x = ExprId::input(1, Symbol::Caller);
    let y = ExprId::input(u64::MAX, Symbol::Caller);
    let report = serde_json::to_value([x.clone(), x.clone(), y.clone()]).unwrap();
    assert_eq!(report[0], report[1]);
    assert_ne!(report[0], report[2]);
    assert_eq!(root(&report[0])["Input"]["symbol"], "Caller");
    assert_eq!(root(&report[2])["Input"]["symbol"], "Caller");
    assert_eq!(root(&report[0])["Input"]["scope"], 1);
    assert_eq!(root(&report[2])["Input"]["scope"], u64::MAX);
    let atoms = serde_json::to_value([
        x.inputs().into_iter().next().unwrap(),
        y.inputs().into_iter().next().unwrap(),
    ])
    .unwrap();
    assert_eq!(atoms[0], root(&report[0])["Input"]);
    assert_eq!(atoms[1], root(&report[2])["Input"]);
}

#[test]
fn native_byte_array_view_and_word_read_share_source_and_length() {
    #[derive(Serialize)]
    struct Report<'a> {
        bytes: &'a ByteArray,
        view: ByteView,
        word: AbstractValue,
        source: &'a SourceId,
        independent: ByteArray,
    }
    let bytes = ByteArray::unknown();
    let source = match bytes.state_root().kind() {
        crate::domain::symbolic::ByteStateKind::Input { source, .. } => source,
        other => panic!("unexpected input {other:?}"),
    };
    let report = serde_json::to_value(Report {
        bytes: &bytes,
        view: bytes.view(),
        word: bytes
            .read_word_checked(&AbstractValue::constant(U256::ZERO), Domain::default())
            .unwrap(),
        source,
        independent: ByteArray::unknown(),
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
fn native_history_storage_and_path_keep_shared_leaves_and_sources() {
    #[derive(Serialize)]
    struct Report {
        before: ExprId,
        after: ExprId,
        storage: SlotStateId,
        path: RelationState,
        independent: SlotStateId,
        other_namespace: SlotStateId,
        key: ExprId,
        value: ExprId,
    }
    let source = SourceId::fresh().unwrap();
    let key = ExprId::fresh().unwrap();
    let value = ExprId::fresh().unwrap();
    let length = ExprId::fresh().unwrap();
    let original = ByteStateId::input(source.clone(), length.clone());
    let before = ExprId::word_read(
        ByteView::new(original.clone(), c(0), length.clone()),
        key.clone(),
        ExprLimits::default(),
    )
    .unwrap();
    let written = original
        .write_byte(key.clone(), value.clone(), ExprLimits::default().into())
        .unwrap();
    let after = ExprId::word_read(
        ByteView::new(written, c(0), length),
        key.clone(),
        ExprLimits::default(),
    )
    .unwrap();
    let storage = SlotStateId::input(source.clone())
        .store(key.clone(), value.clone(), ExprLimits::default().into())
        .unwrap();
    let mut path = RelationState::default();
    let policy = RelationLimits {
        ..RelationLimits::default()
    };
    assert_eq!(
        path.add_equal(&value, &c(7), &policy),
        CheckResult::MayBeFeasible
    );
    let before_export = serde_json::to_value(&before).unwrap();
    let report = serde_json::to_value(Report {
        before,
        after,
        storage,
        path,
        independent: SlotStateId::input(SourceId::fresh().unwrap()),
        other_namespace: SlotStateId::input(SourceId::scoped(
            source.token().clone(),
            U256::from(9),
        )),
        key,
        value,
    })
    .unwrap();
    assert_eq!(report["before"], before_export);
    let before_graph = &report["before"];
    let input = node(before_graph, "BytesInput");
    let source_key = source_identity(before_graph, &input["source"]);
    let length_key = leaf_identity(before_graph, child(before_graph, &input["length"]));
    let after_graph = &report["after"];
    let input = node(after_graph, "BytesInput");
    assert_eq!(source_identity(after_graph, &input["source"]), source_key);
    assert_eq!(
        leaf_identity(after_graph, child(after_graph, &input["length"])),
        length_key
    );
    assert!(
        before_graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n.get("WriteByte").is_none())
    );
    let write = node(after_graph, "WriteByte");
    let store_graph = &report["storage"];
    let stored = node(store_graph, "Store");
    assert_eq!(
        source_identity(store_graph, &node(store_graph, "SlotsInput")["source"]),
        source_key
    );
    let key_key = leaf_identity(&report["key"], root(&report["key"]));
    let value_key = leaf_identity(&report["value"], root(&report["value"]));
    assert_eq!(
        leaf_identity(
            before_graph,
            child(before_graph, &node(before_graph, "WordRead")["offset"])
        ),
        key_key
    );
    assert_eq!(
        leaf_identity(after_graph, child(after_graph, &write["offset"])),
        key_key
    );
    assert_eq!(
        leaf_identity(store_graph, child(store_graph, &stored["key"])),
        key_key
    );
    assert_eq!(
        leaf_identity(after_graph, child(after_graph, &write["value"])),
        value_key
    );
    assert_eq!(
        leaf_identity(store_graph, child(store_graph, &stored["value"])),
        value_key
    );
    let path_graph = &report["path"]["constraints"][0]["Equal"]["left"];
    assert_eq!(leaf_identity(path_graph, root(path_graph)), value_key);
    for name in ["independent", "other_namespace"] {
        let graph = &report[name];
        assert_ne!(
            source_identity(graph, &node(graph, "SlotsInput")["source"]),
            source_key
        );
    }
}

#[test]
fn native_identity_scope_changes_between_processes() {
    const PROBE: &str = "EVM_ABSTRACT_NATIVE_EXPORT_PROBE";
    const PREFIX: &str = "native-identity:";
    if std::env::var_os(PROBE).is_some() {
        println!(
            "{PREFIX}{}",
            serde_json::to_string(&ExprId::fresh().unwrap()).unwrap()
        );
        return;
    }
    let probe = || {
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "domain::symbolic::graph::tests::native_identity_scope_changes_between_processes",
                "--nocapture",
                "--test-threads=4",
            ])
            .env(PROBE, "1")
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let stdout = String::from_utf8(result.stdout).unwrap();
        let graph = stdout
            .lines()
            .find_map(|line| line.strip_prefix(PREFIX))
            .unwrap();
        serde_json::from_str::<Value>(graph).unwrap()
    };
    let a = probe();
    let b = probe();
    assert_eq!(root(&a)["Fresh"], 1);
    assert_eq!(root(&b)["Fresh"], 1);
    assert_ne!(leaf_identity(&a, root(&a)), leaf_identity(&b, root(&b)));
}

#[test]
fn export_keeps_node_depth_and_aggregate_exact_payload_budgets() {
    let expression = ExprId::operation(
        opcode::ADD,
        &[ExprId::fresh().unwrap(), c(3)],
        ExprLimits::default(),
    )
    .unwrap();
    for limits in [
        GraphLimits {
            max_nodes: expression.nodes() - 1,
            ..GraphLimits::default()
        },
        GraphLimits {
            max_depth: expression.depth() - 1,
            ..GraphLimits::default()
        },
    ] {
        assert!(matches!(
            SymbolicGraph::expression(&expression, limits),
            Err(ExprError::Limit { .. })
        ));
    }
    let bytes = ByteStateId::exact(Arc::from([1, 2, 3, 4]));
    let source = ByteView::new(ByteStateId::exact(Arc::from([5, 6, 7, 8])), c(0), c(4));
    let copied = bytes
        .copy(c(1), source, c(1), ExprLimits::default().into())
        .unwrap();
    assert!(matches!(
        SymbolicGraph::bytes(
            &copied,
            GraphLimits {
                max_exact_bytes: 7,
                ..GraphLimits::default()
            }
        ),
        Err(ExprError::ExportBytes { bytes: 8, limit: 7 })
    ));
    assert!(
        SymbolicGraph::bytes(
            &copied,
            GraphLimits {
                max_exact_bytes: 8,
                ..GraphLimits::default()
            }
        )
        .is_ok()
    );
    let relaxed = ExprLimits {
        max_nodes: 65_536,
        max_depth: 512,
    };
    let mut deep = ExprId::fresh().unwrap();
    for _ in 0..GraphLimits::default().max_depth {
        deep = ExprId::operation(opcode::ADD, &[deep, c(1)], relaxed).unwrap();
    }
    assert!(
        serde_json::to_value(deep)
            .unwrap_err()
            .to_string()
            .contains("exceeding its policy")
    );
}

#[test]
fn native_outer_serialization_propagates_cancellation() {
    let bytes = ByteArray::unknown();
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

#[test]
fn exported_distinct_identities_do_not_assert_disequality_or_hash_injectivity() {
    let policy = RelationLimits {
        ..RelationLimits::default()
    };
    let a = ExprId::fresh().unwrap();
    let b = ExprId::fresh().unwrap();
    let left = ByteView::new(
        ByteStateId::input(SourceId::fresh().unwrap(), c(32)),
        c(0),
        c(32),
    );
    let right = ByteView::new(
        ByteStateId::input(SourceId::fresh().unwrap(), c(32)),
        c(0),
        c(32),
    );
    let hash_a = ExprId::keccak(left.clone(), ExprLimits::default()).unwrap();
    let hash_b = ExprId::keccak(right.clone(), ExprLimits::default()).unwrap();
    let report =
        serde_json::to_value([a.clone(), b.clone(), hash_a.clone(), hash_b.clone()]).unwrap();
    assert_ne!(report[0], report[1]);
    assert_ne!(report[2], report[3]);
    let byte_a = ExprId::byte_read(left.state, c(0), ExprLimits::default()).unwrap();
    let byte_b = ExprId::byte_read(right.state, c(0), ExprLimits::default()).unwrap();
    let different_content = ExprId::operation(
        opcode::EQ,
        &[byte_a.clone(), byte_b.clone()],
        ExprLimits::default(),
    )
    .unwrap();
    let same_hash = ExprId::operation(
        opcode::EQ,
        &[hash_a.clone(), hash_b.clone()],
        ExprLimits::default(),
    )
    .unwrap();
    let mut collision = RelationState::default();
    assert_eq!(
        collision.assume(&different_content, false, &policy),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        collision.assume(&same_hash, true, &policy),
        CheckResult::MayBeFeasible
    );
    let pairs = [
        (a, b),
        (
            ExprId::input(1, Symbol::Caller),
            ExprId::input(2, Symbol::Caller),
        ),
        (byte_a, byte_b),
        (hash_a, hash_b),
    ];
    for (left, right) in pairs {
        let equality =
            ExprId::operation(opcode::EQ, &[left, right], ExprLimits::default()).unwrap();
        for nonzero in [true, false] {
            assert_eq!(
                RelationState::default().assume(&equality, nonzero, &policy),
                CheckResult::MayBeFeasible
            );
        }
    }
}

#[test]
fn deep_shared_dag_renaming_and_export_use_bounded_host_stack() {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            const STEPS: usize = 2048;
            let limits = ExprLimits {
                max_nodes: 4 * STEPS,
                max_depth: STEPS + 1,
            };
            let leaf = ExprId::fresh().unwrap();
            let one = c(1);
            // Retain the spine so cleanup does not measure Arc destruction's
            // unrelated host-stack behavior instead of graph traversal.
            let mut retained = vec![leaf.clone()];
            for _ in 0..STEPS {
                retained.push(
                    ExprId::operation(
                        opcode::ADD,
                        &[retained.last().unwrap().clone(), one.clone()],
                        limits,
                    )
                    .unwrap(),
                );
            }
            let root = retained.last().unwrap();
            let input = ExprId::input(u64::MAX, Symbol::Caller);
            let mut renamed = root
                .rename_fresh(
                    &[(leaf.clone(), input.clone())].into_iter().collect(),
                    limits,
                )
                .unwrap();
            assert_eq!(
                (renamed.nodes(), renamed.depth()),
                (2 * STEPS + 1, STEPS + 1)
            );
            assert_eq!(renamed.inputs(), input.inputs());
            assert!(renamed.fresh_leaves().is_empty());
            assert_eq!(root.fresh_leaves(), [leaf].into_iter().collect());
            let graph = SymbolicGraph::expression(&renamed, limits).unwrap();
            assert_eq!(graph.nodes.len(), STEPS + 2);
            assert_eq!(graph.root, STEPS + 1);
            for (index, node) in graph.nodes.iter().enumerate() {
                node.visit_edges(|edge| {
                    let symbolic_structure::Edge::Word(child) = edge else {
                        panic!("word graph")
                    };
                    assert!(child < index);
                });
            }
            let encoded = serde_json::to_value(&graph).unwrap();
            assert_eq!(encoded["nodes"][0]["Input"]["scope"], u64::MAX);
            assert_eq!(
                encoded["nodes"][graph.root]["Operation"]["args"],
                json!([STEPS, 1])
            );
            // Release each rewritten parent while its child is still retained.
            while let crate::domain::symbolic::ExprKind::Operation { args, .. } = renamed.kind() {
                renamed = args[0].clone();
            }
            while retained.pop().is_some() {}
        })
        .unwrap()
        .join()
        .unwrap();
}
