//! Intern complete immutable buffers, stores and code; references are report-local.
mod values;

use super::{checkpoint, mapping, value};
use alloy_primitives::{Address, B256, hex, keccak256};
use evm_abstract::analysis::control::Control;
use evm_abstract::{
    analysis::{FrameCode, MachinePayload},
    bytecode::Program,
    world::{ByteArray, Code, Existence, Store},
};
use evm_abstract_protocol as api;

pub(super) struct Pools<'a> {
    control: &'a Control,
    buffers: Vec<ByteArray>,
    stores_native: Vec<Store>,
    pub buffers_dto: Vec<api::ByteArraySnapshot>,
    pub stores: Vec<api::StoreSnapshot>,
    pub programs: Vec<api::ProgramInfo>,
}
pub(super) fn blocks(program: &Program) -> Vec<api::DisasmBlock> {
    program
        .blocks()
        .iter()
        .map(|block| api::DisasmBlock {
            id: block.id,
            start_pc: block.start_pc,
            instructions: block
                .instructions
                .iter()
                .map(mapping::instruction)
                .collect(),
        })
        .collect()
}
pub(super) fn mode(code: FrameCode) -> api::CodeKind {
    match code {
        FrameCode::Runtime => api::CodeKind::Runtime,
        FrameCode::InitCode => api::CodeKind::Initcode,
        FrameCode::Empty => api::CodeKind::Empty,
        FrameCode::Precompile(_) => api::CodeKind::Precompile,
        FrameCode::InvalidDelegation => api::CodeKind::InvalidDelegation,
    }
}
impl<'a> Pools<'a> {
    pub fn new(control: &'a Control) -> Self {
        Self {
            control,
            buffers: Vec::new(),
            stores_native: Vec::new(),
            buffers_dto: Vec::new(),
            stores: Vec::new(),
            programs: Vec::new(),
        }
    }
    pub fn program(
        &mut self,
        address: Address,
        hash: B256,
        kind: api::CodeKind,
        program: &Program,
    ) -> usize {
        let address = address.to_string();
        let hash = hash.to_string();
        if let Some(existing) = self.programs.iter().find(|item| {
            item.code_address == address && item.code_hash == hash && item.kind == kind
        }) {
            return existing.id;
        }
        let id = self.programs.len();
        self.programs.push(api::ProgramInfo {
            id,
            code_address: address,
            code_hash: hash,
            kind,
            bytecode: hex::encode_prefixed(program.bytes()),
            blocks: blocks(program),
        });
        id
    }
    pub fn buffer(&mut self, source: &ByteArray) -> Result<usize, api::ApiError> {
        checkpoint(self.control)?;
        if let Some(id) = self.buffers.iter().position(|item| item == source) {
            return Ok(id);
        }
        let id = self.buffers.len();
        self.buffers.push(source.clone());
        let mut values = values::Dictionary::default();
        let cells = source
            .stored_bytes()
            .map(|(offset, byte)| {
                checkpoint(self.control)?;
                Ok(api::ByteCell {
                    offset,
                    value: values.intern(value::info(byte)),
                })
            })
            .collect::<Result<_, api::ApiError>>()?;
        self.buffers_dto.push(api::ByteArraySnapshot {
            length: value::info(source.len()),
            default: value::info(source.default_byte()),
            cells,
            values: values.into_values(),
            memory: source.is_memory(),
        });
        Ok(id)
    }
    pub fn account(
        &mut self,
        source: &Store,
        address: Address,
    ) -> Result<api::AccountState, api::ApiError> {
        checkpoint(self.control)?;
        let code = source.code(address);
        let hash = if source.existence(address) == Existence::Absent {
            Some(B256::ZERO)
        } else {
            source.raw_account_code(address).map(keccak256)
        };
        let (kind, program, target) = match code {
            Some(Code::Runtime(program)) => (
                api::CodeKind::Runtime,
                Some(self.program(
                    address,
                    keccak256(program.bytes()),
                    api::CodeKind::Runtime,
                    program,
                )),
                None,
            ),
            Some(Code::Delegation(target)) => {
                (api::CodeKind::Delegation, None, Some(target.to_string()))
            }
            Some(Code::Empty) => (api::CodeKind::Empty, None, None),
            Some(Code::Unknown) | None => (api::CodeKind::Unknown, None, None),
        };
        Ok(api::AccountState {
            address: address.to_string(),
            existence: match source.existence(address) {
                Existence::Unknown => api::AccountExistence::Unknown,
                Existence::Present => api::AccountExistence::Present,
                Existence::Absent => api::AccountExistence::Absent,
            },
            balance: value::info(&source.read_balance(address)),
            nonce: value::info(&source.nonce(address)),
            code_kind: kind,
            code_hash: hash.map(|hash| hash.to_string()),
            program,
            delegation_target: target,
            storage_default: value::info(source.storage_default(address)),
            storage: source
                .slots()
                .iter()
                .filter(|((owner, _), _)| *owner == address)
                .map(|((_, slot), stored)| {
                    checkpoint(self.control)?;
                    Ok(api::StorageEntry {
                        slot: value::word(*slot),
                        value: value::info(stored),
                    })
                })
                .collect::<Result<_, api::ApiError>>()?,
            transient_default: value::info(source.transient_default(address)),
            transient: source
                .transient_slots()
                .iter()
                .filter(|((owner, _), _)| *owner == address)
                .map(|((_, slot), stored)| {
                    checkpoint(self.control)?;
                    Ok(api::StorageEntry {
                        slot: value::word(*slot),
                        value: value::info(stored),
                    })
                })
                .collect::<Result<_, api::ApiError>>()?,
            created: source.created_in_transaction(address),
            pending_destruction: source.pending_destruction(address),
        })
    }
    pub fn store(&mut self, source: &Store) -> Result<usize, api::ApiError> {
        checkpoint(self.control)?;
        if let Some(id) = self.stores_native.iter().position(|item| item == source) {
            return Ok(id);
        }
        let id = self.stores_native.len();
        self.stores_native.push(source.clone());
        let accounts = source
            .addresses()
            .into_iter()
            .map(|address| self.account(source, address))
            .collect::<Result<_, _>>()?;
        let logs = source
            .possible_logs()
            .iter()
            .map(|(key, log)| {
                Ok(api::LogSnapshot {
                    address: key.address.to_string(),
                    code_address: key.code_address.to_string(),
                    pc: key.pc,
                    topics: log.topics.iter().map(value::info).collect(),
                    data: self.buffer(&log.data)?,
                })
            })
            .collect::<Result<_, api::ApiError>>()?;
        self.stores.push(api::StoreSnapshot {
            storage_default: value::info(source.global_storage_default()),
            transient_default: value::info(source.global_transient_default()),
            balance_default: value::info(source.balance_default()),
            accounts,
            logs,
            logs_unknown: source.logs_unknown(),
        });
        Ok(id)
    }
    pub fn machine(
        &mut self,
        source: &MachinePayload,
    ) -> Result<api::MachineSnapshot, api::ApiError> {
        let store = self.store(&source.store)?;
        let frames = source
            .call_stack
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                checkpoint(self.control)?;
                let key = &frame.key;
                let program = frame.program.as_ref().map(|program| {
                    self.program(key.code_address, key.code_hash, mode(frame.code), program)
                });
                let continuation = index
                    .checked_sub(1)
                    .and_then(|child| source.call_stack.children().get(child))
                    .map(|child| {
                        let next = &child.continuation;
                        api::ContinuationSnapshot {
                            return_block: next.return_block,
                            output_offset: value::info(&next.output_offset),
                            output_size: value::info(&next.output_size),
                            creation: next.creation.map(|address| address.to_string()),
                        }
                    });
                Ok(api::FrameSnapshot {
                    index,
                    program,
                    code_address: key.code_address.to_string(),
                    code_hash: key.code_hash.to_string(),
                    storage_address: key.address.to_string(),
                    address_value: value::address(key.address_value),
                    caller: value::address(key.caller),
                    call_value: value::info(&frame.call_value),
                    is_static: key.is_static,
                    kind: mode(frame.code),
                    basic_block: key.basic_block_index,
                    context: key.jump_history.clone(),
                    stack: frame.stack.iter().map(value::info).collect(),
                    memory: self.buffer(&frame.memory)?,
                    calldata: self.buffer(&frame.calldata)?,
                    returndata: self.buffer(&frame.returndata)?,
                    rollback_store: self.store(frame.saved_store.state())?,
                    continuation,
                })
            })
            .collect::<Result<_, api::ApiError>>()?;
        Ok(api::MachineSnapshot { frames, store })
    }
}
