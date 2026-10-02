//! 单个基本块的抽象执行。正常出边携带同一个弹栈后的状态。
//! 这里不操作工作表，也不决定何时合并；避免把语义和调度混在一起。

use super::{DiagnosticKind, EdgeKind};
use crate::{
    bytecode::Program,
    domain::{Domain, Value},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

pub(super) struct Successor {
    pub block: usize,
    pub kind: EdgeKind,
}

pub(super) struct Execution {
    pub stack: Vec<Value>,
    pub executed_pcs: Vec<usize>,
    pub successors: Vec<Successor>,
    pub diagnostics: Vec<(usize, DiagnosticKind)>,
    pub jump_pc: Option<usize>,
}

pub(super) fn execute(
    program: &Program,
    block_id: usize,
    entry: &[Value],
    domain: Domain,
) -> Execution {
    let block = &program.blocks()[block_id];
    let mut result = Execution {
        stack: entry.to_vec(),
        executed_pcs: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        jump_pc: None,
    };
    for instruction in &block.instructions {
        let pc = instruction.pc;
        let op = instruction.opcode;
        result.executed_pcs.push(pc);
        if !instruction.is_valid() {
            result.diagnostics.push((pc, DiagnosticKind::InvalidOpcode));
            return result;
        }
        let (inputs, outputs) = instruction.stack_io();
        if result.stack.len() < inputs {
            result
                .diagnostics
                .push((pc, DiagnosticKind::StackUnderflow));
            return result;
        }
        if result.stack.len() - inputs + outputs > 1024 {
            result.diagnostics.push((pc, DiagnosticKind::StackOverflow));
            return result;
        }

        if let Some(value) = instruction.immediate {
            result.stack.push(Value::constant(value));
        } else if (opcode::DUP1..=opcode::DUP16).contains(&op) {
            let depth = usize::from(op - opcode::DUP1 + 1);
            result
                .stack
                .push(result.stack[result.stack.len() - depth].clone());
        } else if (opcode::SWAP1..=opcode::SWAP16).contains(&op) {
            let top = result.stack.len() - 1;
            result
                .stack
                .swap(top, top - usize::from(op - opcode::SWAP1 + 1));
        } else {
            // revm 的 stack_io 对 DUP/SWAP 表示观察深度，因此它们必须在上面
            // 特殊处理。通用指令这里按“栈顶先弹出”收集参数。
            let mut args: Vec<Value> = result.stack.drain(result.stack.len() - inputs..).collect();
            args.reverse();
            if matches!(op, opcode::JUMP | opcode::JUMPI) {
                result.jump_pc = Some(block.start_pc);
                let condition = args.get(1);
                if condition.is_none_or(Value::may_be_nonzero) {
                    add_jump_targets(
                        program,
                        &args[0],
                        pc,
                        &mut result,
                        if op == opcode::JUMP {
                            EdgeKind::Jump
                        } else {
                            EdgeKind::BranchTrue
                        },
                    );
                }
                // 条件为零时根本不检查跳转目标合法性。这条顺序关系也属于语义。
                if condition.is_some_and(Value::may_be_zero)
                    && block_id + 1 < program.blocks().len()
                {
                    result.successors.push(Successor {
                        block: block_id + 1,
                        kind: EdgeKind::BranchFalse,
                    });
                }
                return result;
            }
            if outputs > 0 {
                let value = match op {
                    opcode::PC => Value::constant(U256::from(pc)),
                    opcode::CODESIZE => Value::constant(U256::from(program.byte_len())),
                    _ => domain.apply(op, &args),
                };
                if !matches!(op, 0x01..=0x0b | 0x10..=0x1d | opcode::PC | opcode::CODESIZE) {
                    result.diagnostics.push((pc, DiagnosticKind::OpaqueResult));
                }
                result.stack.extend(std::iter::repeat_n(value, outputs));
            }
            if instruction.ends_block() {
                return result;
            }
        }
    }
    if block_id + 1 < program.blocks().len() {
        result.successors.push(Successor {
            block: block_id + 1,
            kind: EdgeKind::Fallthrough,
        });
    }
    result
}

fn add_jump_targets(
    program: &Program,
    target: &Value,
    pc: usize,
    result: &mut Execution,
    kind: EdgeKind,
) {
    match target.constants() {
        Some(values) => {
            for value in values {
                let valid = usize::try_from(*value)
                    .ok()
                    .and_then(|target_pc| program.jumpdest_blocks().get(&target_pc).copied());
                if let Some(block) = valid {
                    result.successors.push(Successor { block, kind });
                } else {
                    result.diagnostics.push((pc, DiagnosticKind::InvalidJump));
                }
            }
        }
        None => {
            result.diagnostics.push((pc, DiagnosticKind::UnknownJump));
            for block in program.jumpdest_blocks().values() {
                result.successors.push(Successor {
                    block: *block,
                    kind,
                });
            }
        }
    }
}
