//! 解码与基本块划分：先弄清哪些字节是指令，才能讨论控制流。
//!
//! 特别容易踩坑：`60 5b` 是 `PUSH1 0x5b`，第二个字节不是 `JUMPDEST`。
//! PUSH 缺失的立即数按 EVM 规则在**右边**补零，例如 `61 ab` 推入 `0xab00`。
//! 指令名与栈输入/输出数量来自 `revm-bytecode`，这里不维护另一套 opcode 表。

use crate::Fork;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use revm_bytecode::{
    Bytecode,
    eip7702::{EIP7702_MAGIC_BYTES, Eip7702DecodeError},
    opcode::{self, OpCode},
};
use serde::{Serialize, Serializer};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, OnceLock},
};
use thiserror::Error;

#[cfg(test)]
mod tests;

/// 一条指令。`pc` 是字节偏移，不是第几条指令。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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

/// Why a decoded opcode exceptionally halts under the program's fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum InvalidOpcodeReason {
    /// The defined `0xfe INVALID` instruction deliberately halts exceptionally.
    ExplicitInvalid,
    /// The opcode has no instruction definition in ordinary EVM bytecode.
    UndefinedOpcode,
    /// The instruction is defined upstream but is not enabled in this fork.
    UnavailableInFork(Fork),
}

impl fmt::Display for InvalidOpcodeReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExplicitInvalid => f.write_str("explicit exceptional-halt instruction"),
            Self::UndefinedOpcode => f.write_str("undefined ordinary-bytecode opcode"),
            Self::UnavailableInFork(fork) => write!(f, "instruction unavailable in fork {fork}"),
        }
    }
}

impl Instruction {
    /// 使用上游定义的名称显示指令。
    pub fn name(&self) -> &'static str {
        OpCode::name_by_op(self.opcode)
    }

    /// A defined mnemonic, excluding the upstream placeholder for unknown bytes.
    pub fn mnemonic(&self) -> Option<&'static str> {
        OpCode::new(self.opcode).map(|_| self.name())
    }

    /// The same fork-specific rule used by validity checks and execution faults.
    pub fn invalid_reason(&self) -> Option<InvalidOpcodeReason> {
        if self.opcode == opcode::INVALID {
            Some(InvalidOpcodeReason::ExplicitInvalid)
        } else if OpCode::new(self.opcode).is_none() {
            Some(InvalidOpcodeReason::UndefinedOpcode)
        } else if !(self.opcode <= 0x4a
            || (0x50..=0xa4).contains(&self.opcode)
            || matches!(self.opcode, 0xf0..=0xf5 | 0xfa | 0xfd | 0xff))
            || (self.opcode == opcode::CLZ && !self.fork.supports_clz())
        {
            Some(InvalidOpcodeReason::UnavailableInFork(self.fork))
        } else {
            None
        }
    }

    /// 使用解码时选择的 fork。上游包含未来指令，不等于它们已在主网启用。
    pub fn is_valid(&self) -> bool {
        self.invalid_reason().is_none()
    }

    /// 正常控制流在这里结束，或必须分叉。
    pub fn ends_block(&self) -> bool {
        !self.is_valid()
            || matches!(self.opcode, opcode::JUMP | opcode::JUMPI)
            // A call suspends this frame. Its next instruction is a continuation
            // block, reached only when a child returns or the call fails.
            || matches!(self.opcode, opcode::CALL | opcode::CALLCODE | opcode::DELEGATECALL | opcode::STATICCALL)
            || matches!(self.opcode, opcode::CREATE | opcode::CREATE2)
            || OpCode::new_or_unknown(self.opcode).info().is_terminating()
    }

    /// 通用栈操作的输入/输出数。DUP/SWAP 要由调用方保留原值的身份。
    pub fn stack_io(&self) -> (usize, usize) {
        let (inputs, outputs) = OpCode::new_or_unknown(self.opcode).input_output();
        (usize::from(inputs), usize::from(outputs))
    }
}

/// 线性执行的一段指令；编号按字节偏移递增。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BasicBlock {
    /// 当前 [`Program::blocks`] 中的稠密索引；不是跨程序的全局编号。
    /// 字节地址由 `start_pc` 保存，带上下文的分析状态使用 [`crate::analysis::StateKey`]。
    pub id: usize,
    /// 首条指令的字节偏移。
    pub start_pc: usize,
    /// 按程序顺序排列的指令。
    pub instructions: Vec<Instruction>,
}

/// 已解码的程序；私有不可变数据在帧、回滚快照和摘要之间共享。
/// 值相等仍比较完整程序，克隆不复制指令或索引。
#[derive(Clone, PartialEq, Eq)]
pub struct Program {
    data: Arc<ProgramData>,
}

#[derive(Debug, Eq, Serialize)]
#[serde(rename = "Program")]
struct ProgramData {
    fork: Fork,
    byte_len: usize,
    #[serde(skip)]
    bytes: Vec<u8>,
    #[serde(skip)]
    bytes_digest: OnceLock<B256>,
    blocks: Vec<BasicBlock>,
    #[serde(skip)]
    jumpdest_blocks: BTreeMap<usize, usize>,
}

impl PartialEq for ProgramData {
    fn eq(&self, other: &Self) -> bool {
        // Cache initialization is not part of the captured program's value.
        self.fork == other.fork
            && self.byte_len == other.byte_len
            && self.bytes == other.bytes
            && self.blocks == other.blocks
            && self.jumpdest_blocks == other.jumpdest_blocks
    }
}

impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Keep the original derived layout without explicit trait-object fields.
        #[derive(Debug)]
        #[expect(
            dead_code,
            reason = "fields are read by the derived Debug implementation"
        )]
        struct Program<'a> {
            fork: &'a Fork,
            byte_len: &'a usize,
            bytes: &'a Vec<u8>,
            blocks: &'a Vec<BasicBlock>,
            jumpdest_blocks: &'a BTreeMap<usize, usize>,
        }
        let view = Program {
            fork: &self.data.fork,
            byte_len: &self.data.byte_len,
            bytes: &self.data.bytes,
            blocks: &self.data.blocks,
            jumpdest_blocks: &self.data.jumpdest_blocks,
        };
        fmt::Debug::fmt(&view, f)
    }
}

impl Serialize for Program {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.data.as_ref().serialize(serializer)
    }
}

/// 文本输入或不支持的格式错误；普通 EVM 字节码中的无效操作码则是程序中的异常终止。
/// Nested delegation failures are retained in [`DecodeError::InvalidDelegation`];
/// callers inspect that concrete variant instead of an erased error source chain.
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
    /// EOF 有独立的容器和指令语义，不能按普通 EVM 指令流切块。
    #[error("EOF containers are unsupported; provide ordinary EVM runtime bytecode")]
    UnsupportedEof,
    /// [EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) 标记是账户代码指针，不是被执行的指令流。不能输出一个虚假空 CFG。
    #[error("EIP-7702 delegates execution to {address}; analyze that account's runtime bytecode")]
    DelegatedCode {
        /// 原始 marker 中的目标地址，不自动访问 RPC。
        address: Address,
    },
    /// 委托格式验证复用 revm 的解析器，保留长度/版本等具体错误。
    #[error("malformed EIP-7702 delegation indicator: {0}")]
    InvalidDelegation(Eip7702DecodeError),
}

impl From<Eip7702DecodeError> for DecodeError {
    fn from(error: Eip7702DecodeError) -> Self {
        Self::InvalidDelegation(error)
    }
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
            // Program 只表示实际执行的指令流；账户指针属于 world 的事实层。
            // Account::from_hex 保留此目标，跨合约引擎按固定 world 做单层解析。
            // 字节码独立入口仍返回目标，不能把 marker 误解成完成的空 CFG。
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
                    // 这次解码只按指令顺序追加块：已有 B0..B(n-1) 时，len=n，
                    // 所以新块的编号 n 唯一且连续，之后可直接用 blocks[id] 查找。
                    // Program 解码完成后只提供只读访问，不删除或重排块，编号不会失效。
                    // 同一原始块的不同路径/上下文由 StateKey 区分，不靠这里的 B 编号。
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
            data: Arc::new(ProgramData {
                fork,
                byte_len: bytes.len(),
                bytes: bytes.to_vec(),
                bytes_digest: OnceLock::new(),
                blocks,
                jumpdest_blocks,
            }),
        })
    }

    /// 本程序的执行层规则；也是其 CFG/SSA 与 JSON 输出使用的规则。
    pub fn fork(&self) -> Fork {
        self.data.fork
    }

    /// 原始字节码长度，供 CODESIZE 抽象执行使用。
    pub fn byte_len(&self) -> usize {
        self.data.byte_len
    }

    /// Immutable source bytes used by CODECOPY and EXTCODECOPY.
    pub fn bytes(&self) -> &[u8] {
        &self.data.bytes
    }

    /// Keccak of the literal captured bytes, shared by all handles of this
    /// immutable program. Callers retain their cold-byte work allowance even
    /// after initialization; cache warmth must never change analysis quotes.
    pub(crate) fn bytes_digest(&self) -> B256 {
        *self
            .data
            .bytes_digest
            .get_or_init(|| keccak256(self.bytes()))
    }

    /// 基本块，包括尚未证明可达的块。
    pub fn blocks(&self) -> &[BasicBlock] {
        &self.data.blocks
    }

    /// 只有真实指令边界上的 JUMPDEST 才是合法动态跳转目标。
    pub fn jumpdest_blocks(&self) -> &BTreeMap<usize, usize> {
        &self.data.jumpdest_blocks
    }
}
