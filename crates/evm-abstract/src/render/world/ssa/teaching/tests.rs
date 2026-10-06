use super::render;
use crate::{
    Address, Fork, U256,
    analysis::{self, ExecutionConfig, MachineEdgeKind, Status, WorldAnalysis},
    domain::Value,
    ssa,
    world::{Account, ByteArray, Code, Entry, World},
};
use revm_bytecode::opcode;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn run(world: World, entry: Address, calldata: ByteArray) -> WorldAnalysis {
    let graph = analysis::analyze_world(
        world,
        Entry::concrete(entry, address(0x900), Value::constant(U256::ZERO), calldata),
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
    let mut world = World::new(Fork::Osaka, "teaching SSA fixture");
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

fn contains(text: &str, expected: &str) {
    assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
}

fn transition_section(text: &str, index: usize) -> &str {
    let start = text.find(&format!("  T{index} | ")).unwrap();
    let rest = &text[start..];
    let end = rest.find("\n  T").unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn shared_layout_separates_world_metadata_from_real_bytecode_block_titles() {
    let graph = fixture("600360078190030100", None, ByteArray::empty());
    let (ir, text) = rendered(&graph);
    contains(
        &text,
        "\nS0 | C0 | F0 active | state owner=A0 | context=[]:\nB0 @ 0x0000:\n",
    );
    let body = text.split_once("B0 @ 0x0000:\n").unwrap().1;
    let instructions = body
        .lines()
        .take_while(|line| !line.contains("stack out (before dispatch)"))
        .collect::<Vec<_>>();
    assert_eq!(instructions.len(), ir.blocks()[0].instructions.len());
    let title_pc_column = "B0 @ 0x".len();
    for line in instructions {
        assert_eq!(line.find(':').unwrap(), title_pc_column + 4);
        assert!(line[..title_pc_column].chars().all(|c| c == ' '));
        assert!(line[title_pc_column..].starts_with("000"));
    }
}

#[test]
fn shared_layout_grows_for_multi_digit_world_block_ids() {
    let mut code = "5b5f50".repeat(12);
    code.push_str("00");
    let graph = fixture(&code, None, ByteArray::empty());
    let (ir, text) = rendered(&graph);
    for block in ir.blocks() {
        let state = &graph.states()[block.state];
        let source = &state.program().unwrap().blocks()[state.active().basic_block_index];
        let title = format!("B{} @ 0x{:04x}:\n", source.id, source.start_pc);
        let body = text.split_once(&title).unwrap().1;
        let column = title.find("0x").unwrap() + 2;
        let instructions = body
            .lines()
            .take_while(|line| !line.contains("stack out (before dispatch)"))
            .collect::<Vec<_>>();
        assert_eq!(instructions.len(), block.instructions.len());
        for line in instructions {
            assert_eq!(line.find(':').unwrap(), column + 4);
            assert!(line[..column].chars().all(|c| c == ' '));
        }
        let stack = body
            .lines()
            .find(|line| line.contains("stack out"))
            .unwrap();
        assert_eq!(stack.find("stack out").unwrap(), column);
    }
}

#[test]
fn tac_keeps_pop_order_and_dup_swap_alias_identity() {
    let graph = fixture("600360078190030100", None, ByteArray::empty());
    let (ir, text) = rendered(&graph);
    contains(&text, "0000: %0 = PUSH1 0x3");
    contains(&text, "0002: %1 = PUSH1 0x7");
    contains(&text, "0004: DUP2 %0 ; duplicate alias");
    contains(&text, "0005: SWAP1 %0 %1 ; aliases reordered");
    contains(&text, "0006: %2 = SUB %1 %0");
    contains(&text, "0007: %3 = ADD %2 %0");
    contains(&text, "stack out (before dispatch) F0: [%3]");
    assert_eq!(
        ir.value_count(),
        4,
        "stack aliases must not define new values"
    );
    for noisy in [
        "opcode=",
        "immediate=",
        "operands=",
        "results=",
        "fault=",
        "code hash=",
        "effect phi",
        "instruction effects:",
    ] {
        assert!(!text.contains(noisy), "unexpected {noisy:?} in:\n{text}");
    }
}

#[test]
fn call_result_definitions_belong_to_resume_transitions() {
    let graph = fixture(
        "602a5f5f5f5f5f61020061fffff100",
        Some("60075f555f5ff3"),
        ByteArray::empty(),
    );
    let (ir, text) = rendered(&graph);
    let (blocks, transitions) = text.split_once("\nTransitions\n").unwrap();
    assert!(!blocks.contains("CALL result"));
    contains(blocks, " CALL %");
    contains(blocks, "F1 active");
    assert!(blocks.lines().any(|line| {
        line.contains("stack out") && line.contains("F0: [%") && line.contains("; F1:")
    }));
    for (index, transition) in ir.transitions().iter().enumerate() {
        let body = transition_section(&text, index);
        match transition.kind {
            MachineEdgeKind::Call => {
                assert_eq!(transition.result, None);
                contains(body, "suspend F0; enter F1; save rollback checkpoint");
                assert!(!body.contains("CALL result"));
            }
            MachineEdgeKind::Return | MachineEdgeKind::Failure => {
                let result = transition.result.unwrap();
                let expected = usize::from(transition.kind == MachineEdgeKind::Return);
                contains(body, &format!("CALL result %{result} = {expected}"));
                if transition.kind == MachineEdgeKind::Return {
                    contains(body, "resume F0; commit child effects");
                }
                assert_eq!(
                    transitions
                        .matches(&format!("CALL result %{result} ="))
                        .count(),
                    1
                );
            }
            MachineEdgeKind::Intraprocedural(_) => {}
            MachineEdgeKind::Revert => panic!("fixture has no revert"),
        }
    }
    assert!(
        ir.transitions()
            .iter()
            .any(|transition| transition.kind == MachineEdgeKind::Return)
    );
    assert!(
        ir.transitions()
            .iter()
            .any(|transition| transition.kind == MachineEdgeKind::Failure)
    );
}

#[test]
fn revert_returns_zero_and_restores_a_checkpoint() {
    let graph = fixture(
        "5f5f5f5f5f61020061fffff100",
        Some("60075f555f5ffd"),
        ByteArray::empty(),
    );
    let (ir, text) = rendered(&graph);
    let (index, transition) = ir
        .transitions()
        .iter()
        .enumerate()
        .find(|(_, transition)| transition.kind == MachineEdgeKind::Revert)
        .unwrap();
    let body = transition_section(&text, index);
    contains(body, "rollback to saved checkpoint; retain revert data");
    contains(
        body,
        &format!("CALL result %{} = 0", transition.result.unwrap()),
    );
    assert!(!body.contains("commit child effects"));
}

#[test]
fn creation_results_are_addresses_and_failure_is_zero() {
    let root = address(0x101);
    let mut world = World::new(Fork::Osaka, "teaching CREATE fixture");
    world
        .insert(
            root,
            Account::from_hex("5f5f5360015f5ff000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world.insert(root.create(0), Account::absent()).unwrap();
    let graph = run(world, root, ByteArray::empty());
    let (ir, text) = rendered(&graph);
    assert!(
        !text
            .split_once("\nTransitions\n")
            .unwrap()
            .0
            .contains("CREATE address")
    );
    assert!(
        ir.transitions()
            .iter()
            .any(|transition| transition.kind == MachineEdgeKind::Return)
    );
    assert!(
        ir.transitions()
            .iter()
            .any(|transition| transition.kind == MachineEdgeKind::Failure)
    );
    for (index, transition) in ir
        .transitions()
        .iter()
        .enumerate()
        .filter(|(_, transition)| transition.result.is_some())
    {
        let body = transition_section(&text, index);
        let value = if transition.kind == MachineEdgeKind::Return {
            "created address"
        } else {
            "0"
        };
        contains(
            body,
            &format!("CREATE address %{} = {value}", transition.result.unwrap()),
        );
        assert!(!body.contains("CALL result"));
    }
}

#[test]
fn parallel_transition_phi_inputs_and_loop_states_are_preserved() {
    let parallel = fixture("602a5f356007575b00", None, ByteArray::unknown());
    let (ir, text) = rendered(&parallel);
    let edges = parallel.edges();
    let (first, second, destination) = edges
        .iter()
        .enumerate()
        .find_map(|(first, edge)| {
            edges
                .iter()
                .enumerate()
                .skip(first + 1)
                .find_map(|(second, other)| {
                    (edge.from == other.from && edge.to == other.to)
                        .then_some((first, second, edge.to))
                })
        })
        .unwrap();
    let phi = ir.blocks()[destination].phis.first().unwrap();
    let first_value = phi
        .inputs
        .iter()
        .find(|(index, _)| *index == first)
        .unwrap()
        .1;
    let second_value = phi
        .inputs
        .iter()
        .find(|(index, _)| *index == second)
        .unwrap()
        .1;
    contains(&text, &format!("T{first}: %{first_value}"));
    contains(&text, &format!("T{second}: %{second_value}"));
    contains(&text, &format!("%{} = phi(", phi.result));
    let loop_graph = fixture("60005b600101600256", None, ByteArray::empty());
    let (ir, text) = rendered(&loop_graph);
    let joined = ir
        .blocks()
        .iter()
        .find(|block| block.phis.iter().any(|phi| phi.inputs.len() > 1))
        .unwrap();
    let phi = joined.phis.iter().find(|phi| phi.inputs.len() > 1).unwrap();
    assert!(
        text.lines()
            .any(|line| line.starts_with(&format!("S{} | C", joined.state))
                && line.contains(" | context="))
    );
    let state = &loop_graph.states()[joined.state];
    let source = &state.program().unwrap().blocks()[state.active().basic_block_index];
    contains(
        &text,
        &format!("\nB{} @ 0x{:04x}:\n", source.id, source.start_pc),
    );
    for (transition, value) in &phi.inputs {
        contains(&text, &format!("T{transition}: %{value}"));
    }
}

#[test]
fn native_empty_delegation_and_synthetic_locations_do_not_invent_pcs() {
    let empty = fixture("", None, ByteArray::empty());
    let (_, text) = rendered(&empty);
    contains(&text, "empty executable code; implicit completion");
    assert!(!text.contains("B0 @ 0x0000"));
    let native = run(
        World::new(Fork::Osaka, "native teaching fixture"),
        address(4),
        ByteArray::exact(b"hello"),
    );
    let (_, text) = rendered(&native);
    contains(&text, "native execution; no bytecode instructions");
    assert!(!text.contains("@ 0x"));
    let mut world = World::new(Fork::Osaka, "invalid delegation teaching fixture");
    let root = address(0x101);
    let target = address(0x200);
    let mut authority = Account::empty();
    authority.code = Code::Delegation(target);
    let mut nested = Account::empty();
    nested.code = Code::Delegation(root);
    world.insert(root, authority).unwrap();
    world.insert(target, nested).unwrap();
    let (_, text) = rendered(&run(world, root, ByteArray::empty()));
    contains(&text, "invalid nested delegation; exceptional halt");
    assert!(!text.contains("implicit completion"));
    let synthetic = fixture("5f5f5f5f5f61020061fffff1", Some("00"), ByteArray::empty());
    let (_, text) = rendered(&synthetic);
    contains(&text, "synthetic end-of-code continuation");
    for state in synthetic.states().iter().filter(|state| {
        state
            .program()
            .and_then(|program| program.blocks().get(state.active().basic_block_index))
            .is_none()
    }) {
        let body = text.split_once(&format!("S{} | ", state.id)).unwrap().1;
        let body = body.split_once("\nS").map_or(body, |(body, _)| body);
        let body = body
            .split_once("\nTransitions")
            .map_or(body, |(body, _)| body);
        assert!(!body.lines().any(|line| line.starts_with('B')));
        assert!(!body.lines().any(|line| {
            line.trim_start()
                .split_once(':')
                .is_some_and(|(pc, _)| !pc.is_empty() && pc.chars().all(|c| c.is_ascii_hexdigit()))
        }));
    }
}

#[test]
fn opcode_or_stack_fault_is_marked_only_on_faulting_instructions() {
    let graph = fixture("01", None, ByteArray::empty());
    let (ir, text) = rendered(&graph);
    assert_eq!(ir.blocks()[0].instructions[0].opcode, opcode::ADD);
    assert!(ir.blocks()[0].instructions[0].fault);
    contains(&text, "0000: ADD ; exceptional halt (opcode/stack)");
    let root = address(0x101);
    let mut world = World::new(Fork::Osaka, "static write teaching fixture");
    world
        .insert(root, Account::from_hex("60035f5500", Fork::Osaka).unwrap())
        .unwrap();
    let graph = analysis::analyze_world(
        world,
        {
            let mut entry = Entry::concrete(
                root,
                address(0x900),
                Value::constant(U256::ZERO),
                ByteArray::empty(),
            );
            entry.environment.is_static = true;
            entry
        },
        ExecutionConfig::default(),
    )
    .unwrap();
    assert!(
        graph
            .outcomes()
            .iter()
            .all(|outcome| outcome.kind == analysis::OutcomeKind::Failure)
    );
    let (_, text) = rendered(&graph);
    contains(&text, "SSTORE %1 %0");
    assert!(!text.contains("exceptional halt (opcode/stack)"));
}

#[test]
fn reverting_initcode_defines_a_zero_address_at_its_transition() {
    let root = address(0x101);
    let mut world = World::new(Fork::Osaka, "CREATE revert teaching fixture");
    world
        .insert(
            root,
            Account::from_hex("625f5ffd5f526003601d5ff000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world.insert(root.create(0), Account::absent()).unwrap();
    let graph = run(world, root, ByteArray::empty());
    let (ir, text) = rendered(&graph);
    let (index, transition) = ir
        .transitions()
        .iter()
        .enumerate()
        .find(|(_, transition)| transition.kind == MachineEdgeKind::Revert)
        .unwrap();
    let body = transition_section(&text, index);
    contains(
        body,
        &format!("CREATE address %{} = 0", transition.result.unwrap()),
    );
    contains(body, "rollback to saved checkpoint; retain revert data");
    assert!(!body.contains("CALL result"));
}

#[test]
fn delegatecall_header_separates_code_identity_from_state_owner() {
    let graph = fixture("5f5f5f5f61020061fffff400", Some("00"), ByteArray::empty());
    let (_, text) = rendered(&graph);
    let child = graph
        .states()
        .iter()
        .find(|state| state.key.frames.len() == 2)
        .unwrap();
    assert_eq!(child.active().code_address, address(0x200));
    assert_eq!(child.active().address, address(0x101));
    contains(
        &text,
        &format!(
            "S{} | C1 | F1 active | state owner=A0 | context=[]:",
            child.id
        ),
    );
    contains(&text, "\nB0 @ 0x0000:\n");
}
