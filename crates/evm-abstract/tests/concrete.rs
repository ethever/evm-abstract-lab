//! 用现成的 revm 作为独立的具体执行 oracle，避免测试与抽象实现共享同一错误。
//! 有限样本能发现反例，不能代替全部 256 bit 状态空间上的正确性证明。

use evm_abstract::{
    Fork, U256,
    analysis::{self, Analysis, Config, EdgeKind, Status},
    bytecode::Program,
    domain::{Domain, Value, provenance::Origin},
    ssa,
};
use proptest::{arbitrary::any, prop_assert_eq, proptest};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::TxEnv,
    database::{BENCH_CALLER, BENCH_TARGET, BenchmarkDB},
    interpreter::{Interpreter, interpreter::EthInterpreter, interpreter_types::Jumps},
    primitives::{Bytes, TxKind},
    state::Bytecode,
};

#[derive(Clone, Debug)]
struct Step {
    pc: usize,
    opcode: u8,
    before: Vec<U256>,
    after: Vec<U256>,
}
#[derive(Default)]
struct Trace {
    steps: Vec<Step>,
}

impl<CTX> Inspector<CTX, EthInterpreter> for Trace {
    fn step(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        self.steps.push(Step {
            pc: interpreter.bytecode.pc(),
            opcode: interpreter.bytecode.opcode(),
            before: interpreter.stack.data().clone(),
            after: Vec::new(),
        });
    }
    fn step_end(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        self.steps.last_mut().unwrap().after = interpreter.stack.data().clone();
    }
}

fn concrete(code: &[u8], calldata: Vec<u8>, fork: Fork) -> Trace {
    let context = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(fork.spec_id()))
        .with_db(BenchmarkDB::new_bytecode(Bytecode::new_raw(
            Bytes::copy_from_slice(code),
        )));
    let mut evm = context.build_mainnet_with_inspector(Trace::default());
    let result = evm
        .inspect_one_tx(
            TxEnv::builder()
                .caller(BENCH_CALLER)
                .kind(TxKind::Call(BENCH_TARGET))
                .gas_limit(1_000_000)
                .data(calldata.into())
                .build()
                .unwrap(),
        )
        .unwrap();
    assert!(
        result.is_success(),
        "concrete oracle did not succeed: {result:?}"
    );
    evm.inspector
}

fn arithmetic(op: u8, args: &[U256], fork: Fork) -> U256 {
    let mut code = Vec::new();
    for value in args.iter().rev() {
        code.push(0x7f); // PUSH32，测试夹具只负责拼装，opcode 的语义交给 revm。
        code.extend(value.to_be_bytes::<32>());
    }
    code.extend([op, 0x00]);
    let trace = concrete(&code, Vec::new(), fork);
    *trace
        .steps
        .iter()
        .find(|step| step.opcode == op && step.pc == args.len() * 33)
        .unwrap()
        .after
        .last()
        .unwrap()
}

// 具体 oracle 判断数值；运算来源与直接常量来源应当保持可区分。
fn assert_exact_numeric(actual: &Value, expected: U256) {
    let literal = Value::constant(expected);
    assert_eq!(actual.singleton(), Some(expected));
    assert_eq!(actual.constants(), literal.constants());
    assert_eq!(actual.known_bits(), literal.known_bits());
    assert_eq!(actual.interval(), literal.interval());
    assert_eq!(actual.congruence(), literal.congruence());
    assert!(
        actual
            .provenance()
            .origins()
            .sources()
            .is_some_and(|sources| {
                sources.contains(&Origin::Constant) && sources.contains(&Origin::Arithmetic)
            })
    );
}

#[test]
fn all_pure_operators_match_revm_at_word_boundaries() {
    let values = [
        U256::ZERO,
        U256::from(1),
        U256::from(2),
        U256::from(31),
        U256::from(32),
        U256::from(255),
        U256::from(256),
        U256::from(1) << 255,
        U256::MAX,
    ];
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let last_bitwise = if fork == Fork::Osaka { 0x1e } else { 0x1d };
        for op in (0x01..=0x0b).chain(0x10..=last_bitwise) {
            for index in 0..values.len() {
                let count = match op {
                    0x15 | 0x19 | 0x1e => 1,
                    0x08 | 0x09 => 3,
                    _ => 2,
                };
                let args: Vec<_> = (0..count)
                    .map(|offset| values[(index + offset) % values.len()])
                    .collect();
                let abstract_args: Vec<_> = args.iter().copied().map(Value::constant).collect();
                let result = Domain::default().apply(op, &abstract_args);
                let expected = arithmetic(op, &args, fork);
                assert_eq!(
                    result.singleton(),
                    Some(expected),
                    "fork {fork}, opcode 0x{op:02x}, args {args:?}"
                );
                assert_exact_numeric(&result, expected);
            }
        }
    }
}

proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(96))]
    #[test]
    fn random_256bit_pure_transfers_match_revm(op_index in 0usize..26, a in any::<[u64; 4]>(), b in any::<[u64; 4]>(), c in any::<[u64; 4]>()) {
        let operators: Vec<_> = (0x01..=0x0b).chain(0x10..=0x1e).collect();
        let op = operators[op_index];
        let count = match op { 0x15 | 0x19 | 0x1e => 1, 0x08 | 0x09 => 3, _ => 2 };
        let args = [U256::from_limbs(a), U256::from_limbs(b), U256::from_limbs(c)];
        let abstract_args: Vec<_> = args[..count].iter().copied().map(Value::constant).collect();
        for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
            if op != 0x1e || fork == Fork::Osaka {
                let result = Domain::default().apply(op, &abstract_args);
                let expected = arithmetic(op, &args[..count], fork);
                prop_assert_eq!(result.singleton(), Some(expected));
                assert_exact_numeric(&result, expected);
            }
        }
    }
}

fn covers_trace(analysis: &Analysis, ir: &ssa::Ssa, trace: &Trace) {
    let mut context = Vec::new();
    let mut current: Option<usize> = None;
    let mut previous: Option<&Step> = None;
    let mut values = std::collections::BTreeMap::new();
    for step in &trace.steps {
        if let Some(block) = analysis
            .program()
            .blocks()
            .iter()
            .find(|b| b.start_pc == step.pc)
        {
            let mut incoming_kind = EdgeKind::Fallthrough;
            if let (Some(from), Some(last)) = (current, previous)
                && matches!(last.opcode, 0x56 | 0x57)
            {
                context.push(
                    analysis.program().blocks()[analysis.states()[from].key.basic_block_index]
                        .start_pc,
                );
                let discard = context
                    .len()
                    .saturating_sub(analysis.config().context_depth);
                context.drain(..discard);
                incoming_kind = if last.opcode == 0x56 {
                    EdgeKind::Jump
                } else if last.before[last.before.len() - 2] == U256::ZERO {
                    EdgeKind::BranchFalse
                } else {
                    EdgeKind::BranchTrue
                };
            }
            let state = analysis
                .states()
                .iter()
                .find(|s| {
                    s.key.basic_block_index == block.id
                        && s.key.stack_height == step.before.len()
                        && s.key.context == context
                })
                .unwrap_or_else(|| panic!("missing concrete block entry @ {}", step.pc));
            assert!(
                state
                    .entry_stack
                    .iter()
                    .zip(&step.before)
                    .all(|(abstract_value, concrete)| abstract_value.contains(*concrete))
            );
            if let Some(from) = current {
                assert!(
                    analysis
                        .edges()
                        .iter()
                        .any(|e| e.from == from && e.to == state.id && e.kind == incoming_kind),
                    "missing concrete edge S{from} -> S{} {incoming_kind:?}",
                    state.id
                );
                // φ 是并行赋值：先核对所有旧边输入，再更新本次入口的值。
                for phi in &ir.blocks()[state.id].phis {
                    let input = phi
                        .inputs
                        .iter()
                        .find(|input| input.predecessor == from)
                        .unwrap();
                    assert_eq!(values[&input.value], step.before[phi.slot]);
                }
            }
            for phi in &ir.blocks()[state.id].phis {
                values.insert(phi.result, step.before[phi.slot]);
            }
            current = Some(state.id);
        }
        let state = &analysis.states()[current.expect("concrete execution starts at pc=0")];
        let item = ir.blocks()[state.id]
            .instructions
            .iter()
            .find(|item| item.pc == step.pc)
            .unwrap();
        assert_eq!(item.opcode, step.opcode);
        assert!(!item.fault);
        // 实际 revm 栈提供操作数真值，SSA 名字必须指向相同的动态值。
        let expected_args: Vec<_> = match step.opcode {
            0x80..=0x8f => {
                vec![step.before[step.before.len() - usize::from(step.opcode - 0x80 + 1)]]
            }
            0x90..=0x9f => vec![
                *step.before.last().unwrap(),
                step.before[step.before.len() - usize::from(step.opcode - 0x90 + 2)],
            ],
            _ => step
                .before
                .iter()
                .rev()
                .take(usize::from(
                    revm::bytecode::opcode::OpCode::new(step.opcode)
                        .unwrap()
                        .inputs(),
                ))
                .copied()
                .collect(),
        };
        let actual_args: Vec<_> = item.operands.iter().map(|value| values[value]).collect();
        assert_eq!(
            actual_args, expected_args,
            "SSA operands differ @ {}",
            step.pc
        );
        for (value, concrete) in item
            .results
            .iter()
            .zip(&step.after[step.after.len() - item.results.len()..])
        {
            values.insert(*value, *concrete);
        }
        assert!(
            state.executed_pcs.contains(&step.pc),
            "lost opcode @ {}",
            step.pc
        );
        if state.executed_pcs.last() == Some(&step.pc) {
            let actual_output: Vec<_> = ir.blocks()[state.id]
                .exit_stack
                .iter()
                .map(|value| values[value])
                .collect();
            assert_eq!(
                actual_output, step.after,
                "SSA outgoing values differ @ {}",
                step.pc
            );
            assert_eq!(state.exit_stack.len(), step.after.len());
            assert!(
                state
                    .exit_stack
                    .iter()
                    .zip(&step.after)
                    .all(|(abstract_value, concrete)| abstract_value.contains(*concrete))
            );
        }
        previous = Some(step);
    }
}

#[test]
fn example_cfgs_cover_revm_block_entries_edges_and_outputs() {
    for name in [
        "straight-line",
        "diamond",
        "loop",
        "dynamic-jump",
        "internal-calls",
        "stack-heights",
        "osaka-clz",
    ] {
        let hex = std::fs::read_to_string(format!(
            "{}/../../examples/{name}.hex",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let code = revm::primitives::hex::decode(hex.trim()).unwrap();
        for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
            if name == "osaka-clz" && fork != Fork::Osaka {
                continue;
            }
            for depth in [0, 1, 2, 8, 10] {
                let analysis = analysis::analyze(
                    Program::decode_with_fork(&code, fork).unwrap(),
                    Config {
                        context_depth: depth,
                        ..Config::default()
                    },
                )
                .unwrap();
                assert_eq!(
                    analysis.status(),
                    Status::Converged,
                    "example={name} fork={fork:?} depth={depth} frontiers={:?}",
                    analysis.frontiers()
                );
                let ir = ssa::build(&analysis).unwrap();
                let inputs = if name == "dynamic-jump" {
                    vec![4]
                } else {
                    vec![0, 1]
                };
                for input in inputs {
                    let mut calldata = vec![0_u8; 32];
                    calldata[31] = input;
                    covers_trace(&analysis, &ir, &concrete(&code, calldata, fork));
                }
            }
        }
    }
}

#[test]
fn clz_matches_revm_for_zero_and_every_single_set_bit() {
    for value in std::iter::once(U256::ZERO).chain((0..256).map(|bit| U256::from(1) << bit)) {
        assert_exact_numeric(
            &Domain::default().apply(0x1e, &[Value::constant(value)]),
            arithmetic(0x1e, &[value], Fork::Osaka),
        );
    }
}
