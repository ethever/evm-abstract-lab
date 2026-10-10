//! 教学视图共享身份索引。短名字总有完整图例，不能把代码地址与状态账户混为一谈。
use evm_abstract_notation::Symbol;

use crate::analysis::{FrameCode, FrameKey, WorldAnalysis};
use crate::world::AddressInput;
use alloy_primitives::{Address, B256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) type CodeIdentity = (Address, B256, FrameCode);

pub(super) struct References {
    addresses: BTreeMap<Address, String>,
    codes: BTreeMap<CodeIdentity, String>,
    owner_alias: Option<(Address, String)>,
}

impl References {
    pub(super) fn new(analysis: &WorldAnalysis) -> Self {
        let mut addresses = BTreeSet::from([analysis.entry().address]);
        let mut concrete_inputs = BTreeSet::new();
        super::environment::collect_addresses(&analysis.entry().environment, &mut concrete_inputs);
        addresses.extend(&concrete_inputs);
        addresses.extend(analysis.world().accounts().keys().copied());
        let mut codes = BTreeSet::new();
        for state in analysis.states() {
            for payload in std::iter::once(&state.entry).chain(state.exit.iter()) {
                for frame in payload.call_stack.iter() {
                    addresses.extend([frame.key.address, frame.key.code_address]);
                    addresses.extend(
                        [frame.key.address_value, frame.key.caller]
                            .into_iter()
                            .filter_map(|input| input.as_concrete()),
                    );
                    concrete_inputs.extend(
                        [frame.key.address_value, frame.key.caller]
                            .into_iter()
                            .filter_map(|input| input.as_concrete()),
                    );
                    codes.insert((frame.key.code_address, frame.key.code_hash, frame.code));
                }
            }
        }
        for frontier in analysis.frontiers() {
            if let Some(target) = &frontier.target {
                for frame in &target.frames {
                    addresses.extend([frame.address, frame.code_address]);
                    addresses.extend(
                        [frame.address_value, frame.caller]
                            .into_iter()
                            .filter_map(|input| input.as_concrete()),
                    );
                    concrete_inputs.extend(
                        [frame.address_value, frame.caller]
                            .into_iter()
                            .filter_map(|input| input.as_concrete()),
                    );
                }
            }
        }
        let owner_alias = super::environment::owner_alias(analysis);
        if let Some((internal, _)) = &owner_alias
            && !concrete_inputs.contains(internal)
        {
            addresses.remove(internal);
        }
        Self {
            addresses: addresses
                .into_iter()
                .enumerate()
                .map(|(i, address)| (address, format!("{}", Symbol::Account(i))))
                .collect(),
            codes: codes
                .into_iter()
                .enumerate()
                .map(|(i, code)| (code, format!("{}", Symbol::Code(i))))
                .collect(),
            owner_alias,
        }
    }

    pub(super) fn address(&self, address: Address) -> Option<&str> {
        if let Some((internal, label)) = &self.owner_alias
            && *internal == address
        {
            return Some(label);
        }
        self.addresses.get(&address).map(String::as_str)
    }

    pub(super) fn address_input(&self, input: AddressInput) -> String {
        input
            .as_concrete()
            .and_then(|address| self.addresses.get(&address))
            .cloned()
            .unwrap_or_else(|| input.to_string())
    }
    pub(super) fn code(&self, key: &FrameKey) -> Option<&str> {
        self.codes
            .get(&(key.code_address, key.code_hash, key.mode))
            .map(String::as_str)
    }
    pub(super) fn codes(&self) -> impl Iterator<Item = (&CodeIdentity, &str)> {
        self.codes.iter().map(|(key, id)| (key, id.as_str()))
    }
    pub(super) fn addresses(&self) -> impl Iterator<Item = (Address, &str)> {
        self.addresses.iter().map(|(key, id)| (*key, id.as_str()))
    }
}
