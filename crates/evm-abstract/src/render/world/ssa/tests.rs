use super::render;
use crate::{
    Address, Fork, U256,
    analysis::{self, ExecutionConfig, MachineEdgeKind, Status, WorldAnalysis},
    domain::Value,
    ssa,
    world::{Account, ByteArray, Entry, World},
};
use serde_json::Value as Json;
use std::collections::BTreeSet;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn run(world: World, entry: Address, calldata: ByteArray) -> WorldAnalysis {
    let graph = analysis::analyze_world(
        world,
        Entry {
            address: entry,
            caller: address(0x900),
            value: Value::constant(U256::ZERO),
            calldata,
            is_static: false,
        },
        ExecutionConfig {
            analysis: analysis::Config {
                context_depth: 0,
                ..analysis::Config::default()
            },
            use_summaries: false,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
    graph
}

fn fixture(code: &str, child: Option<&str>, calldata: ByteArray) -> WorldAnalysis {
    let mut world = World::new(Fork::Osaka, "SSA renderer fixture");
    world
        .insert(
            address(0x101),
            Account::from_hex(code, Fork::Osaka).unwrap(),
        )
        .unwrap();
    if let Some(child) = child {
        world
            .insert(
                address(0x200),
                Account::from_hex(child, Fork::Osaka).unwrap(),
            )
            .unwrap();
    }
    run(world, address(0x101), calldata)
}

fn rendered(graph: &WorldAnalysis) -> (ssa::WorldSsa, String) {
    let ir = ssa::build_world(graph).unwrap();
    ir.verify(graph).unwrap();
    let text = render(graph, &ir);
    (ir, text)
}

// 字段集合直接取序列化后的证据；结构扩充时此测试会要求同步可读视图。
fn fields(value: &Json, expected: &[&str]) {
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        expected.iter().copied().collect::<BTreeSet<_>>()
    );
}

fn names(values: &Json) -> String {
    format!(
        "[{}]",
        values
            .as_array()
            .unwrap()
            .iter()
            .map(|value| format!("%{}", value.as_u64().unwrap()))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn number(value: &Json) -> u64 {
    value.as_u64().unwrap()
}

fn contains(text: &str, expected: &str) {
    assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
}

fn section<'a>(text: &'a str, start: &str, next: &str) -> &'a str {
    let rest = text.split_once(start).unwrap().1;
    rest.split_once(next).map_or(rest, |(section, _)| section)
}

#[test]
fn readable_text_covers_every_serialized_ssa_field_and_frame_context() {
    let graph = fixture(
        "602a5f5f5f5f5f61020061fffff100",
        Some("60075f55600b5f52600360095f5ffd"),
        ByteArray::empty(),
    );
    let (ir, text) = rendered(&graph);
    let json = serde_json::to_value(&ir).unwrap();
    fields(
        &json,
        &["blocks", "transitions", "value_count", "effect_count"],
    );
    contains(
        &text,
        &format!(
            "blocks={} | transitions={} | stack values={} | effect bundles={}",
            json["blocks"].as_array().unwrap().len(),
            json["transitions"].as_array().unwrap().len(),
            number(&json["value_count"]),
            number(&json["effect_count"])
        ),
    );
    for block in json["blocks"].as_array().unwrap() {
        fields(
            block,
            &[
                "state",
                "phis",
                "instructions",
                "exit_frames",
                "effect",
                "effects",
                "exit_effect",
            ],
        );
        let id = number(&block["state"]);
        let body = section(&text, &format!("  S{id} | active="), "\n  S");
        let body = body
            .split_once("\nTransitions")
            .map_or(body, |(body, _)| body);
        let state = &graph.states()[usize::try_from(id).unwrap()];
        contains(body, &format!("call depth={}", state.key.frames.len()));
        contains(
            body,
            &format!("machine code identity={}", state.key.code_identity),
        );
        for (frame, key) in state.key.frames.iter().enumerate() {
            contains(body, &format!("F{frame} "));
            contains(body, &format!("B{} (", key.basic_block_index));
            contains(
                body,
                &format!("mode={:?} | stack height={}", key.mode, key.stack_height),
            );
            contains(
                body,
                &format!(
                    "code address={} | storage owner={} | code hash={}",
                    key.code_address, key.address, key.code_hash
                ),
            );
            contains(
                body,
                &format!(
                    "caller={} | static={} | jump history={:?}",
                    key.caller, key.is_static, key.jump_history
                ),
            );
        }
        let phis = block["phis"].as_array().unwrap();
        assert_eq!(body.matches(" = frame phi(").count(), phis.len());
        for phi in phis {
            fields(phi, &["frame", "slot", "result", "inputs"]);
            let inputs = phi["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pair| format!("T{}: %{}", number(&pair[0]), number(&pair[1])))
                .collect::<Vec<_>>()
                .join(", ");
            contains(
                body,
                &format!(
                    "%{} = frame phi(F{}, slot={}, inputs=[{inputs}])",
                    number(&phi["result"]),
                    number(&phi["frame"]),
                    number(&phi["slot"])
                ),
            );
        }
        fields(&block["effect"], &["result", "inputs"]);
        let effect_inputs = block["effect"]["inputs"].as_array().unwrap();
        let mut inputs = Vec::new();
        for input in effect_inputs {
            fields(input, &["transition", "effect"]);
            inputs.push(format!(
                "T{}: !{}",
                number(&input["transition"]),
                number(&input["effect"])
            ));
        }
        let effect = if inputs.is_empty() {
            "root world/entry; inputs=[]".to_owned()
        } else {
            format!("inputs=[{}]", inputs.join(", "))
        };
        contains(
            body,
            &format!(
                "!{} = effect phi({effect})",
                number(&block["effect"]["result"])
            ),
        );
        let instructions = block["instructions"].as_array().unwrap();
        assert_eq!(body.matches(" | opcode=").count(), instructions.len());
        for instruction in instructions {
            fields(
                instruction,
                &["pc", "opcode", "immediate", "operands", "results", "fault"],
            );
            let immediate = instruction["immediate"].as_str().unwrap_or("none");
            contains(
                body,
                &format!(
                    "pc=0x{:04x}: {} | opcode=0x{:02x} | immediate={} | operands={} | results={} | fault={}",
                    number(&instruction["pc"]),
                    revm_bytecode::opcode::OpCode::name_by_op(
                        u8::try_from(number(&instruction["opcode"])).unwrap()
                    ),
                    number(&instruction["opcode"]),
                    immediate,
                    names(&instruction["operands"]),
                    names(&instruction["results"]),
                    instruction["fault"].as_bool().unwrap()
                ),
            );
        }
        let stacks = section(
            body,
            "    stack out (before dispatch):\n",
            "    instruction effects:",
        );
        assert_eq!(
            stacks.lines().count(),
            block["exit_frames"].as_array().unwrap().len()
        );
        for (frame, stack) in block["exit_frames"].as_array().unwrap().iter().enumerate() {
            contains(stacks, &format!("F{frame}: {}", names(stack)));
        }
        for effect in block["effects"].as_array().unwrap() {
            contains(
                body,
                &format!(
                    "pc=0x{:04x}: !{} -> !{}",
                    number(&effect[0]),
                    number(&effect[1]),
                    number(&effect[2])
                ),
            );
        }
        contains(
            body,
            &format!("exit effect: !{}", number(&block["exit_effect"])),
        );
    }
    for (index, transition) in json["transitions"].as_array().unwrap().iter().enumerate() {
        fields(
            transition,
            &[
                "edge",
                "kind",
                "operands",
                "stacks",
                "effect_input",
                "effect_result",
                "result",
            ],
        );
        let edge = &graph.edges()[usize::try_from(number(&transition["edge"])).unwrap()];
        let body = section(&text, &format!("  T{index} | "), "\n  T");
        contains(
            body,
            &format!(
                "edge={} | S{} -> S{} | kind={:?}",
                number(&transition["edge"]),
                edge.from,
                edge.to,
                ir.transitions()[index].kind
            ),
        );
        assert_eq!(transition["kind"], serde_json::to_value(edge.kind).unwrap());
        contains(
            body,
            &format!("operands={}", names(&transition["operands"])),
        );
        let stacks = section(body, "    destination stack in:\n", "    effect:");
        assert_eq!(
            stacks.lines().count(),
            transition["stacks"].as_array().unwrap().len()
        );
        for (frame, stack) in transition["stacks"].as_array().unwrap().iter().enumerate() {
            contains(stacks, &format!("F{frame}: {}", names(stack)));
        }
        contains(
            body,
            &format!(
                "effect: !{} -> !{}",
                number(&transition["effect_input"]),
                number(&transition["effect_result"])
            ),
        );
        let result = transition["result"]
            .as_u64()
            .map_or_else(|| "none".to_owned(), |result| format!("%{result}"));
        contains(body, &format!("deferred result: {result}"));
    }
    assert_eq!(text, render(&graph, &ir));
}

#[test]
fn call_revert_and_immediate_failure_keep_dispatch_and_deferred_results_explicit() {
    let graph = fixture(
        "602a5f5f5f5f5f61020061fffff100",
        Some("60075f555f5ffd"),
        ByteArray::empty(),
    );
    let (ir, text) = rendered(&graph);
    for kind in [
        MachineEdgeKind::Call,
        MachineEdgeKind::Revert,
        MachineEdgeKind::Failure,
    ] {
        assert!(
            ir.transitions()
                .iter()
                .any(|transition| transition.kind == kind)
        );
    }
    contains(&text, "F0 suspended");
    contains(&text, "F1 active");
    contains(&text, "CALL success boolean");
    contains(
        &text,
        "resume caller; restore saved checkpoint; retain revert data",
    );
    contains(&text, "reject call or restore child checkpoint");
    contains(
        &text,
        "rollback checkpoints; this is not alias-partitioned MemorySSA",
    );
}

#[test]
fn creation_results_are_addresses_for_success_and_same_frame_failure() {
    let creator = address(0x101);
    let mut world = World::new(Fork::Osaka, "CREATE renderer fixture");
    world
        .insert(
            creator,
            Account::from_hex("5f5f5360015f5ff000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world.insert(creator.create(0), Account::absent()).unwrap();
    let graph = run(world, creator, ByteArray::empty());
    let (ir, text) = rendered(&graph);
    assert!(
        ir.transitions()
            .iter()
            .any(|transition| transition.kind == MachineEdgeKind::Failure)
    );
    let returned = ir
        .transitions()
        .iter()
        .find(|transition| transition.kind == MachineEdgeKind::Return)
        .unwrap();
    assert!(returned.result.is_some());
    contains(&text, "mode=InitCode");
    contains(&text, "CREATE/CREATE2 address; zero on failure or revert");
    contains(&text, "resume caller; commit child effects");
    for (index, transition) in ir
        .transitions()
        .iter()
        .enumerate()
        .filter(|(_, transition)| transition.result.is_some())
    {
        let body = section(&text, &format!("  T{index} | "), "\n  T");
        contains(body, "CREATE/CREATE2 address");
        assert!(!body.contains("CALL success boolean"));
        let edge = &graph.edges()[transition.edge];
        if transition.kind == MachineEdgeKind::Failure {
            assert_eq!(
                graph.states()[edge.from].key.frames.len(),
                graph.states()[edge.to].key.frames.len()
            );
        }
    }
}

#[test]
fn empty_native_and_faulted_blocks_do_not_invent_bytecode_steps() {
    let empty = fixture("", None, ByteArray::empty());
    let (ir, text) = rendered(&empty);
    assert!(ir.blocks()[0].instructions.is_empty());
    contains(&text, "mode=Empty");
    contains(&text, "bytecode instructions:\n      (none;");
    contains(&text, "effect phi(root world/entry; inputs=[])");
    contains(&text, "Transitions\n  (none)");
    let native = run(
        World::new(Fork::Osaka, "native renderer fixture"),
        address(4),
        ByteArray::exact(b"hello"),
    );
    let (ir, text) = rendered(&native);
    assert!(ir.blocks()[0].instructions.is_empty());
    contains(&text, "mode=Precompile(");
    contains(&text, "instruction effects:\n      (none)");
    assert!(!text.contains("opcode="));
    let fault = fixture("01", None, ByteArray::empty());
    let (_, text) = rendered(&fault);
    contains(
        &text,
        "ADD | opcode=0x01 | immediate=none | operands=[] | results=[] | fault=true",
    );
}

#[test]
fn parallel_edges_have_distinct_transition_and_effect_phi_inputs() {
    let graph = fixture("602a5f356007575b00", None, ByteArray::unknown());
    let (ir, text) = rendered(&graph);
    let edges = graph.edges();
    let parallel = edges
        .iter()
        .enumerate()
        .find_map(|(first, edge)| {
            edges
                .iter()
                .enumerate()
                .skip(first + 1)
                .find_map(|(second, other)| {
                    (edge.from == other.from && edge.to == other.to)
                        .then_some((first, second, edge))
                })
        })
        .expect("fixture has a taken branch and fallthrough to the same block");
    let (first, second, edge) = parallel;
    for index in [first, second] {
        contains(
            &text,
            &format!("T{index} | edge={index} | S{} -> S{}", edge.from, edge.to),
        );
    }
    let destination = &ir.blocks()[edge.to];
    assert!(
        destination
            .phis
            .iter()
            .any(|phi| phi.inputs.iter().any(|(index, _)| *index == first)
                && phi.inputs.iter().any(|(index, _)| *index == second))
    );
    contains(
        &text,
        &format!("T{first}: !{}", ir.transitions()[first].effect_result),
    );
    contains(
        &text,
        &format!("T{second}: !{}", ir.transitions()[second].effect_result),
    );
}

#[test]
fn invalid_delegation_is_an_exceptional_halt_without_an_instruction_list() {
    let mut world = World::new(Fork::Osaka, "nested delegation renderer");
    let root = address(0x101);
    let implementation = address(0x200);
    let mut authority = Account::empty();
    authority.code = crate::world::Code::Delegation(implementation);
    let mut nested = Account::empty();
    nested.code = crate::world::Code::Delegation(root);
    world.insert(root, authority).unwrap();
    world.insert(implementation, nested).unwrap();
    let graph = run(world, root, ByteArray::empty());
    let (ir, text) = rendered(&graph);
    assert!(ir.blocks()[0].instructions.is_empty());
    assert!(
        graph
            .outcomes()
            .iter()
            .all(|o| o.kind == crate::analysis::OutcomeKind::Failure)
    );
    contains(&text, "mode=InvalidDelegation");
    contains(&text, "invalid nested delegation; exceptional halt");
    assert!(!text.contains("implicit completion"));
}

#[test]
fn execution_failure_is_distinct_from_an_invalid_opcode_or_stack_fault() {
    let mut world = World::new(Fork::Osaka, "static write renderer");
    let root = address(0x101);
    world
        .insert(root, Account::from_hex("60015f5500", Fork::Osaka).unwrap())
        .unwrap();
    let graph = analysis::analyze_world(
        world,
        Entry {
            address: root,
            caller: address(0x900),
            value: Value::constant(U256::ZERO),
            calldata: ByteArray::empty(),
            is_static: true,
        },
        ExecutionConfig::default(),
    )
    .unwrap();
    let (ir, text) = rendered(&graph);
    let write = ir.blocks()[0]
        .instructions
        .iter()
        .find(|i| i.opcode == revm_bytecode::opcode::SSTORE)
        .unwrap();
    assert!(!write.fault);
    assert!(
        graph
            .outcomes()
            .iter()
            .all(|o| o.kind == crate::analysis::OutcomeKind::Failure)
    );
    contains(&text, "fault marks invalid-opcode or stack faults only");
    contains(&text, "SSTORE");
}
