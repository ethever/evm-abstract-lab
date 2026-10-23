//! Frozen source/owner preparation before native kernel key selection.

use super::super::{Composition, EvmPayloadSummary, EvmSolver, FrameCode, MachineError};
use crate::{
    analysis::{
        EvmMachinePayload, ProcessBoundary, ProcessFrameKey, ProcessSource,
        engine::{ContextPolicy, PartitionPolicy, StatePolicy},
    },
    domain::EvmNumericAlgebra,
    world::{
        Code,
        process::{CapturedProcessCode, OwnerCatalogue, OwnerCatalogueLimits, ProcessOwnerError},
    },
};
use alloy_primitives::{Address, U256, keccak256};
use std::{collections::BTreeSet, sync::Arc};

pub(super) fn owner_error(error: ProcessOwnerError) -> MachineError {
    match error {
        ProcessOwnerError::Work => MachineError::Work,
        ProcessOwnerError::Cancelled => crate::resource::CloneError::Cancelled.into(),
        ProcessOwnerError::Store(error) => MachineError::Store(error),
        _ => MachineError::Process(ProcessBoundary::Catalogue),
    }
}

impl<D, C, P, S, X, E> EvmSolver<'_, D, C, P, S, X, E>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
{
    pub(in crate::analysis::transfer::procedure) fn freeze_process_catalogue(
        &mut self,
        input: &EvmMachinePayload<D::Element>,
    ) -> Result<(), MachineError> {
        if self.table.process_catalogue.is_some() {
            return Ok(());
        }
        self.reserve(
            input
                .store
                .clone_prefix_work()
                .saturating_add(self.world.accounts().len())
                .saturating_add(input.call_stack.depth())
                .saturating_add(2)
                .saturating_mul((usize::BITS as usize + 1).saturating_mul(128)),
        )?;
        let mut names = input.store.addresses();
        names.extend(self.world.accounts().keys().copied());
        names.insert(self.entry.address);
        names.insert(Address::from_word(U256::from(3).into()));
        for frame in input.call_stack.iter() {
            names.insert(frame.key.address);
            names.insert(frame.key.code_address);
        }
        let mut codes = BTreeSet::new();
        for source in &names {
            if let Some(Code::Runtime(program)) = input.store.code(*source) {
                self.reserve(program.byte_len().saturating_mul(4).saturating_add(64))?;
                codes.insert(CapturedProcessCode {
                    source: *source,
                    hash: keccak256(program.bytes()),
                    fork: program.fork(),
                });
            }
        }
        // A root may already be constructor code rather than account runtime.
        // Its fixed input Program is retained by the frame, not added as a
        // future dynamically discoverable runtime body.
        let names: Vec<_> = names.into_iter().collect();
        let codes: Vec<_> = codes.into_iter().collect();
        let catalogue = OwnerCatalogue::freeze(
            &names,
            &codes,
            OwnerCatalogueLimits {
                names: names.len(),
                codes: codes.len(),
            },
            self.budget,
        )
        .map_err(owner_error)?;
        let root_address = match self.entry.environment.to {
            crate::world::AddressInput::Concrete(address) => crate::domain::summary::constant(
                U256::from_be_slice(address.as_slice()),
                self.domain,
                self.budget,
            )?,
            crate::world::AddressInput::Symbolic(_) => crate::domain::summary::unsigned_range(
                U256::ZERO,
                U256::MAX >> 96usize,
                self.domain,
                self.budget,
            )?,
        };
        self.reserve(16)?;
        self.table.process_root_address = Some(Arc::new(root_address));
        self.table.process_catalogue = Some(catalogue);
        Ok(())
    }

    pub(in crate::analysis::transfer::procedure) fn prepare_process_summary(
        &mut self,
        mut input: EvmPayloadSummary<D::Element>,
    ) -> Result<EvmPayloadSummary<D::Element>, MachineError> {
        let catalogue = Arc::clone(
            self.table
                .process_catalogue
                .as_ref()
                .ok_or(MachineError::Shape)?,
        );
        self.reserve(input.frames.len().saturating_mul(256).saturating_add(8))?;
        for frame in &mut input.frames {
            let process = if let Some(existing) = &frame.key.process {
                if existing.epoch != catalogue.epoch() {
                    return Err(MachineError::Process(ProcessBoundary::Catalogue));
                }
                (**existing).clone()
            } else {
                let fork = self.world.fork();
                let source = match frame.code {
                    FrameCode::Runtime => {
                        let program = frame.program.as_ref().ok_or(MachineError::Shape)?;
                        let captured = CapturedProcessCode {
                            source: frame.key.code_address,
                            hash: frame.key.code_hash,
                            fork: program.fork(),
                        };
                        catalogue
                            .admit_code(captured, self.budget)
                            .map_err(owner_error)?;
                        ProcessSource::Runtime {
                            source: captured.source,
                            hash: captured.hash,
                            fork: captured.fork,
                        }
                    }
                    FrameCode::InitCode => match &frame.program {
                        Some(program) => ProcessSource::InitCode {
                            hash: frame.key.code_hash,
                            fork: program.fork(),
                        },
                        None => ProcessSource::AnyInitCode { fork },
                    },
                    FrameCode::Unknown => ProcessSource::AnyRuntime { fork },
                    FrameCode::Empty => ProcessSource::Empty { fork },
                    FrameCode::Precompile(address) => ProcessSource::Precompile { address, fork },
                    FrameCode::InvalidDelegation => ProcessSource::InvalidDelegation {
                        source: frame.key.code_address,
                        hash: frame.key.code_hash,
                        fork,
                    },
                };
                ProcessFrameKey {
                    epoch: catalogue.epoch(),
                    owner: catalogue.classify(frame.key.address),
                    source,
                    is_static: frame.key.is_static,
                }
            };
            if matches!(process.source, ProcessSource::InitCode { .. }) {
                // This prepared vocabulary freezes runtime sources. Coarse
                // CREATE uses AnyInitCode and remains covering; promoting an
                // independently captured constructor must first extend the
                // source-kind catalogue under a new epoch. Never grow hash
                // keys silently through this private admission seam.
                return Err(MachineError::Process(ProcessBoundary::Catalogue));
            }
            if let ProcessSource::Runtime { source, hash, fork } = process.source {
                catalogue
                    .admit_code(CapturedProcessCode { source, hash, fork }, self.budget)
                    .map_err(owner_error)?;
            }
            frame.key.install_process(process);
        }
        // Current state is prepared; each saved caller checkpoint is separate
        // and retains its original native snapshot and genuine sticky touch.
        input.store = input
            .store
            .process_complement(&catalogue, self.domain, self.budget)?;
        input.location.frames = input.frames.iter().map(|frame| frame.key.clone()).collect();
        // Code/lifecycle bounds are joined inputs of a process. Its source is
        // already in the frame key; they must not create evolving whole-store
        // identities or new owner keys after constructor/callback effects.
        input.location.code_identity = catalogue.epoch();
        input.storage_observations = Default::default();
        input.work = input.measure_work();
        self.reserve(0)?;
        Ok(input)
    }
}
