//! Immutable EVM runtime decoding, explicit fork rules, and pure Word256 semantics.
//!
//! These foundations are independent of execution worlds and numeric abstractions.

pub mod bytecode;
pub mod fork;
pub mod word;

pub use alloy_primitives::U256;
pub use bytecode::{BasicBlock, DecodeError, Instruction, InvalidOpcodeReason, Program};
pub use fork::{Fork, ParseForkError};
