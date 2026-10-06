//! 教学视图共享身份索引。短名字总有完整图例，不能把代码地址与状态账户混为一谈。
use crate::analysis::{FrameCode, FrameKey, WorldAnalysis};
use alloy_primitives::{Address, B256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) type CodeIdentity = (Address, B256, FrameCode);

pub(super) struct References {
    addresses: BTreeMap<Address, String>,
    codes: BTreeMap<CodeIdentity, String>,
}

impl References {
    pub(super) fn new(analysis: &WorldAnalysis) -> Self {
        let mut addresses = BTreeSet::from([analysis.entry().address, analysis.entry().caller]);
        addresses.extend(analysis.world().accounts().keys().copied());
        let mut codes = BTreeSet::new();
        for state in analysis.states() {
            for payload in std::iter::once(&state.entry).chain(state.exit.iter()) {
                for frame in payload.call_stack.iter() {
                    addresses.extend([frame.key.address, frame.key.code_address, frame.key.caller]);
                    codes.insert((frame.key.code_address, frame.key.code_hash, frame.code));
                }
            }
        }
        for frontier in analysis.frontiers() {
            if let Some(target) = &frontier.target {
                for frame in &target.frames {
                    addresses.extend([frame.address, frame.code_address, frame.caller]);
                }
            }
        }
        Self {
            addresses: addresses
                .into_iter()
                .enumerate()
                .map(|(i, address)| (address, format!("A{i}")))
                .collect(),
            codes: codes
                .into_iter()
                .enumerate()
                .map(|(i, code)| (code, format!("C{i}")))
                .collect(),
        }
    }

    pub(super) fn address(&self, address: Address) -> Option<&str> {
        self.addresses.get(&address).map(String::as_str)
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
