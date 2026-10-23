//! Process serialization never exposes an internal adapter anchor as ADDRESS.

use super::{Address, AddressInput, B256, FrameKey, Serialize, Serializer};
use crate::analysis::{ProcessFrameKey, ProcessOwner, ProcessSource};
use serde::ser::SerializeMap;

impl FrameKey {
    pub(crate) fn process_work(&self) -> usize {
        self.process.as_ref().map_or(0, |_| {
            std::mem::size_of::<ProcessFrameKey>()
                .div_ceil(std::mem::size_of::<usize>())
                .saturating_mul(2)
                .saturating_add(8)
        })
    }

    /// Logical operands must already be materialized before canonicalizing.
    /// The residual anchor is never consulted by process effects or execution.
    pub(in crate::analysis) fn install_process(&mut self, process: ProcessFrameKey) {
        self.address = match process.owner {
            ProcessOwner::Named(owner) => owner,
            ProcessOwner::Residual => Address::ZERO,
        };
        (self.code_address, self.code_hash) = match process.source {
            ProcessSource::Runtime { source, hash, .. }
            | ProcessSource::InvalidDelegation { source, hash, .. } => (source, hash),
            ProcessSource::InitCode { hash, .. } => (Address::ZERO, hash),
            ProcessSource::Precompile { address, .. } => (address, B256::ZERO),
            _ => (Address::ZERO, B256::ZERO),
        };
        self.is_static = process.is_static;
        self.address_value = AddressInput::unknown_to();
        self.caller = AddressInput::unknown_caller();
        self.basic_block_index = 0;
        self.stack_height = 0;
        self.jump_history.clear();
        self.process = Some(Box::new(process));
    }
}

impl Serialize for FrameKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(process) = &self.process {
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry("process", process)?;
            return map.end();
        }
        let mut map = serializer.serialize_map(Some(10))?;
        map.serialize_entry("code_address", &self.code_address)?;
        map.serialize_entry("code_hash", &self.code_hash)?;
        map.serialize_entry("mode", &self.mode)?;
        map.serialize_entry("address", &self.address)?;
        map.serialize_entry("address_value", &self.address_value)?;
        map.serialize_entry("caller", &self.caller)?;
        map.serialize_entry("is_static", &self.is_static)?;
        map.serialize_entry("basic_block_index", &self.basic_block_index)?;
        map.serialize_entry("stack_height", &self.stack_height)?;
        map.serialize_entry("jump_history", &self.jump_history)?;
        map.end()
    }
}
