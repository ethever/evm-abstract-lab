//! Intern immutable snapshot versions of buffers, transaction stores and code.
//! References are report-local; transaction touch facts belong to each version.
mod budget;
mod values;

use super::{checkpoint, mapping, symbolic, value};
use alloy_primitives::{Address, B256, hex, keccak256};
use evm_abstract::analysis::control::Control;
use evm_abstract::{
    analysis::{EvmMachinePayload, FrameCode},
    bytecode::Program,
    domain::WordElement,
    world::{Code, EvmByteArray, EvmStore, Existence, LogSourceKind},
};
use evm_abstract_protocol as api;
use std::collections::BTreeMap;

pub(super) struct Pools<'a, N: WordElement> {
    /// Original input template only; public process Stores never recreate it.
    pub root_namespace: Option<Address>,
    control: &'a Control,
    buffers: Vec<EvmByteArray<N>>,
    buffer_roots: BTreeMap<evm_abstract::domain::symbolic::ByteStateId, Vec<usize>>,
    stores_native: Vec<EvmStore<N>>,
    pub buffers_dto: Vec<api::ByteArraySnapshot>,
    pub stores: Vec<api::StoreSnapshot>,
    pub programs: Vec<api::ProgramInfo>,
    pub symbolic: symbolic::Pool<'a>,
    pending_native: Vec<std::sync::Arc<EvmMachinePayload<N>>>,
    pub pending: Vec<api::MachineSnapshot>,
}

#[cfg(test)]
mod tests;
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
        FrameCode::Unknown => api::CodeKind::AbstractEnvironment,
    }
}
impl<'a, N: WordElement> Pools<'a, N> {
    pub fn new(control: &'a Control) -> Self {
        Self {
            control,
            buffers: Vec::new(),
            buffer_roots: BTreeMap::new(),
            stores_native: Vec::new(),
            buffers_dto: Vec::new(),
            stores: Vec::new(),
            programs: Vec::new(),
            symbolic: symbolic::Pool::new(control),
            pending_native: Vec::new(),
            pending: Vec::new(),
            root_namespace: None,
        }
    }
    pub fn program(
        &mut self,
        address: Address,
        hash: B256,
        kind: api::CodeKind,
        program: &Program,
    ) -> Result<usize, api::ApiError> {
        let source = (self.root_namespace != Some(address) || kind != api::CodeKind::Runtime)
            .then_some(address);
        self.program_identity(source, hash, kind, program)
    }
    fn program_identity(
        &mut self,
        address: Option<Address>,
        hash: B256,
        kind: api::CodeKind,
        program: &Program,
    ) -> Result<usize, api::ApiError> {
        self.symbolic.charge(1)?;
        let address = address.map(|address| address.to_string());
        let hash = hash.to_string();
        for id in 0..self.programs.len() {
            self.symbolic.charge(1)?;
            let item = &self.programs[id];
            if item.code_address == address && item.code_hash == hash && item.kind == kind {
                return Ok(item.id);
            }
        }
        self.symbolic.payload(program.bytes().len())?;
        self.symbolic.charge(
            program
                .blocks()
                .iter()
                .map(|block| block.instructions.len() + 1)
                .sum(),
        )?;
        let id = self.programs.len();
        self.programs.push(api::ProgramInfo {
            id,
            code_address: address,
            code_hash: hash,
            kind,
            bytecode: hex::encode_prefixed(program.bytes()),
            blocks: blocks(program),
        });
        Ok(id)
    }
    pub(super) fn process_program(
        &mut self,
        key: &evm_abstract::analysis::ProcessFrameKey,
        program: Option<&Program>,
    ) -> Result<Option<usize>, api::ApiError> {
        let Some(program) = program else {
            return Ok(None);
        };
        match key.source {
            evm_abstract::analysis::ProcessSource::InputProgram { hash, .. } => self
                .program_identity(None, hash, api::CodeKind::Runtime, program)
                .map(Some),
            evm_abstract::analysis::ProcessSource::Runtime { source, hash, .. } => self
                .program_identity(Some(source), hash, api::CodeKind::Runtime, program)
                .map(Some),
            evm_abstract::analysis::ProcessSource::InitCode { hash, .. } => self
                .program_identity(None, hash, api::CodeKind::Initcode, program)
                .map(Some),
            _ => Ok(None),
        }
    }
    pub fn buffer(&mut self, source: &EvmByteArray<N>) -> Result<usize, api::ApiError> {
        self.symbolic.charge(1)?;
        let work = budget::BufferWork::new(source, &mut self.symbolic)?;
        if let Some(candidates) = self.buffer_roots.get(source.state_root()) {
            for &id in candidates {
                self.symbolic.charge(1)?;
                if !work.comparison(source, &self.buffers[id], &mut self.symbolic)? {
                    continue;
                }
                let equal = self.buffers[id] == *source;
                // A cancellation arriving during a metered native comparison
                // must not become a successful cache hit.
                self.symbolic.charge(0)?;
                if equal {
                    return Ok(id);
                }
            }
        }
        let id = self.buffers.len();
        work.copy(&mut self.symbolic)?;
        self.buffers.push(source.clone());
        self.symbolic.charge(1)?;
        self.buffer_roots
            .entry(source.state_root().clone())
            .or_default()
            .push(id);
        let mut values = values::Dictionary::default();
        let cells = source
            .stored_bytes()
            .map(|(offset, byte)| {
                checkpoint(self.control)?;
                Ok(api::ByteCell {
                    offset,
                    value: {
                        let info = self.symbolic.value(byte)?;
                        values.intern_checked(info, &mut self.symbolic)?
                    },
                })
            })
            .collect::<Result<_, api::ApiError>>()?;
        self.buffers_dto.push(api::ByteArraySnapshot {
            length: self.symbolic.value(source.len())?,
            default: self.symbolic.value(source.default_byte())?,
            cells,
            values: values.into_values(),
            memory: source.is_memory(),
            symbolic: self.symbolic.view(&source.view())?,
            symbolic_facts: source
                .symbolic_facts()
                .map(|(expression, value)| {
                    Ok(api::SymbolicFact {
                        expression: self.symbolic.expression(expression)?,
                        value: self.symbolic.value(value)?,
                    })
                })
                .collect::<Result<_, api::ApiError>>()?,
        });
        Ok(id)
    }
    pub fn account(
        &mut self,
        source: &EvmStore<N>,
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
                )?),
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
            balance: self.symbolic.value(&source.read_balance(address))?,
            nonce: self.symbolic.value(&source.nonce(address))?,
            code_kind: kind,
            code_hash: hash.map(|hash| hash.to_string()),
            program,
            delegation_target: target,
            storage_default: self.symbolic.value(source.storage_default(address))?,
            storage_root: self.slot_root(source, address, false)?,
            transient_root: self.slot_root(source, address, true)?,
            storage: source
                .slots()
                .iter()
                .filter(|((owner, _), _)| *owner == address)
                .map(|((_, slot), stored)| {
                    checkpoint(self.control)?;
                    Ok(api::StorageEntry {
                        slot: value::word(*slot),
                        value: self.symbolic.value(stored)?,
                    })
                })
                .collect::<Result<_, api::ApiError>>()?,
            transient_default: self.symbolic.value(source.transient_default(address))?,
            transient: source
                .transient_slots()
                .iter()
                .filter(|((owner, _), _)| *owner == address)
                .map(|((_, slot), stored)| {
                    checkpoint(self.control)?;
                    Ok(api::StorageEntry {
                        slot: value::word(*slot),
                        value: self.symbolic.value(stored)?,
                    })
                })
                .collect::<Result<_, api::ApiError>>()?,
            created: source.created_in_transaction(address),
            touched: source.touched_in_transaction(address),
            pending_destruction: source.pending_destruction(address),
        })
    }
    pub fn store(&mut self, source: &EvmStore<N>) -> Result<usize, api::ApiError> {
        checkpoint(self.control)?;
        for id in 0..self.stores_native.len() {
            self.symbolic.charge(1)?;
            self.symbolic.charge(source.work_size())?;
            // Equal data roots may carry different touch facts and therefore
            // different EIP-161 completion behavior. Compare the whole store.
            let equal = self.stores_native[id] == *source;
            self.symbolic.charge(0)?;
            if equal {
                return Ok(id);
            }
        }
        let id = self.stores_native.len();
        self.symbolic.charge(source.work_size())?;
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
                    code_hash: key.code_hash.to_string(),
                    source_kind: match key.source_kind {
                        LogSourceKind::Init => api::LogSourceKind::Init,
                        LogSourceKind::Runtime => api::LogSourceKind::Runtime,
                    },
                    pc: key.pc,
                    topics: log
                        .topics
                        .iter()
                        .map(|v| self.symbolic.value(v))
                        .collect::<Result<_, _>>()?,
                    data: self.buffer(&log.data)?,
                })
            })
            .collect::<Result<_, api::ApiError>>()?;
        self.stores.push(api::StoreSnapshot {
            storage_default: self.symbolic.value(source.global_storage_default())?,
            transient_default: self.symbolic.value(source.global_transient_default())?,
            storage_root: self.symbolic.slots(source.global_storage_state())?,
            transient_root: self.symbolic.slots(source.global_transient_state())?,
            balance_default: self.symbolic.value(source.balance_default())?,
            nonce_default: self.symbolic.value(&source.nonce_default())?,
            code_default: source
                .code_default()
                .map(|code| {
                    Ok::<_, api::ApiError>(match code {
                        Code::Runtime(program) => {
                            self.symbolic.payload(program.bytes().len())?;
                            api::AccountCode::Runtime(hex::encode_prefixed(program.bytes()))
                        }
                        Code::Delegation(target) => {
                            api::AccountCode::Delegation(target.to_string())
                        }
                        Code::Empty => api::AccountCode::Empty,
                        Code::Unknown => api::AccountCode::Unknown,
                    })
                })
                .transpose()?,
            existence_default: match source.existence_default() {
                Existence::Unknown => api::AccountExistence::Unknown,
                Existence::Present => api::AccountExistence::Present,
                Existence::Absent => api::AccountExistence::Absent,
            },
            created_default: source.created_default(),
            pending_destruction_default: source.pending_destruction_default(),
            touched_default: source.touched_default(),
            accounts,
            logs,
            logs_unknown: source.logs_unknown(),
            storage_facts: source
                .symbolic_facts(false)
                .map(|(expression, value)| {
                    Ok(api::SymbolicFact {
                        expression: self.symbolic.expression(expression)?,
                        value: self.symbolic.value(value)?,
                    })
                })
                .collect::<Result<_, api::ApiError>>()?,
            transient_facts: source
                .symbolic_facts(true)
                .map(|(expression, value)| {
                    Ok(api::SymbolicFact {
                        expression: self.symbolic.expression(expression)?,
                        value: self.symbolic.value(value)?,
                    })
                })
                .collect::<Result<_, api::ApiError>>()?,
        });
        Ok(id)
    }
    pub fn machine(
        &mut self,
        source: &EvmMachinePayload<N>,
    ) -> Result<api::MachineSnapshot, api::ApiError> {
        let store = self.store(&source.store)?;
        let frames = source
            .call_stack
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                checkpoint(self.control)?;
                let key = &frame.key;
                if let Some(process_key) = key.process.as_deref() {
                    self.symbolic.charge(48)?;
                    let address = frame.address_operand.as_ref().ok_or_else(|| {
                        super::process::invalid(api::ProcessValidationKind::Operands)
                    })?;
                    let caller = frame.caller_operand.as_ref().ok_or_else(|| {
                        super::process::invalid(api::ProcessValidationKind::Operands)
                    })?;
                    let process_operands = super::process::operands(
                        address,
                        caller,
                        &frame.call_value,
                        &mut self.symbolic,
                    )?;
                    let process_creation = index
                        .checked_sub(1)
                        .and_then(|child| source.call_stack.children().get(child))
                        .and_then(|child| child.continuation.creation)
                        .map(super::process::continuation);
                    return Ok(api::FrameSnapshot {
                        process: Some(super::process::key(process_key)),
                        process_operands: Some(process_operands),
                        process_creation,
                        index,
                        program: self.process_program(process_key, frame.program.as_ref())?,
                        code_address: None,
                        code_hash: None,
                        storage_address: None,
                        address_value: None,
                        caller: None,
                        address_operand: None,
                        caller_operand: None,
                        call_value: self.symbolic.value(&frame.call_value)?,
                        is_static: process_key.is_static,
                        kind: super::process::key(process_key).source.kind(),
                        basic_block: None,
                        context: vec![],
                        stack: vec![],
                        memory: None,
                        calldata: None,
                        returndata: None,
                        rollback_store: self.store(frame.saved_store.state())?,
                        continuation: None,
                    });
                }
                self.symbolic.charge(
                    key.jump_history
                        .len()
                        .saturating_add(frame.stack.len())
                        .saturating_add(1),
                )?;
                let program = frame
                    .program
                    .as_ref()
                    .map(|program| {
                        self.program(key.code_address, key.code_hash, mode(frame.code), program)
                    })
                    .transpose()?;
                let continuation = index
                    .checked_sub(1)
                    .and_then(|child| source.call_stack.children().get(child))
                    .map(|child| {
                        let next = &child.continuation;
                        Ok(api::ContinuationSnapshot {
                            return_block: next.return_block,
                            output_offset: self.symbolic.value(&next.output_offset)?,
                            output_size: self.symbolic.value(&next.output_size)?,
                            creation: next.creation.map(super::process::continuation),
                        })
                    })
                    .transpose()?;
                Ok(api::FrameSnapshot {
                    process: None,
                    process_operands: None,
                    process_creation: None,
                    index,
                    program,
                    code_address: Some(key.code_address.to_string()),
                    code_hash: Some(key.code_hash.to_string()),
                    storage_address: Some(key.address.to_string()),
                    address_value: Some(value::address(key.address_value)),
                    caller: Some(value::address(key.caller)),
                    address_operand: frame
                        .address_operand
                        .as_ref()
                        .map(|value| self.symbolic.value(value))
                        .transpose()?,
                    caller_operand: frame
                        .caller_operand
                        .as_ref()
                        .map(|value| self.symbolic.value(value))
                        .transpose()?,
                    call_value: self.symbolic.value(&frame.call_value)?,
                    is_static: key.is_static,
                    kind: mode(frame.code),
                    basic_block: Some(key.basic_block_index),
                    context: key.jump_history.clone(),
                    stack: frame
                        .stack
                        .iter()
                        .map(|v| self.symbolic.value(v))
                        .collect::<Result<_, _>>()?,
                    memory: Some(self.buffer(&frame.memory)?),
                    calldata: Some(self.buffer(&frame.calldata)?),
                    returndata: Some(self.buffer(&frame.returndata)?),
                    rollback_store: self.store(frame.saved_store.state())?,
                    continuation,
                })
            })
            .collect::<Result<_, api::ApiError>>()?;
        Ok(api::MachineSnapshot {
            frames,
            store,
            path: self.symbolic.relations(&source.relations)?,
        })
    }
    fn slot_root(
        &mut self,
        store: &EvmStore<N>,
        address: Address,
        transient: bool,
    ) -> Result<api::SlotStateRef, api::ApiError> {
        let root = store.slot_root(address, transient).map_err(|error| {
            if self.control.cancellation().is_cancelled() {
                return self.symbolic.failure(api::ExportFailureKind::Cancelled, 0);
            }
            let mut report = self.symbolic.failure(api::ExportFailureKind::StateRoot, 0);
            report.message = format!("storage root export failed: {error}");
            report
        })?;
        self.symbolic.slots(&root)
    }
    pub fn pending(
        &mut self,
        source: &std::sync::Arc<EvmMachinePayload<N>>,
    ) -> Result<usize, api::ApiError> {
        for id in 0..self.pending_native.len() {
            self.symbolic.charge(1)?;
            if std::sync::Arc::ptr_eq(&self.pending_native[id], source) {
                return Ok(id);
            }
        }
        self.symbolic.charge(1)?;
        let snapshot = self.machine(source)?;
        let id = self.pending.len();
        self.pending_native.push(std::sync::Arc::clone(source));
        self.pending.push(snapshot);
        Ok(id)
    }
    pub fn range(
        &mut self,
        witness: &evm_abstract::analysis::EvmRangeWitness<N>,
    ) -> Result<api::RangeWitness, api::ApiError> {
        let request = &witness.request;
        Ok(api::RangeWitness {
            request: api::RangeRequest {
                kind: match request.kind {
                    evm_abstract::analysis::RangeKind::MemoryPolicy => api::RangeKind::MemoryPolicy,
                    evm_abstract::analysis::RangeKind::LogicalMemory => {
                        api::RangeKind::LogicalMemory
                    }
                    evm_abstract::analysis::RangeKind::ReturnData => api::RangeKind::ReturnData,
                },
                offset: self.symbolic.value(&request.offset)?,
                size: self.symbolic.value(&request.size)?,
                limit: self.symbolic.value(&request.limit)?,
            },
            condition: self.symbolic.value(&witness.condition)?,
            nonzero: witness.nonzero,
            payload: self.pending(&witness.payload)?,
        })
    }
}
