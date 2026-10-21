//! 回归测试验证端到端的可观察语义，不依赖工作表的具体访问顺序。
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisInput, ExecutionConfig};

#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::NumericValue;
use evm_abstract::{
    U256,
    analysis::{self, Config, DiagnosticKind, EdgeKind, EvmAnalysis, Status},
    bytecode::{DecodeError, Program},
    ssa,
};

fn cfg_fixture(hex: &str) -> EvmAnalysis<NumericValue> {
    // 基础 CFG/join 回归沿用上下文不敏感的对照，避免精度默认值掩盖汇合。
    native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(
            Program::from_hex(hex).unwrap(),
            Default::default(),
        )
        .unwrap(),
        ExecutionConfig {
            analysis: Config {
                context_depth: 0,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap()
}
fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../examples/{name}.hex",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}
fn states_at(
    analysis: &EvmAnalysis<NumericValue>,
    pc: usize,
) -> Vec<&analysis::EvmState<NumericValue>> {
    analysis
        .states()
        .iter()
        .filter(|s| analysis.program().blocks()[s.key.basic_block_index].start_pc == pc)
        .collect()
}

#[test]
fn push_data_is_not_a_jumpdest() {
    let program = Program::from_hex("600456605b00").unwrap();
    assert!(program.jumpdest_blocks().is_empty());
    let analysis = cfg_fixture("600456605b00");
    assert!(analysis.edges().is_empty());
    assert!(
        analysis
            .diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::InvalidJump)
    );
}

#[test]
fn truncated_push_pads_on_the_right() {
    let program = Program::from_hex("61ab").unwrap();
    assert_eq!(
        program.blocks()[0].instructions[0].immediate,
        Some(U256::from(0xab00))
    );
    assert_eq!(
        cfg_fixture("61ab").states()[0].exit_stack[0]
            .constants()
            .unwrap()
            .len(),
        1
    );
    assert!(cfg_fixture("61ab").states()[0].exit_stack[0].contains(U256::from(0xab00)));
}

#[test]
fn unsupported_and_invalid_inputs_are_distinct() {
    assert!(matches!(
        Program::from_hex("ef0001"),
        Err(DecodeError::UnsupportedEof)
    ));
    assert!(matches!(
        Program::from_hex("abc"),
        Err(DecodeError::OddHexLength(3))
    ));
    assert!(matches!(
        Program::from_hex("gg"),
        Err(DecodeError::InvalidHex { .. })
    ));
    assert_eq!(Program::from_hex("0x60 01\n00").unwrap().byte_len(), 3);
    for code in ["0c600100", "4b600100", "e6600100", "fe600100", "f8600100"] {
        let a = cfg_fixture(code);
        assert_eq!(a.states()[0].executed_pcs, [0]);
        assert!(
            a.diagnostics()
                .iter()
                .any(|d| matches!(d.kind, DiagnosticKind::InvalidOpcode(_)))
        );
        ssa::build(&a).unwrap();
    }
}

#[test]
fn empty_program_is_an_implicit_stop() {
    let a = cfg_fixture("");
    assert_eq!(a.status(), Status::Converged);
    assert!(a.states().is_empty());
    assert_eq!(ssa::build(&a).unwrap().value_count(), 0);
}

#[test]
fn unreachable_blocks_remain_visible_in_disassembly() {
    let a = cfg_fixture("005b600100");
    assert_eq!(a.program().blocks().len(), 2);
    assert_eq!(a.states().len(), 1);
    assert!(a.edges().is_empty());
}

#[test]
fn arithmetic_target_is_recovered() {
    // 3 + 3 -> pc=6，目标不是紧邻 JUMP 的 PUSH 常量。
    let a = cfg_fixture("6003600301565b00");
    assert_eq!(a.edges().len(), 1);
    assert_eq!(states_at(&a, 6).len(), 1);
    assert!(
        !a.diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::UnknownJump)
    );
}

#[test]
fn zero_condition_does_not_validate_invalid_target() {
    let a = cfg_fixture("600060ff5700");
    assert_eq!(a.edges().len(), 1);
    assert_eq!(
        a.edges()[0].kind,
        evm_abstract::analysis::MachineEdgeKind::Intraprocedural(EdgeKind::BranchFalse)
    );
    assert!(
        !a.diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::InvalidJump)
    );
    ssa::build(&a).unwrap();
}

#[test]
fn nonzero_condition_prunes_fallthrough() {
    let a = cfg_fixture("600160075700005b00");
    assert_eq!(a.edges().len(), 1);
    assert_eq!(
        a.edges()[0].kind,
        evm_abstract::analysis::MachineEdgeKind::Intraprocedural(EdgeKind::BranchTrue)
    );
    assert_eq!(states_at(&a, 7).len(), 1);
}

#[test]
fn unknown_jump_preserves_the_storage_write_path() {
    let a = cfg_fixture(&fixture("dynamic-jump"));
    assert_eq!(a.status(), Status::Converged);
    assert_eq!(states_at(&a, 4).len(), 1);
    assert!(
        a.diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::UnknownJump)
    );
    assert!(
        ssa::build(&a)
            .unwrap()
            .blocks()
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| i.opcode == 0x55)
    );
}

#[test]
fn unknown_jump_covers_every_actual_destination() {
    let a = cfg_fixture("600035565b005b005b00");
    let targets: std::collections::BTreeSet<_> = a
        .edges()
        .iter()
        .filter(|e| e.from == 0)
        .map(|e| a.program().blocks()[a.states()[e.to].key.basic_block_index].start_pc)
        .collect();
    assert_eq!(targets, std::collections::BTreeSet::from([4, 6, 8]));
}

#[test]
fn diamond_joins_values_and_ssa_tracks_each_predecessor() {
    let a = cfg_fixture(&fixture("diamond"));
    let merge = states_at(&a, 14)[0];
    let values = merge.entry_stack[0].constants().unwrap();
    assert_eq!(
        values,
        &std::collections::BTreeSet::from([U256::from(1), U256::from(2)])
    );
    assert!(merge.exit_stack[0].contains(U256::from(11)));
    assert!(merge.exit_stack[0].contains(U256::from(12)));
    let ssa = ssa::build(&a).unwrap();
    let phi = &ssa.blocks()[merge.id].phis[0];
    assert_eq!(phi.inputs.len(), 2);
    assert_ne!(phi.inputs[0].value, phi.inputs[1].value);
}

#[test]
fn loop_reaches_a_finite_fixed_point_and_has_a_backedge_phi() {
    let a = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(
            Program::from_hex(&fixture("loop")).unwrap(),
            Default::default(),
        )
        .unwrap(),
        ExecutionConfig {
            analysis: Config {
                context_depth: 0,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap();
    assert_eq!(a.status(), Status::Converged);
    assert!(a.transfers() < 30);
    let head = states_at(&a, 2)[0];
    assert!(head.entry_stack[0].constants().is_none());
    assert!(
        a.edges()
            .iter()
            .any(|e| e.from == head.id && e.to == head.id)
    );
    let ssa = ssa::build(&a).unwrap();
    assert_eq!(ssa.blocks()[head.id].phis[0].inputs.len(), 2);
}

#[test]
fn contexts_keep_two_internal_invocations_separate() {
    let program = Program::from_hex(&fixture("internal-calls")).unwrap();
    let insensitive = cfg_fixture(&fixture("internal-calls"));
    let sensitive = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(program, Default::default()).unwrap(),
        ExecutionConfig {
            analysis: Config {
                context_depth: 1,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap();
    assert_eq!(states_at(&insensitive, 14).len(), 1);
    let calls = states_at(&sensitive, 14);
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].key.context, calls[1].key.context);
    assert!(
        calls
            .iter()
            .all(|s| s.entry_stack[0].constants().unwrap().len() == 1)
    );
    assert!(
        states_at(&insensitive, 14)[0].entry_stack[0]
            .constants()
            .unwrap()
            .len()
            > 1
    );
    ssa::build(&sensitive).unwrap();
}

#[test]
fn stack_heights_are_separate_states() {
    let a = cfg_fixture(&fixture("stack-heights"));
    let merge = states_at(&a, 12);
    assert_eq!(merge.len(), 2);
    let heights: std::collections::BTreeSet<_> = merge.iter().map(|s| s.key.stack_height).collect();
    assert_eq!(heights, std::collections::BTreeSet::from([0, 1]));
    ssa::build(&a).unwrap();
}

#[test]
fn dup_and_swap_preserve_value_identity() {
    let a = cfg_fixture("60018060029000");
    let ssa = ssa::build(&a).unwrap();
    let block = &ssa.blocks()[0];
    assert_eq!(ssa.value_count(), 2);
    assert_eq!(block.exit_stack, [0, 1, 0]);
    assert!(
        block
            .instructions
            .iter()
            .filter(|i| matches!(i.opcode, 0x80 | 0x90))
            .all(|i| i.results.is_empty())
    );
}

#[test]
fn exceptional_stack_paths_do_not_execute_later_instructions() {
    let underflow = cfg_fixture("01600100");
    assert_eq!(underflow.states()[0].executed_pcs, [0]);
    assert!(
        underflow
            .diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::StackUnderflow)
    );
    ssa::build(&underflow).unwrap();
    let overflow = cfg_fixture(&format!("{}00", "5f".repeat(1025)));
    assert_eq!(overflow.states()[0].exit_stack.len(), 1024);
    assert!(
        overflow
            .diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::StackOverflow)
    );
    assert_eq!(ssa::build(&overflow).unwrap().value_count(), 1024);
}

#[test]
fn budget_frontiers_are_typed_and_prevent_ssa() {
    for config in [
        Config {
            max_states: 1,
            ..Config::default()
        },
        Config {
            max_transfers: 1,
            ..Config::default()
        },
    ] {
        let a = native::run(
            &native::product(),
            AnalysisInput::<NumericValue>::from_program(
                Program::from_hex(&fixture("diamond")).unwrap(),
                Default::default(),
            )
            .unwrap(),
            ExecutionConfig {
                analysis: config,
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
        .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
        .unwrap();
        assert_eq!(a.status(), Status::Incomplete);
        assert!(!a.frontiers().is_empty());
        assert!(matches!(
            ssa::build(&a),
            Err(ssa::SsaError::IncompleteAnalysis)
        ));
    }
}

#[test]
fn invalid_configs_are_rejected_before_execution() {
    for config in [
        Config {
            max_states: 0,
            ..Config::default()
        },
        Config {
            max_transfers: 0,
            ..Config::default()
        },
    ] {
        assert!(
            native::run(
                &native::product(),
                AnalysisInput::<NumericValue>::from_program(
                    Program::from_hex("00").unwrap(),
                    Default::default()
                )
                .unwrap(),
                ExecutionConfig {
                    analysis: config,
                    ..ExecutionConfig::default()
                },
                &Control::default()
            )
            .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
            .is_err()
        );
    }
}

#[test]
fn result_exports_are_deterministic() {
    let first = cfg_fixture(&fixture("loop"));
    let second = cfg_fixture(&fixture("loop"));
    assert_eq!(first.status(), Status::Converged);
    assert_eq!(second.status(), Status::Converged);
    ssa::build(&first).unwrap();
    ssa::build(&second).unwrap();
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&first).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&second).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
    assert_eq!(
        evm_abstract::render::dot(
            &first,
            &native::product_with(first.execution().domain_spec())
        ),
        evm_abstract::render::dot(
            &second,
            &native::product_with(second.execution().domain_spec())
        )
    );
    assert!(native_exports_equal_with_fresh_bijection(
        &serde_json::to_value(&first).unwrap(),
        &serde_json::to_value(&second).unwrap()
    ));
}

// Each distinct private leaf gets a distinct report-wide canonical label.
// Only RelationState constraints and path Atoms may be reordered.
fn native_exports_equal_with_fresh_bijection(
    left: &serde_json::Value,
    right: &serde_json::Value,
) -> bool {
    fn rename(
        value: &mut serde_json::Value,
        leaves: &mut std::collections::BTreeMap<u64, u64>,
    ) -> bool {
        match value {
            serde_json::Value::Object(object) => {
                if object.len() == 1 && object.contains_key("Fresh") {
                    let Some(original) = object["Fresh"].as_u64() else {
                        return false;
                    };
                    let next = leaves.len() as u64;
                    object["Fresh"] = (*leaves.entry(original).or_insert(next)).into();
                    true
                } else {
                    object.values_mut().all(|value| rename(value, leaves))
                }
            }
            serde_json::Value::Array(values) => {
                values.iter_mut().all(|value| rename(value, leaves))
            }
            _ => true,
        }
    }
    fn order_conjunctions(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(object) => {
                for value in object.values_mut() {
                    order_conjunctions(value);
                }
                if object.len() == 3
                    && object
                        .get("bottom")
                        .is_some_and(serde_json::Value::is_boolean)
                    && object.get("path").is_some_and(serde_json::Value::is_object)
                    && let Some(atoms) = object
                        .get_mut("constraints")
                        .and_then(serde_json::Value::as_array_mut)
                {
                    atoms.sort_by_cached_key(|atom| serde_json::to_string(atom).unwrap());
                    if let Some(path) = object
                        .get_mut("path")
                        .and_then(serde_json::Value::as_object_mut)
                        && path.len() == 2
                        && let Some(nodes) = path.get("nodes").and_then(serde_json::Value::as_array)
                        && path
                            .get("root")
                            .and_then(serde_json::Value::as_u64)
                            .is_some_and(|root| root < nodes.len() as u64)
                        && let Some(nodes) = path
                            .get_mut("nodes")
                            .and_then(serde_json::Value::as_array_mut)
                    {
                        // Sort whole conjuncts, preserving duplicates and every DAG reference.
                        for node in nodes {
                            if let Some(node) = node.as_object_mut()
                                && node.len() == 1
                                && let Some(atoms) = node
                                    .get_mut("Atoms")
                                    .and_then(serde_json::Value::as_array_mut)
                            {
                                atoms.sort_by_cached_key(|atom| {
                                    serde_json::to_string(atom).unwrap()
                                });
                            }
                        }
                    }
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    order_conjunctions(value);
                }
            }
            _ => {}
        }
    }
    let mut left = left.clone();
    let mut right = right.clone();
    if !rename(&mut left, &mut Default::default()) || !rename(&mut right, &mut Default::default()) {
        return false;
    }
    order_conjunctions(&mut left);
    order_conjunctions(&mut right);
    left == right
}

#[test]
fn result_export_fresh_bijection_preserves_aliasing_and_model_fields() {
    let first = serde_json::json!({
        "graph": {"identity_scope": "producer", "root": 1, "nodes": [{"Fresh": 1}, {"Fresh": 2}]},
        "history": {"identity_scope": "producer", "root": 3, "nodes": [
            {"Fresh": 1},
            {"SlotsInput": {"source": {"token": 0, "namespace": "0x101"}, "bindings": []}},
            {"Constant": "0x4"},
            {"SlotRead": {"state": 1, "key": 2}}
        ]},
        "input": {"Input": {"scope": 7, "symbol": "CallValue"}},
        "source": {"token": 0, "namespace": "0x101"},
        "operands": [{
            "expression": {"identity_scope": "producer", "root": 0, "nodes": [{"Fresh": 2}]},
            "value": evm_abstract::domain::EvmValue::<NumericValue>::constant(U256::from(7))
        }],
        "failure": null,
        "state": {"entry_relations": {
            "constraints": [
                {"Truth": {"expression": {"identity_scope": "producer", "root": 0, "nodes": [{"Fresh": 1}]}, "nonzero": true}},
                {"Range": {"expression": {"identity_scope": "producer", "root": 0, "nodes": [{"Fresh": 2}]}, "lower": "0x3", "upper": "0x5"}}
            ],
            "bottom": false,
            "path": {"root": 0, "nodes": ["True"]}
        }}
    });
    let mut second = first.clone();
    second["graph"]["nodes"][0]["Fresh"] = 11.into();
    second["graph"]["nodes"][1]["Fresh"] = 12.into();
    second["history"]["nodes"][0]["Fresh"] = 11.into();
    second["operands"][0]["expression"]["nodes"][0]["Fresh"] = 12.into();
    second["state"]["entry_relations"]["constraints"][0]["Truth"]["expression"]["nodes"][0]["Fresh"] =
        11.into();
    second["state"]["entry_relations"]["constraints"][1]["Range"]["expression"]["nodes"][0]["Fresh"] =
        12.into();
    second["state"]["entry_relations"]["constraints"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    assert!(native_exports_equal_with_fresh_bijection(&first, &second));

    let mut collapsed = second.clone();
    collapsed["graph"]["nodes"][1]["Fresh"] = 11.into();
    assert!(!native_exports_equal_with_fresh_bijection(
        &first, &collapsed
    ));
    let mut split = second.clone();
    split["history"]["nodes"][0]["Fresh"] = 13.into();
    assert!(!native_exports_equal_with_fresh_bijection(&first, &split));
    let mut unchanged_leaf_collision = first.clone();
    unchanged_leaf_collision["graph"]["nodes"][1]["Fresh"] = 1.into();
    assert!(!native_exports_equal_with_fresh_bijection(
        &first,
        &unchanged_leaf_collision
    ));

    for (pointer, value) in [
        (
            "/graph/identity_scope",
            serde_json::json!("another producer"),
        ),
        ("/graph/root", serde_json::json!(0)),
        ("/input/Input/scope", serde_json::json!(8)),
        ("/source/namespace", serde_json::json!("0x102")),
        ("/history/nodes/2/Constant", serde_json::json!("0x5")),
        (
            "/state/entry_relations/constraints/1/Truth/nonzero",
            serde_json::json!(false),
        ),
        (
            "/operands/0/value",
            serde_json::json!(evm_abstract::domain::EvmValue::<NumericValue>::constant(
                U256::from(8)
            )),
        ),
        ("/failure", serde_json::json!("symbolic retention failed")),
    ] {
        let mut changed = second.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(!native_exports_equal_with_fresh_bijection(&first, &changed));
    }
    let mut missing_constraint = second.clone();
    missing_constraint["state"]["entry_relations"]["constraints"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(!native_exports_equal_with_fresh_bijection(
        &first,
        &missing_constraint
    ));
    let mut reordered_history = second.clone();
    reordered_history["history"]["nodes"]
        .as_array_mut()
        .unwrap()
        .swap(1, 2);
    assert!(!native_exports_equal_with_fresh_bijection(
        &first,
        &reordered_history
    ));
}

#[test]
fn result_export_orders_only_complete_relation_path_atoms() {
    let [first, mut second] = [(1_u64, 2_u64), (11, 12)].map(|(a, b)| {
        let leaf = serde_json::json!({
            "identity_scope": "producer", "root": 0, "nodes": [{"Fresh": a}]
        });
        let expression = serde_json::json!({
            "identity_scope": "producer", "root": 4, "nodes": [
                {"Fresh": a},
                {"Fresh": b},
                {"Input": {"scope": 7, "symbol": "CallValue"}},
                {"Operation": {"opcode": 1, "args": [0, 1]}},
                {"Ite": {"condition": 2, "then_value": 3, "else_value": 1}}
            ]
        });
        let truth = serde_json::json!({"Truth": {"expression": expression, "nonzero": true}});
        // Opaque Fact-shaped sentinels check that key/value associations stay intact.
        let facts = serde_json::json!([
            {"Fact": {"key": "a", "value": [7, 8]}},
            {"Fact": {"key": "b", "value": [9, 10]}}
        ]);
        serde_json::json!({
            // Ordered anchors establish both private identities before any conjunction.
            "anchors": expression,
            "branches": [{"outcome": 0}, {"outcome": 1}],
            "lookalike": {
                "Atoms": facts,
                "path": {"root": 0, "nodes": [{"Atoms": facts}]}
            },
            "outcomes": [
                {"expression": leaf, "value": 7},
                {"expression": expression, "value": 8}
            ],
            "relation": {
                "constraints": [], "bottom": false,
                "path": {"root": 3, "nodes": [
                    {"Atoms": [
                        truth,
                        {"Range": {"expression": leaf, "lower": "0x3", "upper": "0x5"}},
                        facts[0], facts[1], truth
                    ]},
                    "False",
                    {"Ite": {"selector": leaf, "then_path": 0, "else_path": 1}},
                    {"And": [0, 2]}
                ]}
            },
            "source": {"token": 0, "namespace": "0x101"},
            "diagnostics": [],
            "work": {"transfers": 7, "cost_version": 3}
        })
    });
    second["relation"]["path"]["nodes"][0]["Atoms"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(native_exports_equal_with_fresh_bijection(&first, &second));

    for (pointer, value) in [
        (
            "/relation/path/nodes/0/Atoms/0/Truth/nonzero",
            serde_json::json!(false),
        ),
        (
            "/relation/path/nodes/0/Atoms/0/Truth/expression/root",
            serde_json::json!(3),
        ),
        (
            "/relation/path/nodes/0/Atoms/0/Truth/expression/nodes/3/Operation/opcode",
            serde_json::json!(2),
        ),
        (
            "/relation/path/nodes/0/Atoms/0/Truth/expression/nodes/4/Ite/condition",
            serde_json::json!(0),
        ),
        (
            "/relation/path/nodes/0/Atoms/0/Truth/expression/nodes/4/Ite",
            serde_json::json!({"condition": 2, "then_value": 1, "else_value": 3}),
        ),
        (
            "/relation/path/nodes/0/Atoms/0/Truth/expression/nodes/1/Fresh",
            serde_json::json!(11),
        ),
        (
            "/relation/path/nodes/0/Atoms/0/Truth/expression/nodes/0/Fresh",
            serde_json::json!(13),
        ),
        (
            "/outcomes/0/expression/nodes/0/Fresh",
            serde_json::json!(13),
        ),
        (
            "/relation/path/nodes/2/Ite/selector/nodes/0/Fresh",
            serde_json::json!(12),
        ),
        (
            "/relation/path/nodes/2/Ite",
            serde_json::json!({
                "selector": second["relation"]["path"]["nodes"][2]["Ite"]["selector"],
                "then_path": 1, "else_path": 0
            }),
        ),
        (
            "/relation/path/nodes/0/Atoms/1/Fact/key",
            serde_json::json!("a"),
        ),
        (
            "/relation/path/nodes/0/Atoms/1/Fact/value",
            serde_json::json!([7, 8]),
        ),
        (
            "/relation/path/nodes/0/Atoms/0/Truth/expression/identity_scope",
            serde_json::json!("another producer"),
        ),
        (
            "/relation/path/nodes/0/Atoms/0/Truth/expression/nodes/2/Input/scope",
            serde_json::json!(8),
        ),
        (
            "/relation/path/nodes/0/Atoms/0/Truth/expression/nodes/2/Input/symbol",
            serde_json::json!("Origin"),
        ),
        ("/source/token", serde_json::json!(1)),
        ("/source/namespace", serde_json::json!("0x102")),
        ("/diagnostics", serde_json::json!(["changed diagnostic"])),
        ("/work/transfers", serde_json::json!(8)),
        ("/work/cost_version", serde_json::json!(4)),
    ] {
        let mut changed = second.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(
            !native_exports_equal_with_fresh_bijection(&first, &changed),
            "changed {pointer}"
        );
    }

    // A missing distinct atom and a missing duplicate must both remain observable.
    for index in [1, 4] {
        let mut missing = second.clone();
        missing["relation"]["path"]["nodes"][0]["Atoms"]
            .as_array_mut()
            .unwrap()
            .remove(index);
        assert!(!native_exports_equal_with_fresh_bijection(&first, &missing));
    }
    let mut extra = second.clone();
    let atoms = extra["relation"]["path"]["nodes"][0]["Atoms"]
        .as_array_mut()
        .unwrap();
    atoms.push(atoms[0].clone());
    assert!(!native_exports_equal_with_fresh_bijection(&first, &extra));
    let mut replaced = second.clone();
    replaced["relation"]["path"]["nodes"][0]["Atoms"][1] =
        second["relation"]["path"]["nodes"][0]["Atoms"][2].clone();
    assert!(!native_exports_equal_with_fresh_bijection(
        &first, &replaced
    ));

    for pointer in [
        "/anchors/nodes",
        "/branches",
        "/outcomes",
        "/relation/path/nodes",
        "/relation/path/nodes/3/And",
        "/relation/path/nodes/0/Atoms/0/Truth/expression/nodes",
        "/relation/path/nodes/0/Atoms/0/Truth/expression/nodes/3/Operation/args",
        "/relation/path/nodes/0/Atoms/1/Fact/value",
        "/lookalike/Atoms",
        "/lookalike/path/nodes/0/Atoms",
    ] {
        let mut reordered = second.clone();
        reordered
            .pointer_mut(pointer)
            .unwrap()
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        assert!(
            !native_exports_equal_with_fresh_bijection(&first, &reordered),
            "reordered {pointer}"
        );
    }

    // A path-shaped object needs the enclosing RelationState schema and a valid root.
    for (pointer, value) in [
        ("/relation/extra", serde_json::json!(true)),
        ("/relation/bottom", serde_json::json!(null)),
        ("/relation/constraints", serde_json::json!(null)),
        ("/relation/path/root", serde_json::json!(4)),
        ("/relation/path/extra", serde_json::json!(true)),
        ("/relation/path/nodes/0/extra", serde_json::json!(true)),
    ] {
        let mut left = first.clone();
        let mut right = second.clone();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        left.pointer_mut(parent).unwrap()[key] = value.clone();
        right.pointer_mut(parent).unwrap()[key] = value;
        assert!(
            !native_exports_equal_with_fresh_bijection(&left, &right),
            "invalid relation path at {pointer}"
        );
    }
}
