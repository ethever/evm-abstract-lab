//! Concrete executions check recovery facts as upper bounds, independently of
//! the abstract transfer. Pure rewrites may change the trace; effects may not.
use evm_decompile::{
    Artifact, Completion, Config, Execution, ExecutionError, ExecutionLimits, ExternalResponse,
    FactScope, Fork, Input, MemoryHost, Primitive, SemanticHost, Stage, StageKind, Strategy, U256,
    ValidationStatus, ValueFact, decompile, execute_bytecode, execute_ir, execute_structured,
};
use std::collections::{BTreeMap, VecDeque};

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
    fn word(&mut self, value: U256) -> &mut Self {
        self.bytes.push(0x7f);
        self.bytes.extend(value.to_be_bytes::<32>());
        self
    }
    fn target(&mut self, label: &'static str) -> &mut Self {
        self.bytes.push(0x61);
        self.fixups.push((self.bytes.len(), label));
        self.bytes.extend([0, 0]);
        self
    }
    fn dest(&mut self, label: &'static str) -> &mut Self {
        assert!(self.labels.insert(label, self.bytes.len()).is_none());
        self.op(0x5b)
    }
    fn finish(mut self) -> Input {
        for (offset, label) in self.fixups {
            self.bytes[offset..offset + 2]
                .copy_from_slice(&u16::try_from(self.labels[label]).unwrap().to_be_bytes());
        }
        Input::new(self.bytes, Fork::Osaka)
    }
}
fn config(strategy: Strategy) -> Config {
    let mut config = Config {
        strategy,
        ..Config::default()
    };
    config.limits.max_work = 50_000_000;
    config.limits.max_iterations = 20_000;
    config.limits.max_constant_set = 8;
    config
}
fn compile(input: &Input, strategy: Strategy) -> Artifact {
    let artifact = decompile(input, &config(strategy));
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
            .all(|s| s.validation.status == ValidationStatus::Passed)
    );
    assert_eq!(artifact.current().unwrap().kind, StageKind::Lifted);
    artifact
}
fn baseline(artifact: &Artifact) -> &Stage {
    artifact
        .stages
        .iter()
        .find(|s| s.kind == StageKind::Baseline)
        .unwrap()
}
fn includes(fact: &ValueFact, concrete: U256) -> bool {
    match fact {
        ValueFact::Unknown => true,
        ValueFact::Constants(values) => values.contains(&concrete),
        ValueFact::Bottom => false,
    }
}
fn sound_facts(stage: &Stage, execution: &Execution) {
    // Check every loop visit, not only the final value of a reused SSA name.
    let operations: BTreeMap<_, _> = stage
        .ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .map(|op| (op.id, op))
        .collect();
    for step in &execution.trace {
        let Some(id) = step.operation else { continue };
        let op = operations[&id];
        for (&id, &concrete) in op.outputs.iter().zip(&step.outputs) {
            let fact = &stage.ir.value(id).unwrap().fact;
            assert!(
                includes(fact, concrete),
                "stage {:?}, pc {:?}, value {id:?}: {concrete:#x} outside {fact:?}",
                stage.kind,
                step.pc
            );
        }
    }
    for (&id, &concrete) in &execution.values {
        let fact = &stage.ir.value(id).unwrap().fact;
        assert!(
            includes(fact, concrete),
            "stage {:?}, value {id:?}: {concrete:#x} outside {fact:?}",
            stage.kind
        );
    }
}
fn oracle<H>(input: &Input, artifact: &Artifact, host: H) -> (Execution, H)
where
    H: SemanticHost + Clone + PartialEq + std::fmt::Debug,
{
    let limits = ExecutionLimits {
        max_steps: 10_000,
        ..ExecutionLimits::default()
    };
    let mut raw_host = host.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits).unwrap();
    let effects = |execution: &Execution| {
        execution
            .observations()
            .into_iter()
            .filter(|step| !step.effects.is_pure())
            .collect::<Vec<_>>()
    };
    for stage in artifact
        .stages
        .iter()
        .filter(|s| s.kind != StageKind::Decoded)
    {
        let mut graph_host = host.clone();
        let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
        assert_eq!(raw.outcome, graph.outcome, "graph {:?}", stage.kind);
        assert_eq!(effects(&raw), effects(&graph), "graph {:?}", stage.kind);
        assert_eq!(raw_host, graph_host, "graph host {:?}", stage.kind);
        sound_facts(stage, &graph);
        if stage.ir.structured.is_some() {
            let mut syntax_host = host.clone();
            let syntax = execute_structured(stage, &mut syntax_host, &limits).unwrap();
            assert_eq!(raw.outcome, syntax.outcome, "AST {:?}", stage.kind);
            assert_eq!(effects(&raw), effects(&syntax), "AST {:?}", stage.kind);
            assert_eq!(raw_host, syntax_host, "AST host {:?}", stage.kind);
            sound_facts(stage, &syntax);
        }
    }
    (raw, raw_host)
}
fn host(input: &Input, value: U256) -> MemoryHost {
    let mut host = MemoryHost::new(input, Vec::new());
    host.context.insert(0x34, value);
    host
}
fn loads(artifact: &Artifact, opcode: u8) -> Vec<&ValueFact> {
    let stage = baseline(artifact);
    stage
        .ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .filter(|o| o.opcode == opcode)
        .map(|o| &stage.ir.value(o.outputs[0]).unwrap().fact)
        .collect()
}
fn has_store(artifact: &Artifact) -> bool {
    baseline(artifact)
        .ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .any(|o| o.opcode == 0x55)
}

#[test]
fn path_condition_removes_the_contradictory_store() {
    impossible_store("34600a811061000a57005b600a81101561001557005b600160005500");
}

#[test]
fn product_range_removes_the_masked_byte_overflow_store() {
    impossible_store("3460ff1660ff811161000d57005b600160005500");
}

fn impossible_store(hex: &str) {
    let input = Input::new(alloy_primitives::hex::decode(hex).unwrap(), Fork::Osaka);
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = compile(&input, strategy);
        for value in (0..=32).map(U256::from).chain([
            U256::from(254),
            U256::from(255),
            U256::from(256),
            (U256::from(1) << 255) - U256::from(1),
            U256::from(1) << 255,
            U256::MAX,
        ]) {
            let (_, observed) = oracle(&input, &artifact, host(&input, value));
            assert!(observed.storage.is_empty());
        }
        assert!(!has_store(&artifact), "impossible SSTORE survived: {hex}");
    }
}

#[test]
fn conflicting_memory_stores_join_values_and_byte_overlap_remains_sound() {
    let mut a = Asm::default();
    a.push(7)
        .push(0)
        .op(0x52)
        .op(0x34)
        .target("change")
        .op(0x57)
        .target("join")
        .op(0x56)
        .dest("change")
        .push(9)
        .push(0)
        .op(0x52)
        .dest("join")
        .push(0)
        .op(0x51)
        .push(0)
        .op(0x55)
        .push(255)
        .push(31)
        .op(0x53)
        .push(0)
        .op(0x51)
        .push(1)
        .op(0x55)
        .op(0x00);
    let input = a.finish();
    let artifact = compile(&input, Strategy::Dispatcher);
    let facts = loads(&artifact, 0x51);
    assert_eq!(facts.len(), 2);
    assert_eq!(
        facts[0],
        &ValueFact::Constants(vec![U256::from(7), U256::from(9)])
    );
    for value in [0, 1] {
        let (_, observed) = oracle(&input, &artifact, host(&input, U256::from(value)));
        assert_eq!(
            observed.storage[&U256::ZERO],
            U256::from(if value == 0 { 7 } else { 9 })
        );
        assert_eq!(observed.storage[&U256::from(1)], U256::from(255));
    }
}

fn acyclic_fanin_input(count: usize, memory: bool) -> Input {
    const LABELS: [&str; 8] = [
        "case0", "case1", "case2", "case3", "case4", "case5", "case6", "case7",
    ];
    assert!((2..=9).contains(&count));
    let mut a = Asm::default();
    a.op(0x34);
    for (selector, label) in LABELS.iter().take(count - 1).enumerate() {
        a.op(0x80)
            .push(u8::try_from(selector).unwrap())
            .op(0x14)
            .target(label)
            .op(0x57);
    }
    let arm = |a: &mut Asm, selector: usize| {
        a.op(0x50).push(u8::try_from(7 + selector).unwrap());
        if memory {
            a.push(0).op(0x52);
        }
        a.target("join").op(0x56);
    };
    arm(&mut a, count - 1);
    for (selector, label) in LABELS.iter().take(count - 1).enumerate() {
        a.dest(label);
        arm(&mut a, selector);
    }
    a.dest("join");
    if memory {
        a.push(0).op(0x51);
    }
    a.push(0).op(0x55).op(0x00);
    a.finish()
}

fn acyclic_fanin(memory: bool) {
    // All paths are acyclic: receiving many predecessors must not trigger
    // loop widening. The last case deliberately exceeds the finite cap of 8.
    for count in [4, 8, 9] {
        let input = acyclic_fanin_input(count, memory);
        let artifact = compile(&input, Strategy::Dispatcher);
        let stage = baseline(&artifact);
        let result = if memory {
            loads(&artifact, 0x51)[0]
        } else {
            let store = stage
                .ir
                .blocks
                .iter()
                .flat_map(|block| &block.operations)
                .find(|operation| operation.opcode == 0x55)
                .unwrap();
            &stage.ir.value(store.inputs[1]).unwrap().fact
        };
        let expected = if count <= 8 {
            ValueFact::Constants((7..7 + count).map(U256::from).collect())
        } else {
            ValueFact::Unknown
        };
        assert_eq!(result, &expected, "{count}-way memory={memory} fan-in");
        for selector in (0..count).map(U256::from).chain([U256::MAX]) {
            let (_, observed) = oracle(&input, &artifact, host(&input, selector));
            let selected = usize::try_from(selector)
                .ok()
                .filter(|s| *s < count - 1)
                .unwrap_or(count - 1);
            assert_eq!(observed.storage[&U256::ZERO], U256::from(7 + selected));
        }
    }
}

#[test]
fn acyclic_stack_fanin_keeps_all_finite_values_without_loop_widening() {
    acyclic_fanin(false);
}

#[test]
fn acyclic_memory_fanin_keeps_all_finite_values_without_loop_widening() {
    acyclic_fanin(true);
}

#[test]
fn joined_paths_do_not_keep_a_predecessors_condition() {
    let mut a = Asm::default();
    a.op(0x34)
        .push(10)
        .op(0x81)
        .op(0x10)
        .target("small")
        .op(0x57)
        .push(9)
        .target("join")
        .op(0x56)
        .dest("small")
        .push(7)
        .dest("join")
        .op(0x50)
        .push(10)
        .op(0x81)
        .op(0x10)
        .op(0x15)
        .target("large")
        .op(0x57)
        .op(0x00)
        .dest("large")
        .push(1)
        .push(0)
        .op(0x55)
        .op(0x00);
    let input = a.finish();
    let artifact = compile(&input, Strategy::Dispatcher);
    assert!(has_store(&artifact));
    for value in [0, 9, 10, 11, 255] {
        let (_, observed) = oracle(&input, &artifact, host(&input, U256::from(value)));
        assert_eq!(observed.storage.contains_key(&U256::ZERO), value >= 10);
    }
}

#[test]
fn signed_comparisons_and_wrapping_addition_keep_concrete_counterexamples() {
    let sign = U256::from(1) << 255;
    for (opcode, bound) in [(0x10, sign), (0x12, U256::ZERO)] {
        let mut a = Asm::default();
        a.op(0x34)
            .word(bound)
            .op(0x81)
            .op(opcode)
            .target("true")
            .op(0x57)
            .push(0)
            .push(0)
            .op(0x55)
            .op(0x00)
            .dest("true")
            .push(1)
            .push(0)
            .op(0x55)
            .op(0x00);
        let input = a.finish();
        let artifact = compile(&input, Strategy::Dispatcher);
        for value in [
            U256::ZERO,
            U256::from(1),
            sign - U256::from(1),
            sign,
            U256::MAX,
        ] {
            let (_, observed) = oracle(&input, &artifact, host(&input, value));
            let truth = if opcode == 0x10 {
                value < sign
            } else {
                value >= sign
            };
            assert_eq!(observed.storage[&U256::ZERO], U256::from(u8::from(truth)));
        }
    }
    let mut a = Asm::default();
    a.op(0x34)
        .push(0)
        .op(0x81)
        .op(0x11)
        .target("positive")
        .op(0x57)
        .op(0x00)
        .dest("positive")
        .op(0x80)
        .push(1)
        .op(0x01)
        .op(0x81)
        .op(0x81)
        .op(0x10)
        .target("wrapped")
        .op(0x57)
        .op(0x00)
        .dest("wrapped")
        .push(1)
        .push(0)
        .op(0x55)
        .op(0x00);
    let input = a.finish();
    let artifact = compile(&input, Strategy::Dispatcher);
    assert!(has_store(&artifact));
    for value in [
        U256::ZERO,
        U256::from(1),
        U256::MAX - U256::from(1),
        U256::MAX,
    ] {
        let (_, observed) = oracle(&input, &artifact, host(&input, value));
        assert_eq!(
            observed.storage.contains_key(&U256::ZERO),
            value == U256::MAX
        );
    }
}

#[test]
fn same_symbolic_memory_address_forwards_its_last_write() {
    symbolic_forward(0x52, 0x51);
}

#[test]
fn fresh_memory_and_zero_length_copies_read_zero_at_symbolic_addresses() {
    for copy in [None, Some(0x37), Some(0x39), Some(0x3e), Some(0x5e)] {
        let mut a = Asm::default();
        if let Some(copy) = copy {
            // size = 0, source offset = 0, unknown destination = CALLVALUE.
            a.push(0).push(0).op(0x34).op(copy);
        }
        a.op(0x34).op(0x51).push(2).op(0x55).op(0x00);
        let input = a.finish();
        let artifact = compile(&input, Strategy::Dispatcher);
        assert_eq!(
            loads(&artifact, 0x51),
            [&ValueFact::Constants(vec![U256::ZERO])],
            "zero-length copy {copy:?} changed fresh memory facts"
        );
        for address in [0, 1, 31, 64] {
            let (_, observed) = oracle(&input, &artifact, host(&input, U256::from(address)));
            assert_eq!(observed.storage[&U256::from(2)], U256::ZERO);
        }
    }
}

#[test]
fn actual_symbolic_writes_invalidate_the_fresh_zero_memory_default() {
    for write in [0x52, 0x53] {
        let mut a = Asm::default();
        a.push(9)
            .op(0x34)
            .op(write)
            .push(0)
            .op(0x51)
            .push(2)
            .op(0x55)
            .op(0x00);
        let input = a.finish();
        let artifact = compile(&input, Strategy::Dispatcher);
        let fact = loads(&artifact, 0x51)[0];
        let nonzero = if write == 0x52 {
            U256::from(9)
        } else {
            U256::from(9) << 248
        };
        assert!(includes(fact, U256::ZERO) && includes(fact, nonzero));
        for address in [0, 1, 31, 32, 64] {
            let (_, observed) = oracle(&input, &artifact, host(&input, U256::from(address)));
            if address == 0 {
                assert_eq!(observed.storage[&U256::from(2)], nonzero);
            } else if address >= 32 {
                assert_eq!(observed.storage[&U256::from(2)], U256::ZERO);
            }
        }
    }
}

#[test]
fn same_symbolic_storage_key_forwards_its_last_write() {
    symbolic_forward(0x55, 0x54);
}

#[test]
fn same_symbolic_transient_key_forwards_its_last_write() {
    symbolic_forward(0x5d, 0x5c);
}

fn symbolic_forward(write: u8, read: u8) {
    let mut a = Asm::default();
    a.op(0x34)
        .op(0x80)
        .push(7)
        .op(0x90)
        .op(write)
        .op(read)
        .push(2)
        .op(0x55)
        .op(0x00);
    let input = a.finish();
    let artifact = compile(&input, Strategy::Dispatcher);
    for address in [0, 1, 31, 32, 64] {
        let (_, observed) = oracle(&input, &artifact, host(&input, U256::from(address)));
        assert_eq!(observed.storage[&U256::from(2)], U256::from(7));
    }
    assert_eq!(
        loads(&artifact, read),
        [&ValueFact::Constants(vec![U256::from(7)])]
    );
}

#[test]
fn initial_storage_and_transient_storage_are_unconstrained() {
    for read in [0x54, 0x5c] {
        let mut a = Asm::default();
        a.push(0).op(read).push(1).op(0x55).op(0x00);
        let input = a.finish();
        let artifact = compile(&input, Strategy::Dispatcher);
        assert_eq!(loads(&artifact, read), [&ValueFact::Unknown]);
        for value in [U256::ZERO, U256::from(7), U256::MAX] {
            let mut initial = host(&input, U256::ZERO);
            initial.storage.insert(U256::ZERO, value);
            initial.transient.insert(U256::ZERO, value);
            let (_, observed) = oracle(&input, &artifact, initial);
            assert_eq!(observed.storage[&U256::from(1)], value);
        }
    }
}

#[test]
fn uncertain_aliases_invalidate_memory_persistent_and_transient_facts() {
    for (write, read, alias_write) in [(0x52, 0x51, 0x53), (0x55, 0x54, 0x55), (0x5d, 0x5c, 0x5d)] {
        let mut a = Asm::default();
        a.push(7)
            .push(0)
            .op(write)
            .push(9)
            .op(0x34)
            .op(alias_write)
            .push(0)
            .op(read)
            .push(2)
            .op(0x55)
            .op(0x00);
        let input = a.finish();
        let artifact = compile(&input, Strategy::Dispatcher);
        let mut results = Vec::new();
        for address in [0, 1, 31, 32, 64] {
            let (_, observed) = oracle(&input, &artifact, host(&input, U256::from(address)));
            results.push(observed.storage[&U256::from(2)]);
        }
        assert!(results.windows(2).any(|w| w[0] != w[1]));
    }
}

#[test]
fn wrapping_address_arithmetic_can_alias_the_zero_memory_cell() {
    let mut a = Asm::default();
    a.push(7)
        .push(0)
        .op(0x52)
        .push(9)
        .op(0x34)
        .push(32)
        .op(0x01)
        .op(0x52)
        .push(0)
        .op(0x51)
        .push(2)
        .op(0x55)
        .op(0x00);
    let input = a.finish();
    let artifact = compile(&input, Strategy::Dispatcher);
    for value in [U256::ZERO, U256::from(32), U256::MAX - U256::from(31)] {
        let (_, observed) = oracle(&input, &artifact, host(&input, value));
        let wrapped = value == U256::MAX - U256::from(31);
        assert_eq!(
            observed.storage[&U256::from(2)],
            U256::from(if wrapped { 9 } else { 7 })
        );
    }
}

#[test]
fn calls_replace_output_memory_and_can_change_storage() {
    for opcode in [0xf1, 0xf4] {
        let mut a = Asm::default();
        a.push(7)
            .push(0)
            .op(0x52)
            .push(7)
            .push(0)
            .op(0x55)
            .push(32)
            .push(0)
            .push(0)
            .push(0);
        if opcode == 0xf1 {
            a.push(0);
        }
        a.push(1)
            .push(255)
            .op(opcode)
            .op(0x50)
            .push(0)
            .op(0x51)
            .push(2)
            .op(0x55)
            .push(0)
            .op(0x54)
            .push(3)
            .op(0x55)
            .op(0x00);
        let input = a.finish();
        let artifact = compile(&input, Strategy::Dispatcher);
        for success in [0, 1] {
            let mut initial = host(&input, U256::ZERO);
            initial.responses.push_back(ExternalResponse {
                result: U256::from(success),
                return_data: U256::from(99).to_be_bytes::<32>().to_vec(),
                storage_writes: BTreeMap::from([(U256::ZERO, U256::from(42))]),
            });
            let (_, observed) = oracle(&input, &artifact, initial);
            assert_eq!(observed.storage[&U256::from(2)], U256::from(99));
            assert_eq!(
                observed.storage[&U256::from(3)],
                U256::from(if success == 0 { 7 } else { 42 })
            );
        }
    }
}

#[test]
fn moving_loop_values_converge_without_truncating_the_first_eight_values() {
    let mut a = Asm::default();
    a.push(0)
        .dest("head")
        .op(0x80)
        .push(20)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x00)
        .dest("body")
        .op(0x80)
        .push(0)
        .op(0x55)
        .push(1)
        .op(0x01)
        .target("head")
        .op(0x56);
    let input = a.finish();
    let artifact = compile(&input, Strategy::Dispatcher);
    let (raw, observed) = oracle(&input, &artifact, host(&input, U256::ZERO));
    assert_eq!(raw.trace.iter().filter(|s| s.opcode == 0x55).count(), 20);
    assert_eq!(observed.storage[&U256::ZERO], U256::from(19));
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GasHost {
    inner: MemoryHost,
    readings: VecDeque<U256>,
}
impl SemanticHost for GasHost {
    fn execute(&mut self, primitive: Primitive<'_>) -> Result<Vec<U256>, ExecutionError> {
        if primitive.opcode == 0x5a {
            Ok(vec![
                self.readings
                    .pop_front()
                    .expect("bounded changing GAS fixture"),
            ])
        } else {
            self.inner.execute(primitive)
        }
    }
}
#[test]
fn repeated_static_gas_opcode_does_not_reuse_a_previous_iterations_identity() {
    let mut a = Asm::default();
    a.dest("head")
        .op(0x5a)
        .push(10)
        .op(0x90)
        .op(0x11)
        .target("again")
        .op(0x57)
        .op(0x00)
        .dest("again")
        .push(1)
        .push(0)
        .op(0x55)
        .target("head")
        .op(0x56);
    let input = a.finish();
    let artifact = compile(&input, Strategy::Dispatcher);
    for readings in [vec![15, 5], vec![20, 11, 10], vec![10]] {
        let count = readings.len();
        let initial = GasHost {
            inner: host(&input, U256::ZERO),
            readings: readings.into_iter().map(U256::from).collect(),
        };
        let (raw, observed) = oracle(&input, &artifact, initial);
        assert_eq!(raw.trace.iter().filter(|s| s.opcode == 0x5a).count(), count);
        assert!(observed.readings.is_empty());
    }
}

#[test]
fn unknown_jump_target_keeps_its_recovery_scope_open() {
    let input = Input::new(vec![0x34, 0x56, 0x5b, 0x00, 0x5b, 0x00], Fork::Osaka);
    let artifact = decompile(&input, &config(Strategy::Dispatcher));
    let stage = baseline(&artifact);
    assert_eq!(stage.ir.fact_scope, FactScope::RecoveredPaths);
    assert!(stage.ir.blocks.iter().any(|b| b.terminator.is_partial()));
    assert_ne!(artifact.completion, Completion::Complete);
}

#[test]
fn jump_condition_proves_the_computed_target_before_target_enumeration() {
    let mut a = Asm::default();
    // x is the jump operand. The literal PC participates only in x == PC;
    // it is not the operand consumed as the target by JUMPI.
    a.op(0x34)
        .op(0x80)
        .target("legal")
        .op(0x14)
        .op(0x90)
        .op(0x57)
        .op(0x00)
        .dest("legal")
        .push(1)
        .push(0)
        .op(0x55)
        .op(0x00);
    let legal = U256::from(a.labels["legal"]);
    let input = a.finish();
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = compile(&input, strategy);
        assert_eq!(
            baseline(&artifact).ir.fact_scope,
            FactScope::CompleteLocalCfg
        );
        assert!(
            baseline(&artifact)
                .ir
                .blocks
                .iter()
                .all(|b| !b.terminator.is_partial())
        );
        for value in [
            U256::ZERO,
            U256::from(1),
            legal,
            legal + U256::from(1),
            U256::MAX,
        ] {
            let (_, observed) = oracle(&input, &artifact, host(&input, value));
            assert_eq!(observed.storage.contains_key(&U256::ZERO), value == legal);
        }
    }
}

#[test]
fn loop_carried_storage_reads_retain_each_iteration() {
    for (write, read) in [(0x55, 0x54), (0x5d, 0x5c)] {
        let mut a = Asm::default();
        a.push(0)
            .push(0)
            .op(write)
            .dest("head")
            .push(0)
            .op(read)
            .op(0x80)
            .push(3)
            .op(0x90)
            .op(0x10)
            .target("body")
            .op(0x57)
            .push(2)
            .op(0x55)
            .op(0x00)
            .dest("body")
            .push(1)
            .op(0x01)
            .push(0)
            .op(write)
            .target("head")
            .op(0x56);
        let input = a.finish();
        let artifact = compile(&input, Strategy::Dispatcher);
        let (raw, observed) = oracle(&input, &artifact, host(&input, U256::ZERO));
        assert_eq!(raw.trace.iter().filter(|s| s.opcode == read).count(), 4);
        assert_eq!(observed.storage[&U256::from(2)], U256::from(3));
    }
}

#[test]
fn repeated_calls_replace_returndata_identity() {
    let mut a = Asm::default();
    for slot in [1, 2] {
        a.push(0)
            .push(0)
            .push(0)
            .push(0)
            .push(0)
            .push(1)
            .push(255)
            .op(0xf1)
            .op(0x50)
            .op(0x3d)
            .push(slot)
            .op(0x55);
    }
    a.op(0x00);
    let input = a.finish();
    let artifact = compile(&input, Strategy::Dispatcher);
    let mut initial = host(&input, U256::ZERO);
    for length in [1, 2] {
        initial.responses.push_back(ExternalResponse {
            result: U256::from(1),
            return_data: vec![0; length],
            ..ExternalResponse::default()
        });
    }
    let (_, observed) = oracle(&input, &artifact, initial);
    assert_eq!(observed.storage[&U256::from(1)], U256::from(1));
    assert_eq!(observed.storage[&U256::from(2)], U256::from(2));
}

#[test]
fn proved_nested_conditions_normalize_the_terminator_before_structuring() {
    for invert in [false, true] {
        let mut a = Asm::default();
        a.op(0x34)
            .push(10)
            .op(0x81)
            .op(0x10)
            .target("small")
            .op(0x57)
            .op(0x00)
            .dest("small")
            .push(10)
            .op(0x81)
            .op(0x10);
        if invert {
            a.op(0x15);
        }
        a.target("taken")
            .op(0x57)
            .push(7)
            .push(0)
            .op(0x55)
            .op(0x00)
            .dest("taken")
            .push(9)
            .push(0)
            .op(0x55)
            .op(0x00);
        let inner_pc = a.labels["small"];
        let input = a.finish();
        let artifact = compile(&input, Strategy::Dispatcher);
        let inner = baseline(&artifact)
            .ir
            .blocks
            .iter()
            .find(|block| block.source.is_some_and(|source| source.pc == inner_pc))
            .unwrap();
        assert!(
            if invert {
                matches!(
                    inner.terminator,
                    evm_decompile::Terminator::Fallthrough { .. }
                )
            } else {
                matches!(inner.terminator, evm_decompile::Terminator::Jump { .. })
            },
            "proved condition retained a one-sided branch: {:?}",
            inner.terminator
        );
        for value in [0, 1, 9, 10, 11, 255] {
            let (_, observed) = oracle(&input, &artifact, host(&input, U256::from(value)));
            assert_eq!(
                observed.storage.get(&U256::ZERO).copied(),
                (value < 10).then_some(U256::from(if invert { 7 } else { 9 }))
            );
        }
    }
}

fn independent_input(a: &mut Asm, second: bool) {
    if second {
        a.push(0).op(0x35);
    } else {
        a.op(0x34);
    }
}

// These are bytecode recipes, not abstract-domain transfer implementations.
// Their concrete result is supplied only by the raw stack/PC interpreter.
fn generated_expression(a: &mut Asm, recipe: usize, variant: usize) {
    match recipe {
        0 => {
            independent_input(a, false);
            a.word(
                [
                    U256::from(1),
                    U256::from(255),
                    U256::from(1) << 255,
                    U256::MAX,
                ][variant],
            )
            .op(0x01);
        }
        1 => {
            independent_input(a, false);
            independent_input(a, true);
            a.op(0x18)
                .word(
                    [
                        U256::from(255),
                        U256::from(511),
                        U256::MAX >> 128,
                        U256::MAX,
                    ][variant],
                )
                .op(0x16);
        }
        2 | 3 => {
            independent_input(a, recipe == 3);
            independent_input(a, recipe == 2);
            a.word(U256::from(511))
                .op(0x16)
                .op(if recipe == 2 { 0x1b } else { 0x1d });
        }
        4 => {
            independent_input(a, false);
            independent_input(a, true);
            a.op(0x03).push([0, 1, 15, 31][variant]).op(0x0b);
        }
        5 => {
            independent_input(a, false);
            independent_input(a, true);
            a.op(0x02);
            independent_input(a, false);
            independent_input(a, true);
            a.op(0x05).op(0x01);
        }
        _ => unreachable!(),
    }
}

fn generated_condition(a: &mut Asm, recipe: usize, variant: usize) {
    generated_expression(a, recipe, variant);
    generated_expression(a, (recipe + 2) % 6, variant);
    a.op([0x10, 0x11, 0x12, 0x13][variant]);
}

fn generated_program(recipe: usize, variant: usize) -> Input {
    let mut a = Asm::default();
    generated_condition(&mut a, recipe, variant);
    a.target("outer_true").op(0x57).push(20).push(0).op(0x55);
    generated_condition(&mut a, (recipe + 1) % 6, (variant + 1) % 4);
    a.target("false_inner_true")
        .op(0x57)
        .push(22)
        .push(1)
        .op(0x55)
        .target("join")
        .op(0x56)
        .dest("false_inner_true")
        .push(21)
        .push(1)
        .op(0x55)
        .target("join")
        .op(0x56)
        .dest("outer_true")
        .push(10)
        .push(0)
        .op(0x55);
    generated_condition(&mut a, (recipe + 3) % 6, (variant + 3) % 4);
    a.op(0x15)
        .target("true_inner_true")
        .op(0x57)
        .push(12)
        .push(1)
        .op(0x55)
        .target("join")
        .op(0x56)
        .dest("true_inner_true")
        .push(11)
        .push(1)
        .op(0x55)
        .dest("join");
    // The join receives both outer arms. Repeating the original predicate
    // catches a predecessor's constraint being retained as unconditional.
    generated_condition(&mut a, recipe, variant);
    a.target("joined_true")
        .op(0x57)
        .push(32)
        .push(2)
        .op(0x55)
        .target("end")
        .op(0x56)
        .dest("joined_true")
        .push(31)
        .push(2)
        .op(0x55)
        .dest("end");
    generated_expression(&mut a, (recipe + 4) % 6, (variant + 2) % 4);
    a.push(3).op(0x55).op(0x00);
    a.finish()
}

#[test]
fn deterministic_word_compositions_preserve_effects_and_sound_facts_at_joins() {
    let sign = U256::from(1) << 255;
    let boundaries = [
        U256::ZERO,
        U256::from(1),
        U256::from(9),
        U256::from(10),
        U256::from(31),
        U256::from(32),
        U256::from(63),
        U256::from(64),
        U256::from(127),
        U256::from(128),
        U256::from(254),
        U256::from(255),
        U256::from(256),
        U256::from(257),
        U256::from(511),
        U256::from(512),
        sign - U256::from(1),
        sign,
        U256::MAX - U256::from(1),
        U256::MAX,
    ];
    let mut inputs: Vec<_> = boundaries
        .iter()
        .enumerate()
        .map(|(i, &x)| (x, boundaries[(i * 7 + 3) % boundaries.len()]))
        .collect();
    inputs.extend([
        (U256::ZERO, U256::MAX),
        (U256::MAX, U256::ZERO),
        (U256::from(1), U256::MAX),
        (U256::MAX, U256::from(1)),
        (sign, sign - U256::from(1)),
        (sign - U256::from(1), sign),
        (U256::ZERO, U256::ZERO),
        (U256::MAX, U256::MAX),
        (U256::from(255), U256::from(256)),
        (U256::from(256), U256::from(255)),
        (U256::from(31), U256::from(32)),
        (U256::from(32), U256::from(31)),
    ]);
    let mut outer_outcomes = std::collections::BTreeSet::new();
    for recipe in 0..6 {
        for variant in 0..4 {
            let input = generated_program(recipe, variant);
            let artifact = compile(&input, Strategy::Dispatcher);
            for &(x, y) in &inputs {
                let mut initial = host(&input, x);
                initial.calldata = y.to_be_bytes::<32>().to_vec();
                let (_, observed) = oracle(&input, &artifact, initial);
                let first = observed.storage[&U256::ZERO];
                outer_outcomes.insert(first);
                assert_eq!(
                    observed.storage[&U256::from(2)],
                    U256::from(if first == U256::from(10) { 31 } else { 32 })
                );
                let nested = observed.storage[&U256::from(1)];
                let expected = if first == U256::from(10) {
                    [11, 12]
                } else {
                    [21, 22]
                };
                assert!(
                    expected
                        .into_iter()
                        .map(U256::from)
                        .any(|value| value == nested)
                );
            }
        }
    }
    assert_eq!(
        outer_outcomes,
        [U256::from(10), U256::from(20)].into_iter().collect()
    );
}
