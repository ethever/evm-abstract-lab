//! Return-address correlations are checked by source PC and concrete execution.
use evm_bytecode::Program;
use evm_decompile::{
    Artifact, Block, Completion, Config, DiagnosticCode, EntityRef, Execution, ExecutionLimits,
    ExecutionOutcome, ExitKind, ExternalResponse, FactScope, Fork, Input, MemoryHost, Stage,
    StageKind, Strategy, StructuredKind, Terminator, U256, ValidationStatus, ValueFact, decompile,
    execute_bytecode, execute_ir, execute_structured,
};
use std::collections::{BTreeMap, BTreeSet};

const USDC: &str =
    include_str!("../../../examples/known-recursion/captured/usdc-26180958-observed-world.json");
const ADMIN_SLOT: &str = "10d6a54a4754c8869d6886b5f5d7fbfa5b4522237ea5c60d11bc4e7a1ff9390b";
const IMPLEMENTATION_SLOT: &str =
    "7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c3";

fn word(hex: &str) -> U256 {
    U256::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap()
}

fn proxy() -> Input {
    let capture: serde_json::Value = serde_json::from_str(USDC).unwrap();
    let program = Program::from_hex_with_fork(
        capture["accounts"][1]["code"].as_str().unwrap(),
        Fork::Osaka,
    )
    .unwrap();
    let input = Input::new(program.bytes().to_vec(), Fork::Osaka);
    assert_eq!(input.runtime_bytecode.len(), 2186);
    input
}

fn proxy_host(input: &Input, data: Vec<u8>, admin: bool) -> MemoryHost {
    let capture: serde_json::Value = serde_json::from_str(USDC).unwrap();
    let account = &capture["accounts"][1];
    let mut host = MemoryHost::new(input, data);
    host.storage = account["storage"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (word(key), word(value.as_str().unwrap())))
        .collect();
    let caller = if admin {
        host.storage[&word(ADMIN_SLOT)]
    } else {
        U256::from(0xcafe)
    };
    host.context
        .insert(0x30, word(account["address"].as_str().unwrap()));
    host.context.insert(0x33, caller);
    host.context.insert(0x3b, U256::from(1));
    host.context.insert(0x5a, U256::from(1_000_000));
    host
}

fn calldata(value: u64) -> Vec<u8> {
    U256::from(value).to_be_bytes::<32>().to_vec()
}

fn call(selector: [u8; 4], arguments: &[U256]) -> Vec<u8> {
    let mut bytes = selector.to_vec();
    for argument in arguments {
        bytes.extend(argument.to_be_bytes::<32>());
    }
    bytes
}

fn upgrade_and_call(implementation: U256, payload: &[u8]) -> Vec<u8> {
    let mut bytes = call(
        [0x4f, 0x1e, 0xf2, 0x86],
        &[implementation, U256::from(64), U256::from(payload.len())],
    );
    bytes.extend(payload);
    bytes.resize(4 + 96 + payload.len().div_ceil(32) * 32, 0);
    bytes
}

fn baseline(artifact: &Artifact) -> &Stage {
    artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Baseline)
        .expect("recovered baseline")
}

fn at_pc(stage: &Stage, pc: usize) -> Vec<&Block> {
    let blocks: Vec<_> = stage
        .ir
        .blocks
        .iter()
        .filter(|block| block.source.is_some_and(|source| source.pc == pc))
        .collect();
    assert!(!blocks.is_empty(), "missing source PC 0x{pc:x}");
    blocks
}

fn target_pcs(stage: &Stage, block: &Block) -> BTreeSet<usize> {
    block
        .terminator
        .edges()
        .iter()
        .map(|id| {
            let edge = stage.ir.edges.iter().find(|edge| edge.id == *id).unwrap();
            stage.ir.block(edge.to).unwrap().source.unwrap().pc
        })
        .collect()
}

fn exact_returns(stage: &Stage, source_pc: usize, target_pc: usize) {
    for block in at_pc(stage, source_pc) {
        assert!(
            matches!(
                &block.terminator,
                Terminator::Jump { unresolved: false, invalid_targets, .. }
                    if invalid_targets.is_empty()
            ),
            "PC 0x{source_pc:x}: {:?}",
            block.terminator
        );
        assert_eq!(
            target_pcs(stage, block),
            BTreeSet::from([target_pc]),
            "return from PC 0x{source_pc:x}, context {:?}",
            block.id
        );
    }
}

fn validated(artifact: &Artifact) {
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| stage.validation.status == ValidationStatus::Passed),
        "{:?}",
        artifact.diagnostics
    );
    for stage in &artifact.stages {
        for block in &stage.ir.blocks {
            let mut choices = BTreeSet::new();
            for id in block.terminator.edges() {
                let edge = stage.ir.edges.iter().find(|edge| edge.id == id).unwrap();
                let pc = stage.ir.block(edge.to).unwrap().source.unwrap().pc;
                // Executors choose indirect arms by PC. Context copies cannot
                // introduce two competing choices for the same branch role.
                assert!(
                    choices.insert((edge.kind, pc)),
                    "ambiguous target PC 0x{pc:x} from {:?} in {:?}",
                    block.id,
                    stage.kind
                );
            }
        }
    }
    assert!(
        !artifact
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::Validation),
        "{:?}",
        artifact.diagnostics
    );
}

fn complete(artifact: &Artifact) {
    validated(artifact);
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    assert_eq!(artifact.current().unwrap().kind, StageKind::Structured);
}

fn singleton_returns(stage: &Stage, blocks: &[&Block]) {
    for block in blocks {
        let Terminator::Jump {
            target,
            edges,
            unresolved: false,
            invalid_targets,
        } = &block.terminator
        else {
            panic!(
                "expected resolved return at {:?}: {:?}",
                block.source, block.terminator
            );
        };
        assert!(invalid_targets.is_empty());
        assert_eq!(edges.len(), 1, "multiple returns at {:?}", block.source);
        let target_pc = *target_pcs(stage, block).first().unwrap();
        assert_eq!(
            stage.ir.value(*target).unwrap().fact.singleton(),
            Some(U256::from(target_pc)),
            "non-singleton return fact at {:?}",
            block.source
        );
    }
}

fn acyclic_without_dispatcher(artifact: &Artifact) {
    let base = baseline(artifact);
    let mut incoming: BTreeMap<_, _> = base.ir.blocks.iter().map(|block| (block.id, 0)).collect();
    let mut successors = BTreeMap::<_, Vec<_>>::new();
    for edge in &base.ir.edges {
        *incoming.get_mut(&edge.to).unwrap() += 1;
        successors.entry(edge.from).or_default().push(edge.to);
    }
    let mut ready: Vec<_> = incoming
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(*id))
        .collect();
    let mut visited = 0;
    while let Some(id) = ready.pop() {
        visited += 1;
        for target in successors.get(&id).into_iter().flatten() {
            let count = incoming.get_mut(target).unwrap();
            *count -= 1;
            if *count == 0 {
                ready.push(*target);
            }
        }
    }
    assert_eq!(
        visited,
        base.ir.blocks.len(),
        "spurious internal cycle through PCs {:?}",
        base.ir
            .blocks
            .iter()
            .filter(|block| incoming[&block.id] != 0)
            .map(|block| block.source.unwrap().pc)
            .collect::<Vec<_>>()
    );
    for record in evm_decompile::records(&artifact.current().unwrap().ir).values() {
        if let evm_decompile::Record::Node(node) = record {
            assert!(
                !matches!(
                    node.kind,
                    StructuredKind::Dispatcher { .. } | StructuredKind::While { .. }
                ),
                "acyclic internal calls require no loop or dispatcher: {:?}",
                node.id
            );
        }
    }
}

fn provenance(artifact: &Artifact) {
    let mut entities = BTreeSet::new();
    let mut source_entities = BTreeSet::new();
    for stage in &artifact.stages {
        for (entity, record) in evm_decompile::records(&stage.ir) {
            let reference = EntityRef {
                stage: stage.id,
                entity,
            };
            entities.insert(reference);
            if !matches!(
                record,
                evm_decompile::Record::Node(_) | evm_decompile::Record::Transition(_)
            ) {
                source_entities.insert(reference);
            }
        }
    }
    assert_eq!(
        entities,
        artifact
            .mappings
            .origins
            .iter()
            .map(|origin| origin.entity)
            .collect()
    );
    for origin in &artifact.mappings.origins {
        if source_entities.contains(&origin.entity) {
            assert!(!origin.source.is_empty(), "{:?}", origin.entity);
        }
        assert!(
            origin
                .parents
                .iter()
                .all(|parent| entities.contains(parent))
        );
        assert!(origin.source.iter().all(|source| {
            source.pc < source.end_pc && source.end_pc <= artifact.input.runtime_bytecode.len()
        }));
    }
    for event in &artifact.events {
        for relation in &event.clones {
            assert!(entities.contains(&relation.original));
            assert!(entities.contains(&relation.copy));
        }
    }
}

/// Compare every published recovered graph and the final executable syntax.
fn differential(
    input: &Input,
    artifact: &Artifact,
    label: &str,
    host: MemoryHost,
) -> (Execution, MemoryHost) {
    let limits = ExecutionLimits::default();
    let mut raw_host = host.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits).unwrap();
    for stage in artifact
        .stages
        .iter()
        .filter(|stage| stage.kind != StageKind::Decoded)
    {
        let mut graph_host = host.clone();
        let graph = execute_ir(stage, &mut graph_host, &limits)
            .unwrap_or_else(|error| panic!("{label}, {:?}: {error}", stage.kind));
        assert_eq!(raw.outcome, graph.outcome, "{label}, {:?}", stage.kind);
        assert_eq!(
            raw.observations(),
            graph.observations(),
            "{label}, {:?}",
            stage.kind
        );
        assert_eq!(raw_host, graph_host, "{label}, {:?}", stage.kind);
    }
    let current = artifact.current().unwrap();
    assert_eq!(current.kind, StageKind::Structured);
    let mut syntax_host = host;
    let syntax = execute_structured(current, &mut syntax_host, &limits)
        .unwrap_or_else(|error| panic!("{label}, structured: {error}"));
    assert_eq!(raw.outcome, syntax.outcome, "{label}, structured");
    assert_eq!(
        raw.observations(),
        syntax.observations(),
        "{label}, structured"
    );
    assert_eq!(raw_host, syntax_host, "{label}, structured");
    (raw, raw_host)
}

fn exit(execution: &Execution, expected: ExitKind) {
    assert!(
        matches!(execution.outcome, ExecutionOutcome::Exit { kind, .. } if kind == expected),
        "{:?}",
        execution.outcome
    );
}

#[test]
fn captured_usdc_upgrade_return_has_only_its_real_continuation() {
    let input = proxy();
    let artifact = decompile(&input, &Config::default());
    validated(&artifact);
    exact_returns(baseline(&artifact), 0x303, 0x106);
    // The fallback guard is another nested helper whose caller was polluted by
    // the same independent-slot joins; its real return is always 0x201.
    exact_returns(baseline(&artifact), 0x64f, 0x201);
    complete(&artifact);
}

#[test]
fn captured_usdc_literal_helper_returns_are_singleton_and_acyclic() {
    let input = proxy();
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = decompile(
            &input,
            &Config {
                strategy,
                ..Config::default()
            },
        );
        complete(&artifact);
        let base = baseline(&artifact);
        // The proxy forms its internal return targets with literal PUSHes.
        // Check active returns as well as suspended outer continuations.
        singleton_returns(base, &at_pc(base, 0x6a8));
        let jumps: Vec<_> = base
            .ir
            .blocks
            .iter()
            .filter(|block| matches!(block.terminator, Terminator::Jump { .. }))
            .collect();
        singleton_returns(base, &jumps);
        acyclic_without_dispatcher(&artifact);
    }
}

#[test]
fn captured_usdc_upgrade_and_call_effects_match_raw_execution() {
    let input = proxy();
    let implementation = U256::from(0x1234);
    let payload = [0xde, 0xad, 0xbe, 0xef, 0x42];
    let child_slot = U256::from(42);
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = decompile(
            &input,
            &Config {
                strategy,
                ..Config::default()
            },
        );
        validated(&artifact);
        for success in [false, true] {
            let mut host = proxy_host(&input, upgrade_and_call(implementation, &payload), true);
            let proxy_address = host.context[&0x30];
            host.context.insert(0x34, U256::from(7));
            host.responses.push_back(ExternalResponse {
                result: U256::from(u64::from(success)),
                return_data: vec![0x99, 0x88],
                storage_writes: BTreeMap::from([(child_slot, U256::from(77))]),
            });
            let (raw, observed) = differential(
                &input,
                &artifact,
                if success {
                    "upgrade_call_success"
                } else {
                    "upgrade_call_failure"
                },
                host,
            );
            exit(
                &raw,
                if success {
                    ExitKind::Stop
                } else {
                    ExitKind::Revert
                },
            );
            let call = raw.trace.iter().find(|step| step.opcode == 0xf1).unwrap();
            assert_eq!(call.pc, Some(0x2e6));
            assert_eq!(call.inputs[1], proxy_address);
            assert_eq!(call.inputs[2], U256::from(7));
            assert_eq!(call.inputs[4], U256::from(payload.len()));
            let offset = usize::try_from(call.inputs[3]).unwrap();
            assert_eq!(&observed.memory[offset..offset + payload.len()], &payload);
            assert!(observed.responses.is_empty());
            assert_eq!(observed.storage[&word(IMPLEMENTATION_SLOT)], implementation);
            assert_eq!(observed.logs.len(), 1);
            // MemoryHost records primitive effects before enclosing-frame rollback.
            assert_eq!(
                observed.storage.get(&child_slot).copied(),
                success.then_some(U256::from(77))
            );
            let returned = raw.trace.iter().find(|step| step.pc == Some(0x307));
            if success {
                assert_eq!(returned.unwrap().inputs, vec![U256::from(0x106)]);
            } else {
                assert!(returned.is_none());
            }
        }
        let mut invalid = proxy_host(&input, upgrade_and_call(implementation, &payload), true);
        let original = invalid.storage[&word(IMPLEMENTATION_SLOT)];
        invalid.context.insert(0x3b, U256::ZERO);
        let (raw, observed) = differential(&input, &artifact, "upgrade_noncontract", invalid);
        exit(&raw, ExitKind::Revert);
        assert_eq!(observed.storage[&word(IMPLEMENTATION_SLOT)], original);
        assert!(observed.logs.is_empty());
        assert!(raw.trace.iter().all(|step| step.opcode != 0xf1));
    }
}

#[test]
fn captured_usdc_getters_admin_and_fallback_preserve_continuations() {
    let input = proxy();
    let replacement = U256::from(0x1234);
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = decompile(
            &input,
            &Config {
                strategy,
                ..Config::default()
            },
        );
        validated(&artifact);
        for (selector, slot, label) in [
            (
                [0x5c, 0x60, 0xda, 0x1b],
                IMPLEMENTATION_SLOT,
                "implementation",
            ),
            ([0xf8, 0x51, 0xa4, 0x40], ADMIN_SLOT, "admin"),
        ] {
            let host = proxy_host(&input, call(selector, &[]), true);
            let expected = host.storage[&word(slot)];
            let (raw, observed) = differential(&input, &artifact, label, host);
            exit(&raw, ExitKind::Return);
            let ExecutionOutcome::Exit { values, .. } = &raw.outcome else {
                unreachable!()
            };
            assert_eq!(values[1], U256::from(32));
            let offset = usize::try_from(values[0]).unwrap();
            assert_eq!(
                &observed.memory[offset..offset + 32],
                &expected.to_be_bytes::<32>()
            );
            assert!(observed.logs.is_empty());
        }
        for (selector, slot, label) in [
            ([0x36, 0x59, 0xcf, 0xe6], IMPLEMENTATION_SLOT, "upgrade"),
            ([0x8f, 0x28, 0x39, 0x70], ADMIN_SLOT, "change_admin"),
        ] {
            let host = proxy_host(&input, call(selector, &[replacement]), true);
            let (raw, observed) = differential(&input, &artifact, label, host);
            exit(&raw, ExitKind::Stop);
            assert_eq!(observed.storage[&word(slot)], replacement);
            assert_eq!(observed.logs.len(), 1);
        }
        let invalid = proxy_host(&input, call([0x8f, 0x28, 0x39, 0x70], &[U256::ZERO]), true);
        let original = invalid.storage.clone();
        let (raw, observed) = differential(&input, &artifact, "zero_admin", invalid);
        exit(&raw, ExitKind::Revert);
        assert_eq!(observed.storage, original);
        assert!(observed.logs.is_empty());

        for data in [
            vec![0x12, 0x34, 0x56, 0x78, 9],
            vec![1, 2, 3],
            call([0x5c, 0x60, 0xda, 0x1b], &[]),
        ] {
            for success in [false, true] {
                let mut host = proxy_host(&input, data.clone(), false);
                let implementation = host.storage[&word(IMPLEMENTATION_SLOT)];
                let response = vec![0xc0, 0xff, 0xee];
                host.responses.push_back(ExternalResponse {
                    result: U256::from(u64::from(success)),
                    return_data: response.clone(),
                    storage_writes: BTreeMap::from([(U256::from(42), U256::from(77))]),
                });
                let (raw, observed) = differential(&input, &artifact, "nonadmin_fallback", host);
                exit(
                    &raw,
                    if success {
                        ExitKind::Return
                    } else {
                        ExitKind::Revert
                    },
                );
                let delegate = raw.trace.iter().find(|step| step.opcode == 0xf4).unwrap();
                assert_eq!(delegate.inputs[1], implementation);
                assert_eq!(delegate.inputs[3], U256::from(data.len()));
                assert_eq!(&observed.memory[..response.len()], &response);
                assert!(observed.responses.is_empty());
                assert_eq!(observed.storage.contains_key(&U256::from(42)), success);
            }
        }
        let admin_fallback = proxy_host(&input, vec![0x12, 0x34, 0x56, 0x78], true);
        let (raw, _) = differential(&input, &artifact, "admin_fallback", admin_fallback);
        exit(&raw, ExitKind::Revert);
        assert!(raw.trace.iter().all(|step| step.opcode != 0xf4));
    }
}

#[derive(Default)]
struct Asm {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

impl Asm {
    fn op(&mut self, opcode: u8) -> &mut Self {
        self.bytes.push(opcode);
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
            self.bytes[offset..offset + 2]
                .copy_from_slice(&u16::try_from(self.labels[name]).unwrap().to_be_bytes());
        }
        Input::new(self.bytes, Fork::Osaka)
    }
}

#[test]
fn sequential_helper_calls_with_one_outer_return_keep_distinct_active_returns() {
    let mut a = Asm::default();
    a.target("outer_return")
        .target("routine")
        .op(0x56)
        .dest("outer_return")
        .op(0x00)
        .dest("routine")
        .target("after_first")
        .target("helper")
        .op(0x56)
        .dest("after_first");
    increment_slot(&mut a);
    a.target("after_second")
        .target("helper")
        .op(0x56)
        .dest("after_second");
    increment_slot(&mut a);
    a.op(0x56).dest("helper").op(0x56);
    let pcs = a.labels.clone();
    let input = a.finish();
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = decompile(
            &input,
            &Config {
                strategy,
                ..Config::default()
            },
        );
        complete(&artifact);
        let base = baseline(&artifact);
        let helpers = at_pc(base, pcs["helper"]);
        singleton_returns(base, &helpers);
        assert_eq!(
            helpers
                .iter()
                .flat_map(|block| target_pcs(base, block))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([pcs["after_first"], pcs["after_second"]])
        );
        for block in helpers {
            assert_eq!(block.parameters.len(), 2);
            assert_eq!(
                base.ir.value(block.parameters[0]).unwrap().fact.singleton(),
                Some(U256::from(pcs["outer_return"]))
            );
        }
        acyclic_without_dispatcher(&artifact);
        let (raw, host) = differential(
            &input,
            &artifact,
            "sequential_helper",
            MemoryHost::new(&input, Vec::new()),
        );
        exit(&raw, ExitKind::Stop);
        assert_eq!(host.storage[&U256::ZERO], U256::from(2));
        assert_eq!(
            raw.trace.iter().filter(|step| step.opcode == 0x55).count(),
            2
        );
    }
}

fn increment_slot(a: &mut Asm) {
    a.op(0x5f).op(0x54).push(1).op(0x01).op(0x5f).op(0x55);
}

#[test]
fn nested_shared_helpers_preserve_same_height_return_pairs() {
    let mut a = Asm::default();
    a.op(0x5f)
        .op(0x35)
        .target("site_b")
        .op(0x57)
        .target("outer_a")
        .target("middle_a")
        .target("shared")
        .op(0x56)
        .dest("site_b")
        .target("outer_b")
        .target("middle_b")
        .target("shared")
        .op(0x56)
        .dest("shared")
        .target("inner_return")
        .target("inner")
        .op(0x56)
        .dest("inner")
        .op(0x56)
        .dest("inner_return")
        .op(0x56)
        .dest("middle_a")
        .op(0x56)
        .dest("middle_b")
        .op(0x56)
        .dest("outer_a")
        .push(11)
        .op(0x5f)
        .op(0x55)
        .op(0x00)
        .dest("outer_b")
        .push(22)
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    let pcs = a.labels.clone();
    let input = a.finish();
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let config = Config {
            strategy,
            ..Config::default()
        };
        let artifact = decompile(&input, &config);
        complete(&artifact);
        let base = baseline(&artifact);
        exact_returns(base, pcs["middle_a"], pcs["outer_a"]);
        exact_returns(base, pcs["middle_b"], pcs["outer_b"]);
        exact_returns(base, pcs["inner"], pcs["inner_return"]);
        let real_continuations: BTreeSet<_> = at_pc(base, pcs["inner_return"])
            .iter()
            .flat_map(|block| target_pcs(base, block))
            .collect();
        assert_eq!(
            real_continuations,
            BTreeSet::from([pcs["middle_a"], pcs["middle_b"]])
        );
        assert!(
            at_pc(base, pcs["shared"])
                .iter()
                .all(|block| block.parameters.len() == 2)
        );
        provenance(&artifact);
        for (condition, expected) in [(0, 11), (1, 22)] {
            let (raw, host) = differential(
                &input,
                &artifact,
                "nested_helper",
                MemoryHost::new(&input, calldata(condition)),
            );
            exit(&raw, ExitKind::Stop);
            assert_eq!(host.storage[&U256::ZERO], U256::from(expected));
        }
        assert_eq!(artifact, decompile(&input, &config));
    }
}

#[test]
fn unknown_helper_return_keeps_a_partial_exit_and_known_continuation() {
    let (input, pcs, _) = unknown_first_fixture(ReturnConstruction::Literal);
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = decompile(
            &input,
            &Config {
                strategy,
                ..Config::default()
            },
        );
        validated(&artifact);
        assert_eq!(artifact.completion, Completion::Partial);
        let base = baseline(&artifact);
        let contexts = at_pc(base, pcs["helper"]);
        assert!(contexts.iter().any(|block| block.terminator.is_partial()));
        assert_eq!(
            contexts
                .iter()
                .flat_map(|block| target_pcs(base, block))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([pcs["known"]])
        );
        for block in contexts
            .iter()
            .filter(|block| block.terminator.is_partial())
        {
            assert_eq!(
                target_pcs(base, block),
                BTreeSet::from([pcs["known"]]),
                "unknown context must share the observed known continuation"
            );
            let Terminator::Jump {
                target,
                unresolved: true,
                ..
            } = block.terminator
            else {
                unreachable!()
            };
            assert_eq!(base.ir.value(target).unwrap().fact, ValueFact::Unknown);
        }
        let closed: Vec<_> = contexts
            .iter()
            .copied()
            .filter(|block| !block.terminator.is_partial())
            .collect();
        assert!(
            !closed.is_empty(),
            "literal return context must remain closed"
        );
        singleton_returns(base, &closed);
        assert!(base.ir.blocks.iter().all(|block| {
            !block
                .source
                .is_some_and(|source| source.pc == pcs["unrecovered"])
        }));
        assert!(
            artifact
                .stages
                .iter()
                .filter(|stage| stage.kind != StageKind::Decoded)
                .all(|stage| stage.ir.fact_scope == FactScope::RecoveredPaths)
        );
        let (_, known_host) = differential(
            &input,
            &artifact,
            "known_return",
            MemoryHost::new(&input, calldata(0)),
        );
        assert_eq!(known_host.storage[&U256::ZERO], U256::from(42));

        let mut known_dynamic = calldata(1);
        known_dynamic.extend(U256::from(pcs["known"]).to_be_bytes::<32>());
        let (_, known_dynamic_host) = differential(
            &input,
            &artifact,
            "known_target_through_unknown_return",
            MemoryHost::new(&input, known_dynamic),
        );
        assert_eq!(known_dynamic_host.storage[&U256::ZERO], U256::from(42));

        unrecovered_return_is_partial(&input, &artifact, pcs["unrecovered"]);
    }
}

#[test]
fn untagged_return_witnesses_survive_unknown_before_first_helper_evaluation() {
    for construction in [ReturnConstruction::Add, ReturnConstruction::MemoryLoad] {
        let (input, pcs, producer_pc) = unknown_first_fixture(construction);
        for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
            let artifact = decompile(
                &input,
                &Config {
                    strategy,
                    ..Config::default()
                },
            );
            validated(&artifact);
            assert_eq!(artifact.completion, Completion::Partial);
            let base = baseline(&artifact);
            let contexts = at_pc(base, pcs["helper"]);
            // Both untagged returns enter the same queued helper before its
            // first transfer. No finite sibling jump can seed its target pool.
            assert_eq!(contexts.len(), 1, "{construction:?}");
            let block = contexts[0];
            let Terminator::Jump {
                target,
                edges,
                unresolved: true,
                invalid_targets,
            } = &block.terminator
            else {
                panic!("missing unknown return: {:?}", block.terminator);
            };
            assert!(invalid_targets.is_empty());
            assert_eq!(edges.len(), 1);
            assert_eq!(base.ir.value(*target).unwrap().fact, ValueFact::Unknown);
            assert_eq!(target_pcs(base, block), BTreeSet::from([pcs["known"]]));
            let producer = base
                .ir
                .blocks
                .iter()
                .flat_map(|block| &block.operations)
                .find(|operation| {
                    operation
                        .source
                        .is_some_and(|source| source.pc == producer_pc)
                })
                .unwrap();
            assert_eq!(
                base.ir.value(producer.outputs[0]).unwrap().fact.singleton(),
                Some(U256::from(pcs["known"]))
            );
            assert!(base.ir.blocks.iter().all(|block| {
                !block
                    .source
                    .is_some_and(|source| source.pc == pcs["unrecovered"])
            }));
            assert!(
                artifact
                    .stages
                    .iter()
                    .filter(|stage| stage.kind != StageKind::Decoded)
                    .all(|stage| stage.ir.fact_scope == FactScope::RecoveredPaths)
            );
            for condition in [0, 1] {
                let mut data = calldata(condition);
                data.extend(U256::from(pcs["known"]).to_be_bytes::<32>());
                let (raw, observed) = differential(
                    &input,
                    &artifact,
                    "untagged_known_return",
                    MemoryHost::new(&input, data),
                );
                exit(&raw, ExitKind::Stop);
                assert_eq!(observed.storage[&U256::ZERO], U256::from(42));
            }
            unrecovered_return_is_partial(&input, &artifact, pcs["unrecovered"]);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ReturnConstruction {
    Literal,
    Add,
    MemoryLoad,
}

fn unknown_first_fixture(
    construction: ReturnConstruction,
) -> (Input, BTreeMap<&'static str, usize>, usize) {
    let mut a = Asm::default();
    a.op(0x5f).op(0x35).target("dynamic").op(0x57).push(42);
    let mut producer_pc = a.bytes.len();
    a.target("known");
    match construction {
        ReturnConstruction::Literal => {}
        ReturnConstruction::Add => {
            a.op(0x5f);
            producer_pc = a.bytes.len();
            a.op(0x01);
        }
        ReturnConstruction::MemoryLoad => {
            a.op(0x5f).op(0x52).op(0x5f);
            producer_pc = a.bytes.len();
            a.op(0x51);
        }
    }
    a.target("helper")
        .op(0x56)
        .dest("dynamic")
        .push(42)
        .push(32)
        .op(0x35)
        .target("helper")
        .op(0x56)
        .dest("helper")
        .op(0x56)
        .dest("known")
        .op(0x5f)
        .op(0x55)
        .op(0x00)
        .dest("unrecovered")
        .op(0x50)
        .push(99)
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    let pcs = a.labels.clone();
    (a.finish(), pcs, producer_pc)
}

fn unrecovered_return_is_partial(input: &Input, artifact: &Artifact, target_pc: usize) {
    let mut data = calldata(1);
    data.extend(U256::from(target_pc).to_be_bytes::<32>());
    let host = MemoryHost::new(input, data);
    let limits = ExecutionLimits::default();
    let mut raw_host = host.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits).unwrap();
    exit(&raw, ExitKind::Stop);
    assert_eq!(raw_host.storage[&U256::ZERO], U256::from(99));
    let expected = ExecutionOutcome::PartialJump {
        target: Some(U256::from(target_pc)),
    };
    for stage in artifact
        .stages
        .iter()
        .filter(|stage| stage.kind != StageKind::Decoded)
    {
        let mut graph_host = host.clone();
        let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
        assert_eq!(graph.outcome, expected);
        assert!(graph_host.storage.is_empty());
    }
    let mut syntax_host = host;
    let syntax =
        execute_structured(artifact.current().unwrap(), &mut syntax_host, &limits).unwrap();
    assert_eq!(syntax.outcome, expected);
    assert!(syntax_host.storage.is_empty());
}

#[test]
fn unknown_context_shares_known_targets_before_and_after_discovery() {
    for unknown_first in [false, true] {
        let mut a = Asm::default();
        a.op(0x5f).op(0x35).target("delayed").op(0x57);
        returning_frame(&mut a, 11, unknown_first);
        a.dest("delayed")
            .target("delay_two")
            .op(0x56)
            .dest("delay_two")
            .target("delay_three")
            .op(0x56)
            .dest("delay_three");
        returning_frame(&mut a, 22, !unknown_first);
        a.dest("helper")
            .op(0x56)
            .dest("known")
            .op(0x5f)
            .op(0x55)
            .op(0x5f)
            .op(0x51)
            .push(1)
            .op(0x55)
            .op(0x00)
            .dest("unrecovered")
            .op(0x50)
            .push(99)
            .op(0x5f)
            .op(0x55)
            .op(0x00);
        let pcs = a.labels.clone();
        let input = a.finish();
        for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
            let config = Config {
                strategy,
                ..Config::default()
            };
            let artifact = decompile(&input, &config);
            validated(&artifact);
            assert_eq!(artifact.completion, Completion::Partial);
            let base = baseline(&artifact);
            let contexts = at_pc(base, pcs["helper"]);
            let unknown: Vec<_> = contexts
                .iter()
                .filter(|block| block.terminator.is_partial())
                .collect();
            assert!(!unknown.is_empty());
            for block in unknown {
                assert_eq!(target_pcs(base, block), BTreeSet::from([pcs["known"]]));
                let Terminator::Jump {
                    target,
                    unresolved: true,
                    ..
                } = block.terminator
                else {
                    unreachable!()
                };
                assert_eq!(base.ir.value(target).unwrap().fact, ValueFact::Unknown);
            }
            let closed: Vec<_> = contexts
                .iter()
                .copied()
                .filter(|block| !block.terminator.is_partial())
                .collect();
            assert!(
                !closed.is_empty(),
                "literal return context must remain closed"
            );
            singleton_returns(base, &closed);
            assert!(base.ir.blocks.iter().all(|block| {
                !block
                    .source
                    .is_some_and(|source| source.pc == pcs["unrecovered"])
            }));
            assert!(
                artifact
                    .stages
                    .iter()
                    .filter(|stage| stage.kind != StageKind::Decoded)
                    .all(|stage| stage.ir.fact_scope == FactScope::RecoveredPaths)
            );
            // FIFO evaluates the fast helper context before delay_three. Test
            // unknown-first discovery and the reverse order with each frame's
            // own stack arguments, memory snapshot and observable effects.
            for (condition, expected) in [(0, 11), (1, 22)] {
                let mut data = calldata(condition);
                data.extend(U256::from(pcs["known"]).to_be_bytes::<32>());
                let (raw, observed) = differential(
                    &input,
                    &artifact,
                    if unknown_first {
                        "unknown_before_known"
                    } else {
                        "known_before_unknown"
                    },
                    MemoryHost::new(&input, data),
                );
                exit(&raw, ExitKind::Stop);
                assert_eq!(observed.storage[&U256::ZERO], U256::from(expected));
                assert_eq!(observed.storage[&U256::from(1)], U256::from(expected));
            }
            assert_eq!(artifact, decompile(&input, &config));
        }
    }
}

fn returning_frame(a: &mut Asm, payload: u8, unknown: bool) {
    a.push(payload).op(0x5f).op(0x52).push(payload);
    if unknown {
        a.push(32).op(0x35);
    } else {
        a.target("known");
    }
    a.target("helper").op(0x56);
}

#[test]
fn loop_carried_data_converges_without_unbounded_return_contexts() {
    let mut a = Asm::default();
    a.op(0x5f)
        .dest("head")
        .op(0x80)
        .push(12)
        .op(0x90)
        .op(0x10)
        .op(0x15)
        .target("done")
        .op(0x57)
        .target("after_helper")
        .target("helper")
        .op(0x56)
        .dest("after_helper")
        .push(1)
        .op(0x01)
        .op(0x80)
        .op(0x5f)
        .op(0x55)
        .target("head")
        .op(0x56)
        .dest("done")
        .op(0x50)
        .op(0x00)
        .dest("helper")
        .op(0x56);
    let pcs = a.labels.clone();
    let input = a.finish();
    let config = Config::default();
    let artifact = decompile(&input, &config);
    complete(&artifact);
    exact_returns(baseline(&artifact), pcs["helper"], pcs["after_helper"]);
    let decoded = artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Decoded)
        .unwrap();
    assert!(baseline(&artifact).ir.blocks.len() <= decoded.ir.blocks.len() * 4);
    assert!(artifact.statistics.evaluations < 1_000);
    let (_, host) = differential(
        &input,
        &artifact,
        "loop_helper",
        MemoryHost::new(&input, Vec::new()),
    );
    assert_eq!(host.storage[&U256::ZERO], U256::from(12));
    assert_eq!(artifact, decompile(&input, &config));
}

#[test]
fn recursive_context_growth_respects_each_declared_recovery_limit() {
    let mut a = Asm::default();
    a.dest("recurse")
        .target("return")
        .target("recurse")
        .op(0x56)
        .dest("return")
        .op(0x00);
    let input = a.finish();
    let decoded = Program::decode_with_fork(&input.runtime_bytecode, input.fork).unwrap();
    let blocks = decoded.blocks().len();
    let operations: usize = decoded
        .blocks()
        .iter()
        .map(|block| block.instructions.len())
        .sum();
    let mut configurations = Vec::new();
    let mut config = Config::default();
    config.limits.max_blocks = blocks + 2;
    configurations.push(("blocks", config));
    let mut config = Config::default();
    config.limits.max_operations = operations + 8;
    configurations.push(("operations", config));
    let mut config = Config::default();
    config.limits.max_values = 64;
    configurations.push(("values", config));
    let mut config = Config::default();
    config.limits.max_iterations = 8;
    configurations.push(("iterations", config));
    let mut config = Config::default();
    config.limits.max_work = 10_000;
    configurations.push(("work", config));
    for (label, config) in configurations {
        let artifact = decompile(&input, &config);
        validated(&artifact);
        assert_eq!(
            artifact.completion,
            Completion::Limited,
            "{label}: {:?}",
            artifact.diagnostics
        );
        assert!(
            artifact
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::Limit)
        );
        assert!(
            artifact.statistics.work <= config.limits.max_work,
            "{label}"
        );
        assert!(
            artifact.statistics.evaluations <= config.limits.max_iterations,
            "{label}"
        );
        for stage in &artifact.stages {
            assert!(stage.ir.blocks.len() <= config.limits.max_blocks, "{label}");
            assert!(stage.ir.values.len() <= config.limits.max_values, "{label}");
            assert!(
                stage
                    .ir
                    .blocks
                    .iter()
                    .map(|block| block.operations.len())
                    .sum::<usize>()
                    <= config.limits.max_operations,
                "{label}"
            );
        }
        assert_eq!(
            artifact.current().unwrap().kind,
            StageKind::Decoded,
            "{label}"
        );
    }
}
