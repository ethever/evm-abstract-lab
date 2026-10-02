//! 解码与基本块划分：先弄清哪些字节是指令，才能讨论控制流。
//!
//! 特别容易踩坑：`60 5b` 是 `PUSH1 0x5b`，第二个字节不是 `JUMPDEST`。
//! PUSH 缺失的立即数按 EVM 规则在**右边**补零，例如 `61 ab` 推入 `0xab00`。
//! 指令名与栈输入/输出数量来自 `revm-bytecode`，这里不维护另一套 opcode 表。

use alloy_primitives::U256;
use revm_bytecode::opcode::{self, OpCode};
use serde::Serialize;
use std::collections::BTreeMap;
use thiserror::Error;

/// 一条指令。`pc` 是字节偏移，不是第几条指令。
#[derive(Clone, Debug, Serialize)]
pub struct Instruction {
    /// Opcode 在原始字节码中的位置。
    pub pc: usize,
    /// 原始 opcode 字节。
    pub opcode: u8,
    /// PUSH 的值；缺失字节已经补零。
    pub immediate: Option<U256>,
    /// opcode + 声明的立即数字节数，包括缺失的零字节。
    pub size: usize,
}

impl Instruction {
    /// 使用上游定义的名称显示指令。
    pub fn name(&self) -> &'static str {
        OpCode::name_by_op(self.opcode)
    }

    /// 上游表也包含未来 fork 的指令；本实验只接受 Cancun legacy 指令。
    pub fn is_valid(&self) -> bool {
        OpCode::new(self.opcode).is_some()
            && (self.opcode <= 0x4a
                || (0x50..=0xa4).contains(&self.opcode)
                || matches!(self.opcode, 0xf0..=0xf5 | 0xfa | 0xfd | 0xff))
            && self.opcode != 0x1e // CLZ 属于后续 fork，不能提前启用。
    }

    /// 正常控制流在这里结束，或必须分叉。
    pub fn ends_block(&self) -> bool {
        !self.is_valid()
            || matches!(self.opcode, opcode::JUMP | opcode::JUMPI)
            || OpCode::new_or_unknown(self.opcode).info().is_terminating()
    }

    /// 通用栈操作的输入/输出数。DUP/SWAP 要由调用方保留原值的身份。
    pub fn stack_io(&self) -> (usize, usize) {
        let (inputs, outputs) = OpCode::new_or_unknown(self.opcode).input_output();
        (usize::from(inputs), usize::from(outputs))
    }
}

/// 线性执行的一段指令；编号按字节偏移递增。
#[derive(Clone, Debug, Serialize)]
pub struct BasicBlock {
    /// 在 `Program::blocks()` 中的编号。
    pub id: usize,
    /// 首条指令的字节偏移。
    pub start_pc: usize,
    /// 按程序顺序排列的指令。
    pub instructions: Vec<Instruction>,
}

/// 已解码的程序；索引私有，避免调用者修改指令后索引失效。
#[derive(Clone, Debug, Serialize)]
pub struct Program {
    byte_len: usize,
    blocks: Vec<BasicBlock>,
    #[serde(skip)]
    jumpdest_blocks: BTreeMap<usize, usize>,
}

/// 文本输入或不支持的格式错误；无效 legacy opcode 则是程序中的异常终止。
#[derive(Debug, Error)]
pub enum DecodeError {
    /// EVM bytecode 的十六进制文本必须成对。
    #[error("hex input has an odd number of digits: {0}")]
    OddHexLength(usize),
    /// 字符不是十六进制数。
    #[error("invalid hex character at digit {index}: {character:?}")]
    InvalidHex {
        /// 清理空白和 `0x` 前缀后的字符位置。
        index: usize,
        /// 出错字符。
        character: char,
    },
    /// EOF 有独立的容器和指令语义，不能按 legacy 切块。
    #[error("EOF containers are unsupported; provide Cancun legacy runtime bytecode")]
    UnsupportedEof,
}

impl Program {
    /// 接受可含空白、可带 `0x` 前缀的十六进制 runtime bytecode。
    pub fn from_hex(input: &str) -> Result<Self, DecodeError> {
        let compact: String = input.chars().filter(|c| !c.is_whitespace()).collect();
        let digits = compact.strip_prefix("0x").unwrap_or(&compact);
        if !digits.len().is_multiple_of(2) {
            return Err(DecodeError::OddHexLength(digits.len()));
        }
        let mut bytes = Vec::with_capacity(digits.len() / 2);
        let mut high = None;
        for (index, character) in digits.chars().enumerate() {
            let digit = character
                .to_digit(16)
                .ok_or(DecodeError::InvalidHex { index, character })? as u8;
            match high.take() {
                None => high = Some(digit),
                Some(first) => bytes.push((first << 4) | digit),
            }
        }
        Self::decode(&bytes)
    }

    /// 解码完整字节码，保留 halt 后的块供学习不可达代码。
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.starts_with(&[0xef, 0x00]) {
            return Err(DecodeError::UnsupportedEof);
        }
        let mut instructions = Vec::new();
        let mut pc = 0;
        while pc < bytes.len() {
            let op = bytes[pc];
            let width = if (opcode::PUSH1..=opcode::PUSH32).contains(&op) {
                usize::from(op - opcode::PUSH1 + 1)
            } else {
                0
            };
            let immediate = if op == opcode::PUSH0 {
                Some(U256::ZERO)
            } else if width > 0 {
                let available = width.min(bytes.len() - pc - 1);
                let mut word = [0_u8; 32];
                word[32 - width..32 - width + available]
                    .copy_from_slice(&bytes[pc + 1..pc + 1 + available]);
                Some(U256::from_be_bytes(word))
            } else {
                None
            };
            instructions.push(Instruction {
                pc,
                opcode: op,
                immediate,
                size: width + 1,
            });
            pc += width + 1;
        }

        let mut blocks: Vec<BasicBlock> = Vec::new();
        let mut starts_new = true;
        for instruction in instructions {
            if starts_new || instruction.opcode == opcode::JUMPDEST {
                blocks.push(BasicBlock {
                    id: blocks.len(),
                    start_pc: instruction.pc,
                    instructions: Vec::new(),
                });
            }
            starts_new = instruction.ends_block();
            blocks
                .last_mut()
                .expect("a block was just created")
                .instructions
                .push(instruction);
        }
        let jumpdest_blocks = blocks
            .iter()
            .filter_map(|block| {
                (block.instructions[0].opcode == opcode::JUMPDEST)
                    .then_some((block.start_pc, block.id))
            })
            .collect();
        Ok(Self {
            byte_len: bytes.len(),
            blocks,
            jumpdest_blocks,
        })
    }

    /// 原始字节码长度，供 CODESIZE 抽象执行使用。
    pub fn byte_len(&self) -> usize {
        self.byte_len
    }

    /// 基本块，包括尚未证明可达的块。
    pub fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }

    /// 只有真实指令边界上的 JUMPDEST 才是合法动态跳转目标。
    pub fn jumpdest_blocks(&self) -> &BTreeMap<usize, usize> {
        &self.jumpdest_blocks
    }
}
