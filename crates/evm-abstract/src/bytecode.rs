//! Shared EVM runtime decoding and immutable program captures.

pub use evm_bytecode::bytecode::{
    BasicBlock, DecodeError, Instruction, InvalidOpcodeReason, Program,
};
