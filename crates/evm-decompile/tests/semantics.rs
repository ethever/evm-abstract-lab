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
        self.bytes.push(0x61);
        self.fixups.push((self.bytes.len(), name));
        self.bytes.extend([0, 0]);
        self
    }
    fn finish(mut self) -> Input {
        for (offset, name) in self.fixups {
            self.bytes[offset..offset + 2].copy_from_slice(
                &u16::try_from(self.labels[name])
                    .expect("small fixture")
                    .to_be_bytes(),
            );
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
fn nested_natural_loops_preserve_inner_and_outer_effects() {
    let mut a = Asm::default();
    a.op(0x5f)
        .dest("outer")
        .op(0x80)
        .push(2)
        .op(0x90)
        .op(0x10)
        .op(0x15)
        .target("end")
        .op(0x57)
        .op(0x5f)
        .dest("inner")
        .op(0x80)
        .push(3)
        .op(0x90)
        .op(0x10)
        .op(0x15)
        .target("inner_end")
        .op(0x57)
        .push(1)
        .op(0x01)
        .op(0x80)
        .push(1)
        .op(0x55)
        .target("inner")
        .op(0x56)
        .dest("inner_end")
        .op(0x50)
        .push(1)
        .op(0x01)
        .op(0x80)
        .op(0x5f)
        .op(0x55)
        .target("outer")
        .op(0x56)
        .dest("end")
        .op(0x50)
        .op(0x00);
    let input = a.finish();
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = check(
            &input,
            &Config {
                strategy,
                ..Config::default()
            },
            MemoryHost::new(&input, Vec::new()),
        );
        let ast = &artifact
            .current()
            .unwrap()
            .ir
            .structured
            .as_ref()
            .unwrap()
            .root;
        let (_, loops, dispatchers) = counts(ast);
        assert!(loops >= 2);
        assert_eq!(dispatchers, 0);
        let mut host = MemoryHost::new(&input, Vec::new());
        execute_structured(
            artifact.current().unwrap(),
            &mut host,
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(host.storage.get(&U256::ZERO), Some(&U256::from(2)));
        assert_eq!(host.storage.get(&U256::from(1)), Some(&U256::from(3)));
    }
}

#[test]
fn deep_recovered_loop_regions_use_flat_wire_without_json_depth_refusal() {
    let heads = [
        "h00", "h01", "h02", "h03", "h04", "h05", "h06", "h07", "h08", "h09", "h10", "h11", "h12",
        "h13", "h14", "h15", "h16", "h17", "h18", "h19", "h20", "h21", "h22", "h23", "h24",
    ];
    let ends = [
        "e00", "e01", "e02", "e03", "e04", "e05", "e06", "e07", "e08", "e09", "e10", "e11", "e12",
        "e13", "e14", "e15", "e16", "e17", "e18", "e19", "e20", "e21", "e22", "e23", "e24",
    ];
    let mut a = Asm::default();
    for (head, end) in heads.iter().zip(&ends) {
        a.op(0x5f)
            .dest(head)
            .op(0x80)
            .push(1)
            .op(0x90)
            .op(0x10)
            .op(0x15)
            .target(end)
            .op(0x57);
    }
    a.push(42).op(0x5f).op(0x55);
    for (head, end) in heads.iter().zip(&ends).rev() {
        a.push(1).op(0x01).target(head).op(0x56).dest(end).op(0x50);
    }
    a.op(0x00);
    let input = a.finish();
    let mut config = Config::default();
    config.limits.max_structuring_depth = 512;
    let artifact = check(&input, &config, MemoryHost::new(&input, Vec::new()));
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
        .1 >= 25
    );
}

#[test]
fn structured_text_prints_one_terminal_and_readable_partial_and_fault_exits() {
    for (bytes, terminal) in [
        (vec![0x00], "stop("),
        (vec![0x5f, 0x5f, 0xf3], "return("),
        (vec![0x5f, 0x5f, 0xfd], "revert("),
    ] {
        let input = Input::new(bytes, Fork::Osaka);
        let artifact = decompile(&input, &Config::default());
        let text = render_text(&artifact);
        assert_eq!(text.matches(terminal).count(), 1, "{text}");
        assert!(
            !text.contains("Exit {") && !text.contains("exit Exit"),
            "{text}"
        );
    }
    let fault = decompile(&Input::new(vec![0x01], Fork::Osaka), &Config::default());
    let text = render_text(&fault);
    assert!(text.contains("trap(\"EVM stack underflow\")"), "{text}");
    assert!(!text.contains("add("));
    let unknown = decompile(
        &Input::new(vec![0x5f, 0x35, 0x56], Fork::Osaka),
        &Config::default(),
    );
    assert!(render_text(&unknown).contains("partial_indirect_exit"));
    let invalid = decompile(
        &Input::new(vec![0x60, 0xff, 0x56], Fork::Osaka),
        &Config::default(),
    );
    assert!(render_text(&invalid).contains("invalid_jump("));
    let mut refusal = fault;
    refusal.config.limits.max_work = 0;
    assert!(render_text(&refusal).contains("export cancelled or work limit exhausted"));
    let cancel = Cancellation::new();
    cancel.cancel();
    assert!(evm_decompile::render_text_with_cancel(&unknown, &cancel).contains("export cancelled"));
    let mut config = Config::default();
    config.limits.max_iterations = 0;
    let source_only = decompile(&Input::new(vec![0x60, 9, 0x00], Fork::Osaka), &config);
    assert!(render_text(&source_only).contains("PUSH1 0x09"));
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
        let mut dispatches = Vec::new();
        fn collect<'a>(
            node: &'a StructuredNode,
            out: &mut Vec<(evm_decompile::EntityId, &'a [evm_decompile::DispatchCase])>,
        ) {
            match &node.kind {
                StructuredKind::Dispatcher { cases, .. } => out.push((node.id, cases)),
                StructuredKind::Sequence(nodes) => {
                    for node in nodes {
                        collect(node, out);
                    }
                }
                StructuredKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    collect(then_branch, out);
                    collect(else_branch, out);
                }
                StructuredKind::While { header, body, .. } => {
                    collect(header, out);
                    collect(body, out);
                }
                StructuredKind::Indirect { arms, .. } => {
                    for arm in arms {
                        collect(&arm.body, out);
                    }
                }
                _ => {}
            }
        }
        collect(
            &a.current().unwrap().ir.structured.as_ref().unwrap().root,
            &mut dispatches,
        );
        let text = render_text(&a);
        let mut escaped = 0;
        for (id, cases) in dispatches {
            for transition in cases.iter().flat_map(|case| &case.transitions) {
                if !cases.iter().any(|case| case.block == transition.target) {
                    let exit = format!(
                        "state_d{} = b{}; break 'dispatch{}; // edge{}",
                        id.0, transition.target.0, id.0, transition.edge.0
                    );
                    assert!(text.contains(&exit), "{text}");
                    escaped += 1;
                }
            }
        }
        assert!(
            escaped > 0,
            "the irreducible fixture must exercise region exits"
        );
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
        assert_eq!(
            counts(&a.current().unwrap().ir.structured.as_ref().unwrap().root).2,
            0
        );
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
            !a.diff(event.from_stage.unwrap(), event.to_stage)
                .unwrap()
                .added
                .is_empty()
        );
    }
}

#[test]
fn three_entry_irreducible_region_preserves_effects_and_omits_unused_copies() {
    let mut a = Asm::default();
    a.op(0x5f)
        .op(0x35)
        .op(0x15)
        .target("a")
        .op(0x57)
        .op(0x5f)
        .op(0x35)
        .push(1)
        .op(0x14)
        .target("b")
        .op(0x57)
        .target("c")
        .op(0x56)
        .dest("a");
    increment(&mut a);
    a.target("b").op(0x57).target("end").op(0x56).dest("b");
    increment(&mut a);
    a.target("c").op(0x57).target("end").op(0x56).dest("c");
    increment(&mut a);
    a.target("a")
        .op(0x57)
        .target("end")
        .op(0x56)
        .dest("end")
        .op(0x00);
    let input = a.finish();
    for selector in [0, 1, 2] {
        for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
            let artifact = check(
                &input,
                &Config {
                    strategy,
                    ..Config::default()
                },
                MemoryHost::new(&input, calldata(selector)),
            );
            if strategy == Strategy::CodeDuplication {
                assert!(artifact.statistics.cloned_blocks >= 3);
            }
        }
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
    let original = decompile(&input, &Config::default());
    let decoded = &original.stages[0];
    let mut bounded = Config::default();
    bounded.limits.max_operations = decoded.ir.blocks.iter().map(|b| b.operations.len()).sum();
    let limited = decompile(&input, &bounded);
    assert_eq!(limited.completion, Completion::Limited);
    assert_eq!(limited.current().unwrap().kind, StageKind::Decoded);
    assert!(
        limited
            .diagnostics
            .iter()
            .any(|d| d.message.contains("context operation limit"))
    );
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
fn widening_jump_repropagates_stack_and_memory_to_retained_targets() {
    // FIFO reaches source->known before a longer path contributes an unknown
    // target and changed carried values. The concrete unknown target is known.
    let mut a = Asm::default();
    a.op(0x5f)
        .op(0x35)
        .target("early")
        .op(0x57)
        .target("late1")
        .op(0x56)
        .dest("early")
        .op(0x5f)
        .op(0x5f)
        .op(0x52)
        .op(0x5f)
        .target("known")
        .target("source")
        .op(0x56)
        .dest("late1")
        .target("late2")
        .op(0x56)
        .dest("late2")
        .push(1)
        .op(0x5f)
        .op(0x52)
        .push(1)
        .push(32)
        .op(0x35)
        .target("source")
        .op(0x56)
        .dest("source")
        .op(0x56)
        .dest("known")
        .op(0x80)
        .op(0x5f)
        .op(0x51)
        .op(0x50)
        .target("true")
        .op(0x57)
        .op(0x50)
        .push(11)
        .op(0x5f)
        .op(0x55)
        .op(0x00)
        .dest("true")
        .op(0x50)
        .push(22)
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    let known_pc = a.labels["known"];
    let input = a.finish();
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = decompile(
            &input,
            &Config {
                strategy,
                ..Config::default()
            },
        );
        assert_eq!(
            artifact.completion,
            Completion::Partial,
            "{:?}",
            artifact.diagnostics
        );
        assert!(
            !artifact
                .diagnostics
                .iter()
                .any(|d| d.code == evm_decompile::DiagnosticCode::Validation)
        );
        let base = baseline(&artifact);
        let known = base
            .ir
            .blocks
            .iter()
            .find(|b| b.source.is_some_and(|s| s.pc == known_pc))
            .unwrap();
        let carrier = base.ir.value(known.parameters[0]).unwrap();
        assert_eq!(
            carrier.fact,
            ValueFact::Constants(vec![U256::ZERO, U256::from(1)])
        );
        let load = known
            .operations
            .iter()
            .find(|op| op.opcode == 0x51)
            .unwrap();
        assert_eq!(
            base.ir.value(load.outputs[0]).unwrap().fact,
            ValueFact::Unknown
        );
        assert!(
            matches!(&known.terminator, Terminator::Branch { then_edges, else_edge: Some(_), .. } if !then_edges.is_empty())
        );
        for selector in [0, 1] {
            let mut data = calldata(selector);
            data.extend(U256::from(known_pc).to_be_bytes::<32>());
            let host = MemoryHost::new(&input, data);
            let mut raw_host = host.clone();
            let raw = execute_bytecode(&input, &mut raw_host, &ExecutionLimits::default()).unwrap();
            for stage in artifact
                .stages
                .iter()
                .filter(|s| s.kind != StageKind::Decoded)
            {
                let mut graph_host = host.clone();
                let graph =
                    execute_ir(stage, &mut graph_host, &ExecutionLimits::default()).unwrap();
                assert_eq!(raw.outcome, graph.outcome);
                assert_eq!(raw.observations(), graph.observations());
                assert_eq!(raw_host, graph_host);
                if stage.ir.structured.is_some() {
                    let mut syntax_host = host.clone();
                    let syntax =
                        execute_structured(stage, &mut syntax_host, &ExecutionLimits::default())
                            .unwrap();
                    assert_eq!(raw.outcome, syntax.outcome);
                    assert_eq!(raw.observations(), syntax.observations());
                    assert_eq!(raw_host, syntax_host);
                }
            }
        }
    }
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
        let artifact = check(&input, &Config::default(), host.clone());
        let mut observed = host;
        execute_structured(
            artifact.current().unwrap(),
            &mut observed,
            &ExecutionLimits::default(),
        )
        .unwrap();
        assert_eq!(
            observed
                .storage
                .get(&U256::ZERO)
                .copied()
                .unwrap_or_default(),
            U256::from(if success == 1 { 42 } else { 0 })
        );
        assert_eq!(observed.storage[&U256::from(2)], U256::from(success));
        assert_eq!(observed.storage[&U256::from(3)], U256::from(99));
        assert_eq!(
            observed.storage[&U256::from(4)],
            U256::from(if success == 1 { 42 } else { 0 })
        );
        assert_eq!(observed.storage[&U256::from(5)], U256::from(32));
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
fn failed_create_discards_child_writes_and_success_clears_return_data() {
    for opcode in [0xf0, 0xf5] {
        let mut bytes = if opcode == 0xf5 {
            vec![0x60, 9]
        } else {
            Vec::new()
        };
        bytes.extend([
            0x60, 32, 0x5f, 0x5f, opcode, 0x60, 2, 0x55, 0x3d, 0x60, 3, 0x55, 0x5f, 0x54, 0x60, 4,
            0x55, 0x00,
        ]);
        let input = Input::new(bytes, Fork::Osaka);
        for success in [false, true] {
            let mut host = MemoryHost::new(&input, Vec::new());
            host.responses.push_back(ExternalResponse {
                result: U256::from(if success { 7 } else { 0 }),
                return_data: vec![99; 32],
                storage_writes: BTreeMap::from([(U256::ZERO, U256::from(42))]),
            });
            let artifact = check(&input, &Config::default(), host.clone());
            execute_structured(
                artifact.current().unwrap(),
                &mut host,
                &ExecutionLimits::default(),
            )
            .unwrap();
            assert_eq!(
                host.storage[&U256::from(2)],
                U256::from(if success { 7 } else { 0 })
            );
            assert_eq!(
                host.storage[&U256::from(3)],
                U256::from(if success { 0 } else { 32 })
            );
            assert_eq!(
                host.storage[&U256::from(4)],
                U256::from(if success { 42 } else { 0 })
            );
        }
    }
}

#[test]
fn faulting_context_copy_keeps_only_real_clone_and_diagnostic_occurrences() {
    let input = Input::new(
        vec![
            0x5f, 0x35, 0x60, 0x0a, 0x57, 0x60, 9, 0x60, 0x12, 0x56, 0x5b, 0x60, 2, 0x60, 3, 0x60,
            0x12, 0x56, 0x5b, 0x01, 0x5f, 0x55, 0x00,
        ],
        Fork::Osaka,
    );
    for value in [0, 1] {
        for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
            let artifact = check(
                &input,
                &Config {
                    strategy,
                    ..Config::default()
                },
                MemoryHost::new(&input, calldata(value)),
            );
            let entities: std::collections::BTreeSet<_> = artifact
                .stages
                .iter()
                .flat_map(|stage| {
                    evm_decompile::records(&stage.ir)
                        .keys()
                        .map(|entity| EntityRef {
                            stage: stage.id,
                            entity: *entity,
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            let origins: std::collections::BTreeSet<_> =
                artifact.mappings.origins.iter().map(|o| o.entity).collect();
            assert_eq!(entities, origins);
            assert!(artifact.events.iter().any(|event| !event.clones.is_empty()));
            for event in &artifact.events {
                for relation in &event.clones {
                    assert!(entities.contains(&relation.original));
                    assert!(entities.contains(&relation.copy));
                }
                assert!(
                    event
                        .inputs
                        .iter()
                        .chain(&event.outputs)
                        .all(|entity| entities.contains(entity))
                );
            }
            for diagnostic in &artifact.diagnostics {
                if let Some(entity) = diagnostic.entity {
                    assert!(entities.contains(&EntityRef {
                        stage: diagnostic.stage.expect("retained diagnostic stage"),
                        entity
                    }));
                }
            }
            assert!(
                artifact
                    .mappings
                    .origins
                    .iter()
                    .flat_map(|o| &o.parents)
                    .all(|parent| entities.contains(parent))
            );
        }
    }
}

#[test]
fn overflow_and_state_write_underflow_render_faults_without_fake_results() {
    let mut overflow = vec![0x5f; 1025];
    overflow.push(0x00);
    let input = Input::new(overflow, Fork::Osaka);
    let artifact = check(
        &input,
        &Config::default(),
        MemoryHost::new(&input, Vec::new()),
    );
    let text = render_text(&artifact);
    assert!(text.contains("EVM stack overflow"));
    assert_eq!(text.matches(" = ").count(), 1024);
    let fault = baseline(&artifact).ir.blocks[0].operations.last().unwrap();
    assert!(
        fault
            .outputs
            .iter()
            .all(|id| baseline(&artifact).ir.value(*id).unwrap().fact == ValueFact::Bottom)
    );
    let input = Input::new(vec![0x60, 7, 0x55], Fork::Osaka);
    let artifact = check(
        &input,
        &Config::default(),
        MemoryHost::new(&input, Vec::new()),
    );
    let text = render_text(&artifact);
    assert!(text.contains("0x7") && text.contains("EVM stack underflow"));
    assert!(!text.contains("sstore("));
}

#[test]
fn export_is_bounded_and_can_cancel_during_a_large_primitive_body() {
    let mut bytes = [0x5f, 0x50].repeat(4096);
    bytes.push(0x00);
    let artifact = decompile(&Input::new(bytes, Fork::Osaka), &Config::default());
    assert_eq!(artifact.completion, Completion::Complete);
    let cancel = Cancellation::new();
    let complete = evm_decompile::export_text(&artifact, &cancel);
    assert_eq!(complete.status, evm_decompile::TextExportStatus::Complete);
    assert!(complete.work > 0);
    let refused = evm_decompile::export_text_with_observer(&artifact, &cancel, 1, |_| {});
    assert_eq!(refused.status, evm_decompile::TextExportStatus::Limited);
    assert!(refused.work <= 1);
    assert!(refused.text.contains("work limit exhausted"));
    let mut callbacks = 0;
    let interrupted =
        evm_decompile::export_text_with_observer(&artifact, &cancel, usize::MAX, |_| {
            callbacks += 1;
            if callbacks == 64 {
                cancel.cancel();
            }
        });
    assert!(cancel.is_cancelled());
    assert_eq!(
        interrupted.status,
        evm_decompile::TextExportStatus::Cancelled
    );
    assert!(interrupted.work > 0);
    assert!(interrupted.text.contains("export cancelled"));
    let shown = interrupted.text.matches(" = ").count();
    assert!(shown > 0 && shown < 4096);
}

#[test]
fn stage_validation_rejects_duplicate_definitions_bad_scope_and_missing_ast_bodies() {
    let artifact = decompile(&diamond(), &Config::default());
    let base = baseline(&artifact);
    let positions: Vec<_> = base
        .ir
        .blocks
        .iter()
        .enumerate()
        .flat_map(|(bi, b)| {
            b.operations
                .iter()
                .enumerate()
                .filter(|(_, op)| !op.outputs.is_empty())
                .map(move |(oi, op)| (bi, oi, op.outputs[0]))
        })
        .collect();
    let mut duplicate = base.clone();
    duplicate.ir.blocks[positions[1].0].operations[positions[1].1].outputs[0] = positions[0].2;
    let validation = evm_decompile::validate_stage(&duplicate);
    assert_eq!(validation.status, ValidationStatus::Failed);
    assert!(
        validation
            .checks
            .iter()
            .any(|c| c.name == "ssa_definition_ownership" && !c.passed)
    );
    let mut scope = base.clone();
    scope
        .ir
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.operations)
        .find(|op| op.opcode == 0x55)
        .unwrap()
        .inputs[0] = evm_decompile::EntityId(u64::MAX);
    let validation = evm_decompile::validate_stage(&scope);
    assert!(
        validation
            .checks
            .iter()
            .any(|c| c.name == "ssa_block_scope" && !c.passed)
    );
    let mut missing = artifact.current().unwrap().clone();
    missing.ir.structured.as_mut().unwrap().root.kind = StructuredKind::Sequence(Vec::new());
    let validation = evm_decompile::validate_stage(&missing);
    assert!(
        validation
            .checks
            .iter()
            .any(|c| c.name == "structured_references_and_coverage" && !c.passed)
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
