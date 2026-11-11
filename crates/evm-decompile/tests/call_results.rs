//! Logical call buffers must preserve the original copy, reads and first fault.
use evm_decompile::{
    Artifact, Completion, Config, ExecutionError, ExecutionOutcome, ExitKind, ExternalResponse,
    Fork, Input, LiftedConstructionKind, LiftedIfPlan, MemoryHost, ProcedurePreference, U256,
    ValidationStatus, decompile, render_text, validate_stage,
};

#[path = "lifted/error_payloads/fixtures/asm.rs"]
#[allow(dead_code)]
mod assembly;
#[path = "lifted/error_payloads/oracle.rs"]
mod execution;
#[path = "lifted/oracle/expressions.rs"]
mod oracle;
use assembly::Asm;

const PRECOPY: &str = include_str!("fixtures/call-results/delegate-precopy-return.hex");
const REPEATED: &str = include_str!("fixtures/call-results/delegate-repeated-sizes.hex");
const NESTED: &str = include_str!("fixtures/call-results/delegate-nested-buffers.hex");

fn input(hex: &str) -> Input {
    Input::new(evm_bytecode::parse_hex(hex.trim()).unwrap(), Fork::Osaka)
}

fn checked(input: &Input) -> Artifact {
    let mut config = Config::default();
    config.readability.procedures = ProcedurePreference::Inline;
    let artifact = decompile(input, &config);
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| stage.validation.status == ValidationStatus::Passed)
    );
    artifact
}

fn response(success: bool, data: &[u8]) -> ExternalResponse {
    ExternalResponse {
        result: U256::from(u64::from(success)),
        return_data: data.to_vec(),
        ..Default::default()
    }
}

fn bytes(outcome: &ExecutionOutcome, host: &MemoryHost) -> Vec<u8> {
    let ExecutionOutcome::Exit {
        kind: ExitKind::Return | ExitKind::Revert,
        values,
    } = outcome
    else {
        panic!("expected original return/revert, got {outcome:?}")
    };
    let length = usize::try_from(values[1]).unwrap();
    if length == 0 {
        return Vec::new();
    }
    let offset = usize::try_from(values[0]).unwrap();
    host.memory[offset..offset + length].to_vec()
}

#[test]
fn prepared_forwarding_names_the_call_buffer_and_keeps_prefix_copy_on_both_routes() {
    for hex in [PRECOPY, REPEATED] {
        let input = input(hex);
        let artifact = checked(&input);
        let ir = &artifact.current().unwrap().ir;
        let program = ir.lifted.as_ref().unwrap();
        let index = program.render_index(ir);
        let boundary = ir.structured.as_ref().unwrap().root.id;
        assert_eq!(
            program
                .controls
                .iter()
                .filter(|control| matches!(
                    index.if_plan(control.node, boundary),
                    LiftedIfPlan::Require(_)
                ))
                .count(),
            1
        );
        let plan = program
            .constructions
            .iter()
            .find(|plan| plan.kind == LiftedConstructionKind::PreparedConsumer)
            .expect("lazy consumer witness");
        let data = program
            .revert_data
            .iter()
            .find(|data| data.id == plan.revert)
            .unwrap();
        assert_eq!(data.writes.len(), 1);
        assert!(
            !ir.block(plan.block)
                .unwrap()
                .operations
                .iter()
                .any(|op| data.writes.contains(&op.id)),
            "the unconditional prefix copy must not become part of the failed-only capsule"
        );
        let text = render_text(&artifact);
        assert!(
            text.contains("(success1, returndata1) = evm.delegatecall("),
            "{text}"
        );
        assert!(text.contains("size = returndata1.size"), "{text}");
        assert!(text.contains("Revert(data = returndata1)"), "{text}");
        assert!(
            !text.contains("returndata(op") && !text.contains("evm.returndatasize("),
            "{text}"
        );
        assert!(
            text.find("evm.returndatacopy(").unwrap() < text.find("require(").unwrap(),
            "{text}"
        );
        assert_eq!(text.matches("evm.returndatacopy(").count(), 1);
        for data in [vec![], vec![0xde, 0xad, 0, 0xff], vec![0x42; 65]] {
            for success in [false, true] {
                for limit in [0, 32, 128] {
                    let mut host = MemoryHost::new(&input, Vec::new());
                    host.responses.push_back(response(success, &data));
                    let (result, state, _) = execution::certify(&input, &artifact, host, limit);
                    if data.len().div_ceil(32) * 32 > limit {
                        assert_eq!(
                            result,
                            Err(ExecutionError::MemoryLimit),
                            "prefix COPY must still fault even on success"
                        );
                    } else {
                        let outcome = result.unwrap();
                        assert!(matches!(&outcome, ExecutionOutcome::Exit { kind, .. }
                            if *kind == if success { ExitKind::Return } else { ExitKind::Revert }));
                        assert_eq!(bytes(&outcome, &state), data);
                        assert!(state.responses.is_empty());
                    }
                }
            }
        }
    }
}

#[test]
fn successive_calls_have_distinct_buffers_and_failure_stops_before_the_next_call() {
    let input = input(NESTED);
    let artifact = checked(&input);
    let text = render_text(&artifact);
    for number in [1, 2] {
        assert!(
            text.contains(&format!(
                "(success{number}, returndata{number}) = evm.delegatecall("
            )),
            "{text}"
        );
        assert!(
            text.contains(&format!("Revert(data = returndata{number})")),
            "{text}"
        );
    }
    assert_eq!(text.matches("require(").count(), 2, "{text}");
    assert!(
        !text.contains("evm.returndatasize(") && !text.contains("returndata(op"),
        "{text}"
    );
    for a in [false, true] {
        for b in [false, true] {
            let first = [1, 2, 3];
            let second = [9, 8, 7, 6, 5];
            let mut host = MemoryHost::new(&input, Vec::new());
            host.responses.push_back(response(a, &first));
            host.responses.push_back(response(b, &second));
            let (outcome, state, _) = execution::certify(&input, &artifact, host, 4096);
            assert_eq!(
                bytes(&outcome.unwrap(), &state),
                if a { second.to_vec() } else { first.to_vec() }
            );
            assert_eq!(state.responses.len(), usize::from(!a));
        }
    }
}

fn call(a: &mut Asm, opcode: u8, target: u64) {
    a.number(0).number(0).number(0).number(0);
    if matches!(opcode, 0xf1 | 0xf2) {
        a.number(0);
    }
    a.number(target).number(100_000).op(opcode);
}

#[test]
fn all_status_call_opcodes_use_the_same_tuple_and_size_contract() {
    for (opcode, name) in [
        (0xf1, "call"),
        (0xf2, "callcode"),
        (0xf4, "delegatecall"),
        (0xfa, "staticcall"),
    ] {
        let mut a = Asm::default();
        call(&mut a, opcode, 1);
        a.op(0x50)
            .op(0x3d)
            .op(0x80)
            .number(0)
            .number(0)
            .op(0x3e)
            .number(0)
            .op(0xf3);
        let input = a.finish();
        let artifact = checked(&input);
        let text = render_text(&artifact);
        assert!(
            text.contains(&format!("(_, returndata1) = evm.{name}(")),
            "{text}"
        );
        assert!(
            text.contains("returndata1.size") && !text.contains("evm.returndatasize("),
            "{text}"
        );
        let mut host = MemoryHost::new(&input, Vec::new());
        host.responses.push_back(response(true, &[1, 2, 3]));
        let (outcome, host, _) = execution::certify(&input, &artifact, host, 4096);
        assert_eq!(bytes(&outcome.unwrap(), &host), vec![1, 2, 3]);
    }
}

#[test]
fn failure_observations_cannot_be_absorbed_by_prepared_consumer_proofs() {
    let mut a = Asm::default();
    call(&mut a, 0xf4, 1);
    a.op(0x3d)
        .op(0x80)
        .number(0)
        .number(0)
        .op(0x3e)
        .op(0x90)
        .target("success")
        .op(0x57)
        .op(0x50)
        .number(0)
        .op(0x54)
        .op(0x50)
        .op(0x3d)
        .number(0)
        .op(0xfd)
        .dest("success")
        .number(0)
        .op(0xf3);
    let input = a.finish();
    let artifact = checked(&input);
    let ir = &artifact.current().unwrap().ir;
    let program = ir.lifted.as_ref().unwrap();
    assert!(
        program
            .controls
            .iter()
            .all(|control| control.guard.is_none())
    );
    assert!(render_text(&artifact).contains("evm.sload("));
    for success in [false, true] {
        let mut host = MemoryHost::new(&input, Vec::new());
        host.responses.push_back(response(success, &[0xaa]));
        let _ = execution::certify(&input, &artifact, host, 4096);
    }
    let mut forged = artifact.current().unwrap().clone();
    let plan = forged
        .ir
        .lifted
        .as_mut()
        .unwrap()
        .constructions
        .iter_mut()
        .find(|plan| plan.kind == LiftedConstructionKind::PreparedConsumer)
        .unwrap();
    assert!(plan.start > 0);
    plan.start = 0;
    assert_ne!(validate_stage(&forged).status, ValidationStatus::Passed);
}

#[test]
fn changing_the_buffer_on_failure_does_not_relabel_old_memory_as_new_return_data() {
    let mut a = Asm::default();
    call(&mut a, 0xf4, 1);
    a.op(0x3d)
        .op(0x80)
        .number(0)
        .number(0)
        .op(0x3e)
        .op(0x90)
        .target("success")
        .op(0x57)
        .op(0x50);
    call(&mut a, 0xf4, 2);
    a.op(0x50)
        .op(0x3d)
        .number(0)
        .op(0xfd)
        .dest("success")
        .number(0)
        .op(0xf3);
    let input = a.finish();
    let artifact = checked(&input);
    assert!(!render_text(&artifact).contains("require("));
    for success in [false, true] {
        let mut host = MemoryHost::new(&input, Vec::new());
        host.responses.push_back(response(success, &[0x11; 4]));
        host.responses.push_back(response(false, &[0x99; 2]));
        let (outcome, host, _) = execution::certify(&input, &artifact, host, 4096);
        assert_eq!(
            bytes(&outcome.unwrap(), &host),
            vec![0x11; if success { 4 } else { 2 }]
        );
        assert_eq!(host.responses.len(), usize::from(success));
    }
}

#[test]
fn a_read_before_the_call_cannot_borrow_a_future_buffer_binding() {
    let mut a = Asm::default();
    a.op(0x3d);
    call(&mut a, 0xf4, 1);
    a.op(0x50)
        .op(0x3d)
        .op(0x80)
        .number(0)
        .number(0)
        .op(0x3e)
        .op(0x90)
        .op(0x50)
        .number(0)
        .op(0xf3);
    let input = a.finish();
    let artifact = checked(&input);
    let text = render_text(&artifact);
    assert_eq!(text.matches("evm.returndatasize(").count(), 1, "{text}");
    assert!(
        text.find("evm.returndatasize(").unwrap() < text.find("evm.delegatecall(").unwrap(),
        "{text}"
    );
    assert!(text.contains("returndata1.size"), "{text}");
    let mut host = MemoryHost::new(&input, Vec::new());
    host.responses.push_back(response(true, &[0x66]));
    let (outcome, host, _) = execution::certify(&input, &artifact, host, 4096);
    assert_eq!(bytes(&outcome.unwrap(), &host), vec![0x66]);
}

#[test]
fn a_join_of_different_call_epochs_keeps_the_unresolved_size_read_explicit() {
    let mut a = Asm::default();
    a.number(0).op(0x35).target("other").op(0x57);
    call(&mut a, 0xf4, 1);
    a.op(0x50).target("join").op(0x56).dest("other");
    call(&mut a, 0xf4, 2);
    a.op(0x50)
        .dest("join")
        .op(0x3d)
        .op(0x80)
        .number(0)
        .number(0)
        .op(0x3e)
        .number(0)
        .op(0xf3);
    let input = a.finish();
    let artifact = checked(&input);
    let text = render_text(&artifact);
    assert!(
        text.contains("evm.returndatasize(") && !text.contains(".size"),
        "{text}"
    );
    for branch in [0_u8, 1] {
        let mut data = vec![0; 32];
        data[31] = branch;
        let mut host = MemoryHost::new(&input, data);
        host.responses.push_back(response(true, &[branch, 0x42]));
        let (outcome, host, _) = execution::certify(&input, &artifact, host, 4096);
        assert_eq!(bytes(&outcome.unwrap(), &host), vec![branch, 0x42]);
    }
}

#[test]
fn create_results_keep_their_address_word_role() {
    let mut a = Asm::default();
    a.number(0)
        .number(0)
        .number(0)
        .op(0xf0)
        .number(128)
        .op(0x52)
        .op(0x3d)
        .op(0x80)
        .number(0)
        .number(0)
        .op(0x3e)
        .number(0)
        .op(0xf3);
    let input = a.finish();
    let artifact = checked(&input);
    let text = render_text(&artifact);
    assert!(
        text.contains("evm.create(") && !text.contains("success"),
        "{text}"
    );
    let mut host = MemoryHost::new(&input, Vec::new());
    host.responses.push_back(ExternalResponse::default());
    let _ = execution::certify(&input, &artifact, host, 4096);
}

#[test]
fn captured_usdc_proxy_uses_prepared_forwarding_and_preserves_success_return_bytes() {
    let capture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/known-recursion/captured/usdc-26180958-observed-world.json"
    ))
    .unwrap();
    let account = &capture["accounts"][1];
    let input = input(account["code"].as_str().unwrap());
    // Exercise the ordinary user policy, including any selected shared code.
    let artifact = decompile(&input, &Config::default());
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    let ir = &artifact.current().unwrap().ir;
    let program = ir.lifted.as_ref().unwrap();
    let index = program.render_index(ir);
    let boundary = ir.structured.as_ref().unwrap().root.id;
    let consumers: Vec<_> = program
        .constructions
        .iter()
        .filter(|plan| {
            plan.kind == LiftedConstructionKind::PreparedConsumer
                && ir
                    .block(plan.block)
                    .unwrap()
                    .source
                    .is_some_and(|source| source.pc == 0x6a3)
        })
        .collect();
    assert!(
        !consumers.is_empty(),
        "the supplied proxy failure site needs prepared data"
    );
    for plan in &consumers {
        assert!(
            program.controls.iter().any(|control| {
                control.guard.as_ref().is_some_and(|guard| {
                    guard.payload == evm_decompile::LiftedGuardPayload::Data(plan.revert)
                }) && matches!(
                    index.if_plan(control.node, boundary),
                    LiftedIfPlan::Require(_)
                )
            }),
            "captured forwarding guard did not select require at {:?}",
            plan.block
        );
    }
    let text = render_text(&artifact);
    assert!(text.contains("returndata") && text.contains(".size"));
    assert!(
        !text.contains("returndata(op"),
        "internal call IDs leaked into normal output"
    );
    if let Some(directory) = std::env::var_os("CALL_RESULTS_EVIDENCE_DIR") {
        let directory = std::path::Path::new(&directory);
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(directory.join("usdc-proxy-high.txt"), &text).unwrap();
    }
    let word = |text: &str| U256::from_str_radix(text.trim_start_matches("0x"), 16).unwrap();
    for success in [false, true] {
        let returned = vec![0xa5; 65];
        let mut host = MemoryHost::new(&input, vec![0x12, 0x34, 0x56, 0x78]);
        host.storage = account["storage"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (word(key), word(value.as_str().unwrap())))
            .collect();
        host.context
            .insert(0x30, word(account["address"].as_str().unwrap()));
        host.context.insert(0x33, U256::from(0xcafe));
        host.context.insert(0x5a, U256::from(1_000_000));
        host.responses.push_back(response(success, &returned));
        let (outcome, host, _) = execution::certify(&input, &artifact, host, 4096);
        assert_eq!(bytes(&outcome.unwrap(), &host), returned);
    }
}

#[test]
fn refused_projection_reconfiguration_cannot_reuse_a_prepared_require_or_old_cut() {
    let input = input(PRECOPY);
    let artifact = checked(&input);
    let ir = &artifact.current().unwrap().ir;
    let program = ir.lifted.as_ref().unwrap();
    let root = &ir.structured.as_ref().unwrap().root;
    let call_block = ir
        .blocks
        .iter()
        .find(|block| block.operations.iter().any(|op| op.opcode == 0xf4))
        .unwrap()
        .id;
    let mut nodes = vec![root];
    let mut cursor = 0;
    while cursor < nodes.len() {
        match &nodes[cursor].kind {
            evm_decompile::StructuredKind::Sequence(children) => nodes.extend(children),
            evm_decompile::StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => nodes.extend([&**then_branch, &**else_branch]),
            _ => {}
        }
        cursor += 1;
    }
    let call_node = nodes.iter().find(|node| matches!(node.kind, evm_decompile::StructuredKind::Block {block} if block == call_block)).unwrap().id;
    let mut refusals = 0;
    for allowance in [0_usize, 1, 16, 64, 256, 1024, 4096] {
        let mut index = program.render_index(ir);
        assert!(program.controls.iter().any(|control| matches!(
            index.if_plan(control.node, root.id),
            LiftedIfPlan::Require(_)
        )));
        let mut remaining = allowance;
        let result = index.set_call_result_projection(
            |node| node == call_node,
            |cost| {
                if cost > remaining {
                    false
                } else {
                    remaining -= cost;
                    true
                }
            },
        );
        refusals += usize::from(result.is_err());
        assert!(program.controls.iter().all(|control| !matches!(
            index.if_plan(control.node, root.id),
            LiftedIfPlan::Require(_)
        )));
        for node in &nodes {
            if let evm_decompile::StructuredKind::Sequence(children) = &node.kind {
                for position in 0..children.len() {
                    assert!(
                        index
                            .selected_sequence_end(node.id, position, root.id)
                            .is_none()
                    );
                }
            }
        }
    }
    assert!(refusals > 0);
}
