//! Differential fixtures use a direct stack/PC executor, not recovery facts.
use evm_decompile::{
    Artifact, Cancellation, Completion, Config, EntityRef, ExecutionLimits, ExecutionOutcome,
    ExternalResponse, FactScope, Fork, Input, MemoryHost, Phase, Stage, StageKind, Strategy,
    StructuredKind, StructuredNode, Terminator, U256, ValidationStatus, ValueFact, decompile,
    decompile_with_cancel, decompile_with_progress, execute_bytecode, execute_ir,
    execute_structured, render_text,
};
use std::collections::BTreeMap;

#[derive(Default)]
struct Asm {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}
impl Asm {
    fn op(&mut self, op: u8) -> &mut Self {
        self.bytes.push(op);
        self
    }
    fn push(&mut self, value: u8) -> &mut Self {
        self.bytes.extend([0x60, value]);
        self
    }
    fn dest(&mut self, name: &'static str) -> &mut Self {
        self.labels.insert(name, self.bytes.len());
        self.op(0x5b)
    }
    fn target(&mut self, name: &'static str) -> &mut Self {
        self.bytes.push(0x60);
        self.fixups.push((self.bytes.len(), name));
        self.bytes.push(0);
        self
    }
    fn finish(mut self) -> Input {
        for (offset, name) in self.fixups {
            self.bytes[offset] = u8::try_from(self.labels[name]).expect("small fixture");
        }
        Input::new(self.bytes, Fork::Osaka)
    }
}
fn calldata(word: u64) -> Vec<u8> {
    U256::from(word).to_be_bytes::<32>().to_vec()
}
fn baseline(a: &Artifact) -> &Stage {
    a.stages
        .iter()
        .find(|s| s.kind == StageKind::Baseline)
        .expect("baseline")
}
fn counts(node: &StructuredNode) -> (usize, usize, usize) {
    let mut sum = match node.kind {
        StructuredKind::If { .. } => (1, 0, 0),
        StructuredKind::While { .. } => (0, 1, 0),
        StructuredKind::Dispatcher { .. } => (0, 0, 1),
        _ => (0, 0, 0),
    };
    let children: Vec<_> = match &node.kind {
        StructuredKind::Sequence(v) => v.iter().collect(),
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => vec![&**then_branch, &**else_branch],
        StructuredKind::While { header, body, .. } => vec![&**header, &**body],
        StructuredKind::Indirect { arms, .. } => arms.iter().map(|a| &a.body).collect(),
        _ => Vec::new(),
    };
    for child in children {
        let c = counts(child);
        sum.0 += c.0;
        sum.1 += c.1;
        sum.2 += c.2;
    }
    sum
}
fn check(input: &Input, config: &Config, host: MemoryHost) -> Artifact {
    let artifact = decompile(input, config);
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:#?}",
        artifact.diagnostics
    );
    assert!(
        artifact
            .stages
            .iter()
            .all(|s| s.validation.status == ValidationStatus::Passed),
        "{:#?}",
        artifact
            .stages
            .iter()
            .map(|s| &s.validation)
            .collect::<Vec<_>>()
    );
    assert_eq!(artifact.current().unwrap().kind, StageKind::Structured);
    let limits = ExecutionLimits::default();
    let mut raw_host = host.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits).unwrap();
    for stage in artifact
        .stages
        .iter()
        .filter(|s| s.kind != StageKind::Decoded)
    {
        let mut graph_host = host.clone();
        let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
        assert_eq!(raw.outcome, graph.outcome, "graph {:?}", stage.id);
        assert_eq!(
            raw.observations(),
            graph.observations(),
            "graph {:?}",
            stage.id
        );
        assert_eq!(raw_host, graph_host, "graph host {:?}", stage.id);
        if stage.ir.structured.is_some() {
            let mut syntax_host = host.clone();
            let syntax = execute_structured(stage, &mut syntax_host, &limits).unwrap();
            assert_eq!(raw.outcome, syntax.outcome, "AST {:?}", stage.id);
            assert_eq!(
                raw.observations(),
                syntax.observations(),
                "AST {:?}",
                stage.id
            );
            assert_eq!(raw_host, syntax_host, "AST host {:?}", stage.id);
        }
    }
    let wire = serde_json::to_vec(&artifact).unwrap();
    let decoded: Artifact = serde_json::from_slice(&wire).unwrap();
    assert_eq!(artifact, decoded);
    artifact
}
fn diamond() -> Input {
    let mut a = Asm::default();
    a.op(0x5f)
        .op(0x35)
        .target("then")
        .op(0x57)
        .push(3)
        .target("join")
        .op(0x56)
        .dest("then")
        .push(7)
        .dest("join")
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    a.finish()
}
fn loop_fixture() -> Input {
    let mut a = Asm::default();
    a.op(0x5f)
        .dest("head")
        .op(0x80)
        .push(4)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x50)
        .op(0x00)
        .dest("body")
        .push(1)
        .op(0x01)
        .op(0x80)
        .op(0x5f)
        .op(0x55)
        .target("head")
        .op(0x56);
    a.finish()
}
fn irreducible() -> Input {
    let mut a = Asm::default();
    a.op(0x5f).op(0x35).target("b").op(0x57).dest("a");
    increment(&mut a);
    a.target("b").op(0x57).target("end").op(0x56).dest("b");
    increment(&mut a);
    a.target("a")
        .op(0x57)
        .target("end")
        .op(0x56)
        .dest("end")
        .op(0x00);
    a.finish()
}
fn increment(a: &mut Asm) {
    a.op(0x5f)
        .op(0x54)
        .push(1)
        .op(0x01)
        .op(0x80)
        .op(0x5f)
        .op(0x55)
        .push(4)
        .op(0x90)
        .op(0x10);
}
#[test]
fn diamonds_have_executable_if_and_parallel_phi() {
    let input = diamond();
    for value in [0, 1] {
        let a = check(
            &input,
            &Config::default(),
            MemoryHost::new(&input, calldata(value)),
        );
        let c = counts(&a.current().unwrap().ir.structured.as_ref().unwrap().root);
        assert!(c.0 > 0);
        assert_eq!(c.2, 0);
        assert!(
            baseline(&a)
                .ir
                .blocks
                .iter()
                .any(|b| !b.parameters.is_empty())
        );
        assert!(render_text(&a).contains("if ("));
    }
}
#[test]
fn natural_loop_converges_with_stable_values_and_useful_while() {
    let input = loop_fixture();
    let a = check(
        &input,
        &Config::default(),
        MemoryHost::new(&input, Vec::new()),
    );
    let c = counts(&a.current().unwrap().ir.structured.as_ref().unwrap().root);
    assert!(c.1 > 0);
    assert_eq!(c.2, 0);
    assert!(a.statistics.values < 200);
    assert!(a.statistics.evaluations > baseline(&a).ir.blocks.len());
    assert!(render_text(&a).contains("while ("));
}
#[test]
fn backedge_phi_assignments_are_simultaneous_for_loop_swaps() {
    let mut a = Asm::default();
    a.push(1)
        .push(2)
        .op(0x5f)
        .dest("head")
        .op(0x80)
        .push(3)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x50)
        .op(0x5f)
        .op(0x55)
        .push(1)
        .op(0x55)
        .op(0x00)
        .dest("body")
        .push(1)
        .op(0x01)
        .op(0x91)
        .op(0x90)
        .op(0x91)
        .target("head")
        .op(0x56);
    let input = a.finish();
    check(
        &input,
        &Config::default(),
        MemoryHost::new(&input, Vec::new()),
    );
}
#[test]
fn dispatcher_is_a_real_state_machine_only_for_residual_control() {
    let input = irreducible();
    for value in [0, 1] {
        let a = check(
            &input,
            &Config::default(),
            MemoryHost::new(&input, calldata(value)),
        );
        assert!(counts(&a.current().unwrap().ir.structured.as_ref().unwrap().root).2 > 0);
        let event = a
            .events
            .iter()
            .find(|e| e.rule == "dispatcher_lowering")
            .unwrap();
        assert!(!event.created.is_empty());
        assert!(
            a.mappings
                .origins
                .iter()
                .filter(|o| event.created.contains(&o.entity.entity))
                .all(|o| !o.reason.is_empty())
        );
    }
}
#[test]
fn code_duplication_changes_graph_and_remaps_effectful_bodies_and_values() {
    let input = irreducible();
    let config = Config {
        strategy: Strategy::CodeDuplication,
        ..Config::default()
    };
    for value in [0, 1] {
        let a = check(&input, &config, MemoryHost::new(&input, calldata(value)));
        assert!(a.statistics.cloned_blocks > 0);
        assert!(a.current().unwrap().ir.blocks.len() > baseline(&a).ir.blocks.len());
        let event = a
            .events
            .iter()
            .find(|e| e.rule == "selective_node_split")
            .unwrap();
        assert!(!event.redirects.is_empty());
        assert!(!event.clones.is_empty());
        assert!(
            event
                .clones
                .iter()
                .all(|c| c.original.entity != c.copy.entity)
        );
        let copies: Vec<_> = event
            .clones
            .iter()
            .filter(|c| {
                a.stage(c.copy.stage)
                    .unwrap()
                    .ir
                    .value(c.copy.entity)
                    .is_some()
            })
            .collect();
        assert!(!copies.is_empty());
        assert!(
            copies
                .iter()
                .all(|c| !a.origin(c.copy).unwrap().source.is_empty())
        );
        for clone in &event.clones {
            assert!(a.derived_from(clone.original).contains(&clone.copy));
        }
        assert!(
            a.diff(event.from_stage.unwrap(), event.to_stage)
                .unwrap()
                .added
                .len()
                > 0
        );
    }
}
#[test]
fn dispatcher_and_duplication_artifact_namespaces_are_distinct_and_stable() {
    let input = irreducible();
    let d = decompile(&input, &Config::default());
    assert_eq!(d, decompile(&input, &Config::default()));
    let mut config = Config {
        strategy: Strategy::CodeDuplication,
        ..Config::default()
    };
    let c = decompile(&input, &config);
    assert_ne!(d.id, c.id);
    config.limits.max_cloned_blocks += 1;
    assert_ne!(c.id, decompile(&input, &config).id);
}
#[test]
fn clone_refusal_keeps_previous_validated_graph_and_reports_strategy_limit() {
    let input = irreducible();
    let mut config = Config {
        strategy: Strategy::CodeDuplication,
        ..Config::default()
    };
    config.limits.max_cloned_blocks = 0;
    let a = decompile(&input, &config);
    assert_eq!(a.completion, Completion::Limited);
    assert_eq!(a.current().unwrap().kind, StageKind::Simplified);
    assert!(a.current().unwrap().ir.structured.is_none());
    assert!(
        a.diagnostics
            .iter()
            .any(|d| d.message.contains("splitting"))
    );
    assert!(a.events.iter().all(|e| e.clones.is_empty()));
}
#[test]
fn shared_internal_returns_recover_finite_dynamic_targets() {
    let mut a = Asm::default();
    a.op(0x5f)
        .op(0x35)
        .target("site2")
        .op(0x57)
        .target("ret1")
        .target("callee")
        .op(0x56)
        .dest("ret1")
        .op(0x5f)
        .op(0x55)
        .op(0x00)
        .dest("site2")
        .target("ret2")
        .target("callee")
        .op(0x56)
        .dest("ret2")
        .op(0x5f)
        .op(0x55)
        .op(0x00)
        .dest("callee")
        .push(42)
        .op(0x90)
        .op(0x56);
    let input = a.finish();
    for value in [0, 1] {
        let artifact = check(
            &input,
            &Config::default(),
            MemoryHost::new(&input, calldata(value)),
        );
        assert!(baseline(&artifact).ir.blocks.iter().any(|b|matches!(&b.terminator,Terminator::Jump {edges,unresolved:false,..}if edges.len()==2)));
    }
}
#[test]
fn shared_pc_with_different_stack_heights_has_stable_separate_contexts() {
    let mut a = Asm::default();
    a.op(0x5f)
        .op(0x35)
        .target("site2")
        .op(0x57)
        .target("ret1")
        .target("callee")
        .op(0x56)
        .dest("ret1")
        .op(0x5f)
        .op(0x55)
        .op(0x00)
        .dest("site2")
        .push(99)
        .target("ret2")
        .target("callee")
        .op(0x56)
        .dest("ret2")
        .op(0x5f)
        .op(0x55)
        .op(0x50)
        .op(0x00)
        .dest("callee")
        .push(42)
        .op(0x90)
        .op(0x56);
    let callee = a.labels["callee"];
    let input = a.finish();
    for value in [0, 1] {
        let artifact = check(
            &input,
            &Config::default(),
            MemoryHost::new(&input, calldata(value)),
        );
        let contexts: Vec<_> = baseline(&artifact)
            .ir
            .blocks
            .iter()
            .filter(|b| b.source.is_some_and(|s| s.pc == callee))
            .collect();
        assert_eq!(contexts.len(), 2);
        assert_ne!(contexts[0].parameters.len(), contexts[1].parameters.len());
        assert_ne!(contexts[0].operations[1].id, contexts[1].operations[1].id);
    }
}
#[test]
fn unknown_jump_is_partial_and_known_fallthrough_still_structures() {
    // calldata[0] condition, calldata[32] target; false side is a useful diamond.
    let mut a = Asm::default();
    a.push(32)
        .op(0x35)
        .op(0x5f)
        .op(0x35)
        .op(0x90)
        .op(0x57)
        .op(0x5f)
        .op(0x35)
        .target("then")
        .op(0x57)
        .push(1)
        .target("end")
        .op(0x56)
        .dest("then")
        .push(2)
        .dest("end")
        .op(0x50)
        .op(0x00);
    let input = a.finish();
    let artifact = decompile(&input, &Config::default());
    assert_eq!(artifact.completion, Completion::Partial);
    assert_eq!(
        artifact.current().unwrap().ir.fact_scope,
        FactScope::RecoveredPaths
    );
    assert!(
        counts(
            &artifact
                .current()
                .unwrap()
                .ir
                .structured
                .as_ref()
                .unwrap()
                .root
        )
        .0 > 0
    );
    let mut host = MemoryHost::new(&input, calldata(1));
    let execution = execute_structured(
        artifact.current().unwrap(),
        &mut host,
        &ExecutionLimits::default(),
    )
    .unwrap();
    assert!(matches!(
        execution.outcome,
        ExecutionOutcome::PartialJump { .. }
    ));
}
#[test]
fn unresolved_reentry_does_not_certify_conditional_facts_or_fold_them() {
    let mut a = Asm::default();
    a.op(0x5f)
        .op(0x35)
        .target("unknown")
        .op(0x57)
        .push(1)
        .target("join")
        .op(0x56)
        .dest("unknown")
        .push(99)
        .push(32)
        .op(0x35)
        .op(0x56)
        .dest("join")
        .push(1)
        .op(0x01)
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    let input = a.finish();
    let artifact = decompile(&input, &Config::default());
    assert_eq!(artifact.completion, Completion::Partial);
    let simplified = artifact
        .stages
        .iter()
        .find(|s| s.kind == StageKind::Simplified)
        .unwrap();
    assert_eq!(simplified.ir.fact_scope, FactScope::RecoveredPaths);
    assert!(
        simplified
            .ir
            .blocks
            .iter()
            .flat_map(|b| &b.operations)
            .all(|o| o.replacement.is_none())
    );
    assert!(
        artifact
            .events
            .iter()
            .any(|e| e.rule == "preserve_open_facts")
    );
}
#[test]
fn memory_merges_drop_conflicting_stores_and_overlapping_byte_writes() {
    let mut a = Asm::default();
    a.push(7)
        .op(0x5f)
        .op(0x52)
        .op(0x5f)
        .op(0x35)
        .target("change")
        .op(0x57)
        .target("join")
        .op(0x56)
        .dest("change")
        .push(9)
        .op(0x5f)
        .op(0x52)
        .dest("join")
        .op(0x5f)
        .op(0x51)
        .op(0x5f)
        .op(0x55)
        .push(255)
        .push(31)
        .op(0x53)
        .op(0x5f)
        .op(0x51)
        .push(1)
        .op(0x55)
        .op(0x00);
    let input = a.finish();
    for value in [0, 1] {
        let artifact = check(
            &input,
            &Config::default(),
            MemoryHost::new(&input, calldata(value)),
        );
        let loads: Vec<_> = baseline(&artifact)
            .ir
            .blocks
            .iter()
            .flat_map(|b| &b.operations)
            .filter(|o| o.opcode == 0x51)
            .collect();
        assert_eq!(loads.len(), 2);
        assert!(loads.iter().all(|o| o.memory_input.is_some()));
        assert!(loads.iter().all(
            |o| baseline(&artifact).ir.value(o.outputs[0]).unwrap().fact == ValueFact::Unknown
        ));
    }
}
#[test]
fn external_call_results_returndata_output_memory_and_storage_are_explicit() {
    // Store 7 at memory[0], call overwrites 32 output bytes and storage[0].
    let input = Input::new(
        vec![
            0x60, 7, 0x5f, 0x52, 0x5f, 0x54, 0x50, 0x60, 32, 0x5f, 0x5f, 0x5f, 0x5f, 0x60, 1, 0x60,
            0xff, 0xf1, 0x60, 2, 0x55, 0x5f, 0x51, 0x60, 3, 0x55, 0x5f, 0x54, 0x60, 4, 0x55, 0x3d,
            0x60, 5, 0x55, 0x00,
        ],
        Fork::Osaka,
    );
    for success in [0, 1] {
        let mut host = MemoryHost::new(&input, Vec::new());
        host.responses.push_back(ExternalResponse {
            result: U256::from(success),
            return_data: U256::from(99).to_be_bytes::<32>().to_vec(),
            storage_writes: BTreeMap::from([(U256::ZERO, U256::from(42))]),
        });
        let artifact = check(&input, &Config::default(), host);
        let ops: Vec<_> = baseline(&artifact)
            .ir
            .blocks
            .iter()
            .flat_map(|b| &b.operations)
            .collect();
        let call = ops.iter().find(|o| o.opcode == 0xf1).unwrap();
        assert!(
            call.effects.call
                && call.effects.memory_write
                && call.effects.return_data_write
                && call.effects.storage_write
        );
        assert!(
            call.effect_input.is_some()
                && call.effect_output.is_some()
                && call.memory_output.is_some()
        );
        assert!(ops.iter().filter(|o| o.opcode == 0x54).all(|o| {
            baseline(&artifact).ir.value(o.outputs[0]).unwrap().fact == ValueFact::Unknown
        }));
    }
}
#[test]
fn logs_and_transient_effects_preserve_order_across_structuring() {
    let input = Input::new(
        vec![
            0x60, 9, 0x5f, 0x5d, 0x5f, 0x5c, 0x5f, 0x52, 0x60, 32, 0x5f, 0xa0, 0x60, 32, 0x5f, 0xf3,
        ],
        Fork::Osaka,
    );
    let a = check(
        &input,
        &Config::default(),
        MemoryHost::new(&input, Vec::new()),
    );
    assert!(
        a.current()
            .unwrap()
            .validation
            .checks
            .iter()
            .any(|c| c.name == "observable_effect_order" && c.passed)
    );
}
#[test]
fn invalid_jumps_false_falloff_truncated_push_and_stack_faults_are_explicit() {
    for bytes in [
        vec![0x60, 1, 0x56],
        vec![0x5f, 0x60, 255, 0x57],
        vec![0x61, 0xab],
        vec![0x01],
        vec![0xfe],
    ] {
        let input = Input::new(bytes, Fork::Osaka);
        check(
            &input,
            &Config::default(),
            MemoryHost::new(&input, Vec::new()),
        );
    }
}
#[test]
fn codesize_retains_unreachable_trailing_bytes() {
    let input = Input::new(vec![0x38, 0x5f, 0x55, 0x00, 0x5b, 0x60, 0x55], Fork::Osaka);
    check(
        &input,
        &Config::default(),
        MemoryHost::new(&input, Vec::new()),
    );
}
#[test]
fn constants_results_parameters_and_copies_have_bidirectional_source_origins() {
    let input = diamond();
    let a = decompile(&input, &Config::default());
    for stage in a.stages.iter().filter(|s| s.kind != StageKind::Decoded) {
        for value in &stage.ir.values {
            let reference = EntityRef {
                stage: stage.id,
                entity: value.id,
            };
            let origin = a.origin(reference).unwrap();
            assert!(!origin.source.is_empty(), "{value:?}");
            assert!(
                origin
                    .source
                    .iter()
                    .any(|s| a.descendants(s.pc).contains(&reference))
            );
        }
    }
    let p = Input::new(vec![0x60, 2, 0x60, 3, 0x01, 0x00], Fork::Osaka);
    let a = decompile(&p, &Config::default());
    let stage = a.current().unwrap();
    let add = stage.ir.blocks[0]
        .operations
        .iter()
        .find(|o| o.opcode == 0x01)
        .unwrap();
    assert_eq!(add.replacement, Some(U256::from(5)));
    assert!(
        a.origin(EntityRef {
            stage: stage.id,
            entity: add.outputs[0]
        })
        .unwrap()
        .source
        .iter()
        .any(|s| s.pc == 4)
    );
}
#[test]
fn cancellation_limits_and_history_refusal_preserve_atomic_stages() {
    let input = loop_fixture();
    let cancel = Cancellation::new();
    cancel.cancel();
    let a = decompile_with_cancel(&input, &Config::default(), &cancel);
    assert_eq!(a.completion, Completion::Cancelled);
    assert!(a.stages.is_empty());
    let mut config = Config::default();
    config.limits.max_work = 0;
    assert_eq!(decompile(&input, &config).completion, Completion::Limited);
    config = Config::default();
    config.limits.max_iterations = 1;
    let a = decompile(&input, &config);
    assert_eq!(a.completion, Completion::Limited);
    assert_eq!(a.current().unwrap().kind, StageKind::Decoded);
    config = Config::default();
    config.limits.max_history_entities = 1;
    let a = decompile(&input, &config);
    assert_eq!(a.completion, Completion::Limited);
    assert!(a.stages.is_empty() && a.events.is_empty() && a.mappings.origins.is_empty());
    let cancel = Cancellation::new();
    let mut seen = Vec::new();
    let a = decompile_with_progress(&input, &Config::default(), &cancel, |p| {
        if p.phase == Phase::Structure {
            cancel.cancel();
        }
        seen.push(p);
    });
    assert_eq!(a.completion, Completion::Cancelled);
    assert!(a.current().unwrap().ir.structured.is_none());
    assert!(seen.windows(2).all(|p| p[0].work <= p[1].work
        && p[0].blocks <= p[1].blocks
        && p[0].operations <= p[1].operations));
    assert!(seen.iter().any(|p| p.phase == Phase::Validate));
    assert!(seen.iter().any(|p| p.phase == Phase::Provenance));
}
#[test]
fn cancelled_provenance_does_not_publish_half_an_event() {
    let input = diamond();
    let cancel = Cancellation::new();
    let a = decompile_with_progress(&input, &Config::default(), &cancel, |p| {
        if p.phase == Phase::Provenance {
            cancel.cancel();
        }
    });
    assert_eq!(a.completion, Completion::Cancelled);
    assert!(a.stages.is_empty() && a.events.is_empty() && a.mappings.origins.is_empty());
}
#[test]
fn nested_structured_wire_roundtrip_and_depth_refusal_are_explicit() {
    let mut a = Asm::default();
    for (i, label) in ["one", "two", "three", "four", "five", "six"]
        .iter()
        .enumerate()
    {
        a.op(0x5f)
            .op(0x35)
            .target(label)
            .op(0x57)
            .push(i as u8)
            .op(0x50);
        a.dest(label);
    }
    a.op(0x00);
    let input = a.finish();
    check(
        &input,
        &Config::default(),
        MemoryHost::new(&input, calldata(1)),
    );
    let mut config = Config::default();
    config.limits.max_structuring_depth = 0;
    let artifact = decompile(&input, &config);
    assert_eq!(artifact.completion, Completion::Limited);
    assert!(artifact.current().unwrap().ir.structured.is_none());
    let wire = serde_json::to_vec(&artifact).unwrap();
    let _: Artifact = serde_json::from_slice(&wire).unwrap();
}
