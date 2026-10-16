//! Compilation is independent of addresses, snapshots, and transaction inputs.

use crate::{Fork, bytecode::Program};
use alloy_primitives::{B256, keccak256};
use revm_bytecode::opcode;
use serde::Serialize;
use snapshot_state::OrderedMap;
use std::{collections::BTreeSet, sync::Arc};
use thiserror::Error;

/// The execution mode participates in source identity even for identical bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum CodeMode {
    /// Installed account runtime.
    Runtime,
    /// Constructor code, retained separately from installed runtime.
    Initcode,
}

/// Content identity only; this is never an account-existence observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CodeId {
    fork: Fork,
    mode: CodeMode,
    hash: B256,
}

impl CodeId {
    /// Rules used to compile the bytes.
    pub fn fork(self) -> Fork {
        self.fork
    }

    /// Runtime and initcode have independent source identities.
    pub fn mode(self) -> CodeMode {
        self.mode
    }

    /// Keccak of the original bytes, independent of decoding and mode.
    pub fn hash(self) -> B256 {
        self.hash
    }
}

/// Categories of initial inputs that an instruction may inspect.
///
/// These are static requirements, not a certificate that an executed graph can
/// be reused. In particular, a call may execute unknown code and nested callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum FactKind {
    /// Installed code or a delegation pointer.
    Code,
    /// Initial account-presence information.
    Existence,
    /// Initial balance.
    Balance,
    /// Initial account nonce.
    Nonce,
    /// Persistent storage belonging to the active state owner.
    Storage,
    /// Transaction or block inputs, including calldata and its length.
    Input,
    /// Unknown, indirect, recursive, or reentrant state interaction is possible.
    ExternalInteraction,
}

/// Reusable block semantics compiled without an address or world.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BlockFacts {
    /// Index into the compiled program's blocks.
    pub block: usize,
    /// First byte offset in this source.
    pub start_pc: usize,
    /// Minimum initial stack height for all instructions in the block.
    pub required_stack: usize,
    /// Height after executing with exactly `required_stack` input values.
    pub produced_stack: usize,
    /// Static initial-input requirements, including instructions after a fault.
    pub requirements: BTreeSet<FactKind>,
    /// A persistent state effect may occur; static-call rules still apply at runtime.
    pub may_write_state: bool,
    /// Every opcode uses only the stack and captured code/control position.
    /// Execution additionally checks constant inputs and an unchanged payload
    /// outside the active stack/control delta before retaining a reusable result.
    pub stack_only: bool,
}

/// Decoded bytes and their reusable block semantics. No state observations live here.
#[derive(Clone, Debug)]
pub struct CompiledCode {
    id: CodeId,
    program: Program,
    blocks: Vec<BlockFacts>,
    requirements: BTreeSet<FactKind>,
}

impl CompiledCode {
    /// Content, fork, and mode identity.
    pub fn id(&self) -> CodeId {
        self.id
    }

    /// The immutable decoded program shared with native execution frames.
    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Stack and input requirements for each decoded block.
    pub fn blocks(&self) -> &[BlockFacts] {
        &self.blocks
    }

    /// Union of static requirements over all blocks, including unreachable ones.
    pub fn requirements(&self) -> &BTreeSet<FactKind> {
        &self.requirements
    }
}

/// Immutable compiled code shared by independent initial-state sessions.
#[derive(Clone, Debug)]
pub struct CodeCatalog {
    fork: Fork,
    entries: OrderedMap<CodeId, Arc<CompiledCode>>,
}

/// Incoherent code input is rejected before any analysis can start.
#[derive(Debug, Error)]
pub enum CatalogError {
    /// All programs in a catalog must use the same EVM rules.
    #[error("catalog uses {catalog}, but program uses {program}")]
    MixedFork {
        /// Fixed catalog rules.
        catalog: Fork,
        /// Rejected program rules.
        program: Fork,
    },
    /// Compare bytes as well as hashes before reusing compiled semantics.
    #[error("different code bytes share catalog identity {0:?}")]
    HashCollision(CodeId),
}

impl CodeCatalog {
    /// Start a code-only catalog. No provider, address, or snapshot is required.
    pub fn new(fork: Fork) -> Self {
        Self {
            fork,
            entries: OrderedMap::new(),
        }
    }

    /// Rules fixed for all entries.
    pub fn fork(&self) -> Fork {
        self.fork
    }

    /// Return a new catalog with compiled runtime; existing catalogs stay unchanged.
    pub fn with_runtime(&self, program: Program) -> Result<(Self, CodeId), CatalogError> {
        self.with_program(program, CodeMode::Runtime)
    }

    /// Compile one source mode once, sharing it when the same bytes are supplied again.
    pub fn with_program(
        &self,
        program: Program,
        mode: CodeMode,
    ) -> Result<(Self, CodeId), CatalogError> {
        if program.fork() != self.fork {
            return Err(CatalogError::MixedFork {
                catalog: self.fork,
                program: program.fork(),
            });
        }
        let id = CodeId {
            fork: self.fork,
            mode,
            hash: keccak256(program.bytes()),
        };
        if let Some(existing) = self.entries.get(&id) {
            if existing.program != program {
                return Err(CatalogError::HashCollision(id));
            }
            return Ok((self.clone(), id));
        }
        let blocks: Vec<_> = program
            .blocks()
            .iter()
            .map(|block| {
                let mut required_stack = 0usize;
                let mut height = 0usize;
                let mut requirements = BTreeSet::new();
                let mut may_write_state = false;
                for instruction in &block.instructions {
                    let (inputs, outputs) = instruction.stack_io();
                    if height < inputs {
                        required_stack += inputs - height;
                        height = inputs;
                    }
                    height = height - inputs + outputs;
                    classify(instruction.opcode, &mut requirements, &mut may_write_state);
                }
                BlockFacts {
                    block: block.id,
                    start_pc: block.start_pc,
                    required_stack,
                    produced_stack: height,
                    requirements,
                    may_write_state,
                    stack_only: block
                        .instructions
                        .iter()
                        .all(|i| i.is_valid() && stack_only(i.opcode)),
                }
            })
            .collect();
        let requirements = blocks
            .iter()
            .flat_map(|block| block.requirements.iter().copied())
            .collect();
        let mut next = self.clone();
        next.entries.insert(
            id,
            Arc::new(CompiledCode {
                id,
                program,
                blocks,
                requirements,
            }),
        );
        Ok((next, id))
    }

    /// Compiled semantics by content identity, without an account lookup.
    pub fn get(&self, id: CodeId) -> Option<&Arc<CompiledCode>> {
        self.entries.get(&id)
    }

    /// Decoded bytes consumed by the native executor.
    pub fn program(&self, id: CodeId) -> Option<&Program> {
        self.get(id).map(|code| code.program())
    }

    pub(crate) fn source(&self, hash: B256, mode: CodeMode) -> Option<&CompiledCode> {
        self.get(CodeId {
            fork: self.fork,
            mode,
            hash,
        })
        .map(Arc::as_ref)
    }
}

pub(super) fn classify(op: u8, requirements: &mut BTreeSet<FactKind>, may_write_state: &mut bool) {
    match op {
        opcode::SLOAD | opcode::SSTORE => {
            requirements.insert(FactKind::Storage);
        }
        opcode::BALANCE | opcode::SELFBALANCE => {
            requirements.insert(FactKind::Balance);
        }
        opcode::EXTCODESIZE | opcode::EXTCODECOPY | opcode::EXTCODEHASH => {
            requirements.extend([FactKind::Code, FactKind::Existence]);
        }
        opcode::CALL
        | opcode::CALLCODE
        | opcode::DELEGATECALL
        | opcode::STATICCALL
        | opcode::CREATE
        | opcode::CREATE2
        | opcode::SELFDESTRUCT => {
            requirements.extend([
                FactKind::Code,
                FactKind::Existence,
                FactKind::Balance,
                FactKind::Nonce,
                FactKind::Storage,
                FactKind::Input,
                FactKind::ExternalInteraction,
            ]);
        }
        opcode::ADDRESS
        | opcode::ORIGIN
        | opcode::CALLER
        | opcode::CALLVALUE
        | opcode::CALLDATALOAD
        | opcode::CALLDATASIZE
        | opcode::CALLDATACOPY
        | opcode::GASPRICE
        | opcode::BLOCKHASH
        | opcode::COINBASE
        | opcode::TIMESTAMP
        | opcode::NUMBER
        | opcode::DIFFICULTY
        | opcode::GASLIMIT
        | opcode::CHAINID
        | opcode::BASEFEE
        | opcode::BLOBHASH
        | opcode::BLOBBASEFEE
        | opcode::GAS => {
            requirements.insert(FactKind::Input);
        }
        _ => {}
    }
    *may_write_state |= matches!(
        op,
        opcode::SSTORE | opcode::TSTORE | opcode::LOG0
            ..=opcode::LOG4
                | opcode::CALL
                | opcode::CALLCODE
                | opcode::DELEGATECALL
                | opcode::CREATE
                | opcode::CREATE2
                | opcode::SELFDESTRUCT
    );
}

// Positive list: adding an opcode elsewhere cannot accidentally admit an
// initial-state read, memory mutation, gas-dependent result or frame lifecycle.
fn stack_only(op: u8) -> bool {
    matches!(op,
        opcode::ADD | opcode::MUL | opcode::SUB | opcode::DIV | opcode::SDIV
        | opcode::MOD | opcode::SMOD | opcode::ADDMOD | opcode::MULMOD
        | opcode::EXP | opcode::SIGNEXTEND | opcode::LT | opcode::GT
        | opcode::SLT | opcode::SGT | opcode::EQ | opcode::ISZERO
        | opcode::AND | opcode::OR | opcode::XOR | opcode::NOT | opcode::BYTE
        | opcode::SHL | opcode::SHR | opcode::SAR | opcode::CLZ
        | opcode::POP | opcode::JUMP | opcode::JUMPI | opcode::JUMPDEST | opcode::PC
        | opcode::PUSH0..=opcode::PUSH32 | opcode::DUP1..=opcode::DUP16
        | opcode::SWAP1..=opcode::SWAP16)
}
