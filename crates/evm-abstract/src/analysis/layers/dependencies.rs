//! Conservative execution reads, including omitted/negative initial facts.
//!
//! The index certifies invalidation, not permission to transplant old payloads.
//! Pure block reuse has a separate, stricter stack-delta contract. Calls and
//! terminals include whole-state dependencies: summaries, unknown callbacks,
//! account lifecycle, and rollback expose state beyond an instruction's stack.

use super::{
    FactKey,
    catalog::{CodeCatalog, CodeMode, FactKind, classify},
};
use crate::{
    Address,
    analysis::{FrameCode, MachineEdge, MachinePayload},
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A possible initial read. `None` is a wildcard, never a default zero/address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum FactDependency {
    /// Account code resolution, including unknown, empty and delegated code.
    Code(Option<Address>),
    /// Initial account balance.
    Balance(Option<Address>),
    /// Initial account nonce.
    Nonce(Option<Address>),
    /// Presence, including a negative observation.
    Existence(Option<Address>),
    /// Persistent storage; a wildcard key includes completeness/default reads.
    Storage {
        /// State owner, independently of the code address.
        address: Option<Address>,
        /// Exact key when available; otherwise every key of the selected owner.
        slot: Option<crate::U256>,
    },
    /// Immutable transaction/block inputs. This session cannot mutate them.
    Inputs,
    /// Conservative closure for interactions, summaries and exposed Store roots.
    AllInitialState,
}

impl FactDependency {
    /// Semantic intersection includes consequences of presence and completeness.
    pub fn affected_by(self, key: FactKey) -> bool {
        let at = |address: Option<Address>| address.is_none_or(|a| a == key.address());
        match self {
            Self::AllInitialState => true,
            Self::Inputs => false,
            Self::Code(address) => {
                at(address) && matches!(key, FactKey::Code(_) | FactKey::Existence(_))
            }
            Self::Balance(address) => {
                at(address) && matches!(key, FactKey::Balance(_) | FactKey::Existence(_))
            }
            Self::Nonce(address) => {
                at(address) && matches!(key, FactKey::Nonce(_) | FactKey::Existence(_))
            }
            Self::Existence(address) => at(address) && matches!(key, FactKey::Existence(_)),
            Self::Storage { address, slot } => {
                at(address)
                    && match key {
                        FactKey::Storage { slot: changed, .. } => slot.is_none_or(|s| s == changed),
                        FactKey::StorageComplete(_) | FactKey::Existence(_) => true,
                        _ => false,
                    }
            }
        }
    }
}

/// Reusable read/invalidation index for one immutable execution graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FactDependencies {
    reads: BTreeMap<FactDependency, BTreeSet<usize>>,
    successors: BTreeMap<usize, BTreeSet<usize>>,
}

impl FactDependencies {
    /// Recorded positive, negative and wildcard reads with their graph states.
    pub fn reads(&self) -> impl Iterator<Item = (&FactDependency, &BTreeSet<usize>)> {
        self.reads.iter()
    }

    /// Direct readers and every reachable dependent, including call/return edges.
    /// This does not authorize reusing an unaffected state's Store or checkpoint.
    pub fn affected_states(&self, added: &[FactKey]) -> BTreeSet<usize> {
        let mut affected: BTreeSet<_> = self
            .reads
            .iter()
            .filter(|(read, _)| added.iter().any(|key| read.affected_by(*key)))
            .flat_map(|(_, states)| states.iter().copied())
            .collect();
        let mut queue: VecDeque<_> = affected.iter().copied().collect();
        while let Some(state) = queue.pop_front() {
            if let Some(next) = self.successors.get(&state) {
                for target in next {
                    if affected.insert(*target) {
                        queue.push_back(*target);
                    }
                }
            }
        }
        affected
    }

    pub(crate) fn add(&mut self, state: usize, read: FactDependency) {
        self.reads.entry(read).or_default().insert(state);
    }

    pub(crate) fn finish(&mut self, edges: &[MachineEdge]) {
        for edge in edges {
            self.successors
                .entry(edge.from)
                .or_default()
                .insert(edge.to);
        }
    }

    pub(crate) fn record(&mut self, state: usize, payload: &MachinePayload, catalog: &CodeCatalog) {
        let frame = payload.active();
        self.add(state, FactDependency::Code(Some(frame.key.code_address)));
        if frame.code != FrameCode::Runtime && frame.code != FrameCode::InitCode {
            self.add(state, FactDependency::AllInitialState);
            return;
        }
        let mode = if frame.code == FrameCode::InitCode {
            CodeMode::Initcode
        } else {
            CodeMode::Runtime
        };
        let block = frame
            .program
            .as_ref()
            .and_then(|p| p.blocks().get(frame.key.basic_block_index));
        let Some(block) = block else {
            self.add(state, FactDependency::AllInitialState);
            return;
        };
        let compiled = catalog
            .source(frame.key.code_hash, mode)
            .filter(|compiled| frame.program.as_ref() == Some(compiled.program()))
            .and_then(|code| code.blocks().get(frame.key.basic_block_index));
        let requirements = compiled.map_or_else(
            || {
                let mut requirements = BTreeSet::new();
                let mut writes = false;
                for i in &block.instructions {
                    classify(i.opcode, &mut requirements, &mut writes);
                }
                requirements
            },
            |compiled| compiled.requirements.clone(),
        );
        let owner = frame.address_operand.as_ref().map_or_else(
            || frame.key.address_value.as_concrete(),
            |value| {
                value
                    .singleton()
                    .map(|word| Address::from_slice(&word.to_be_bytes::<32>()[12..]))
            },
        );
        for requirement in requirements {
            let dependency = match requirement {
                FactKind::Storage => FactDependency::Storage {
                    address: owner,
                    slot: None,
                },
                FactKind::Input => FactDependency::Inputs,
                FactKind::Code => FactDependency::Code(None),
                FactKind::Existence => FactDependency::Existence(None),
                FactKind::Balance => FactDependency::Balance(None),
                FactKind::Nonce => FactDependency::Nonce(None),
                FactKind::ExternalInteraction => FactDependency::AllInitialState,
            };
            self.add(state, dependency);
        }
        if block.instructions.last().is_some_and(|i| {
            matches!(
                i.opcode,
                revm_bytecode::opcode::STOP
                    | revm_bytecode::opcode::RETURN
                    | revm_bytecode::opcode::REVERT
                    | revm_bytecode::opcode::SELFDESTRUCT
                    | revm_bytecode::opcode::INVALID
            )
        }) {
            self.add(state, FactDependency::AllInitialState);
        }
    }
}
