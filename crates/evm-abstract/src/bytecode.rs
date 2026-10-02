//! 解码与基本块划分：先弄清哪些字节是指令，才能讨论控制流。
//!
//! 特别容易踩坑：`60 5b` 是 `PUSH1 0x5b`，第二个字节不是 `JUMPDEST`。
//! PUSH 缺失的立即数按 EVM 规则在**右边**补零，例如 `61 ab` 推入 `0xab00`。
//! 指令名与栈输入/输出数量来自 `revm-bytecode`，这里不维护另一套 opcode 表。

use crate::Fork;
use alloy_primitives::{Address, Bytes, U256};
use revm_bytecode::{
    Bytecode,
    eip7702::{EIP7702_MAGIC_BYTES, Eip7702DecodeError},
    opcode::{self, OpCode},
};
use serde::Serialize;
use std::collections::BTreeMap;
use thiserror::Error;

/// 一条指令。`pc` 是字节偏移，不是第几条指令。
#[derive(Clone, Debug, Serialize)]
pub struct Instruction {
    // 与 Program 一起在解码时固定；调用方不能只把某一条指令改成别的 fork。
    #[serde(skip)]
    fork: Fork,
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

    /// 使用解码时选择的 fork。上游包含未来指令，不等于它们已在主网启用。
    pub fn is_valid(&self) -> bool {
        OpCode::new(self.opcode).is_some()
            && (self.opcode <= 0x4a
                || (0x50..=0xa4).contains(&self.opcode)
                || matches!(self.opcode, 0xf0..=0xf5 | 0xfa | 0xfd | 0xff))
            && (self.opcode != opcode::CLZ || self.fork.supports_clz())
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
    fork: Fork,
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
    #[error("EOF containers are unsupported; provide legacy runtime bytecode")]
    UnsupportedEof,
    /// EIP-7702 标记是账户代码指针，不是被执行的指令流。不能输出一个虚假空 CFG。
    #[error("EIP-7702 delegates execution to {address}; analyze that account's runtime bytecode")]
    DelegatedCode {
        /// 原始 marker 中的目标地址，不自动访问 RPC。
        address: Address,
    },
    /// 委托格式验证复用 revm 的解析器，保留长度/版本等具体错误。
    #[error("malformed EIP-7702 delegation indicator: {0}")]
    InvalidDelegation(#[from] Eip7702DecodeError),
}

impl Program {
    /// 接受可含空白、可带 `0x` 前缀的十六进制 runtime bytecode，默认 Osaka。
    pub fn from_hex(input: &str) -> Result<Self, DecodeError> {
        Self::from_hex_with_fork(input, Fork::default())
    }

    /// 用明确的规则集解码。选择版本后，后续分析不会再次单独选择版本。
    pub fn from_hex_with_fork(input: &str, fork: Fork) -> Result<Self, DecodeError> {
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
        Self::decode_with_fork(&bytes, fork)
    }

    /// 使用默认 Osaka 解码，保留 halt 后的块供学习不可达代码。
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::decode_with_fork(bytes, Fork::default())
    }

    /// 按指定 fork 解码完整字节码。EOF 与账户委托不能当作普通指令分析。
    pub fn decode_with_fork(bytes: &[u8], fork: Fork) -> Result<Self, DecodeError> {
        if bytes.starts_with(&[0xef, 0x00]) {
            return Err(DecodeError::UnsupportedEof);
        }
        if fork.supports_delegation() && bytes.starts_with(EIP7702_MAGIC_BYTES) {
            let delegated = Bytecode::new_eip7702_raw(Bytes::copy_from_slice(bytes))?;
            return Err(DecodeError::DelegatedCode {
                address: delegated
                    .eip7702_address()
                    .expect("revm validated a delegation indicator"),
            });
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
                fork,
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
            fork,
            byte_len: bytes.len(),
            blocks,
            jumpdest_blocks,
        })
    }

    /// 本程序的执行层规则；也是其 CFG/SSA 与 JSON 输出使用的规则。
    pub fn fork(&self) -> Fork {
        self.fork
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
