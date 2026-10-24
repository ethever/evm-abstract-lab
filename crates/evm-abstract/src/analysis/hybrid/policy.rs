//! Frozen code-region context allocation at the existing native admission seam.
//!
//! Erasing optional history changes the actual prepared payload before the
//! delegated key is chosen. Source, owner, staticness, values, rollback roots and
//! continuations are preserved; the existing covering join owns every merge.

use super::HybridRegion;
use crate::{
    analysis::{
        Acceleration, AccelerationInput, AccelerationPolicy, EvmMachinePayload, EvmWorldAnalysis,
        FrameKey, MachineError, MachinePartition, PartitionRefusal, PartitionResult, Partitioned,
        PartitionedRelative, PolicyContext, PolicySite, ProcedureOutput, ProcedureSummaryPolicy,
        ProcessSource, ProjectionError, RelativePartitionResult, RelativeState, RuntimeExecution,
        WholeStatePolicy,
    },
    domain::{EvmNumericAlgebra, WordElement, symbolic::ExprId},
    resource::{WorkBudget, clone_payment::pay},
    world::{Code, EvmEntry, RootInputBinding},
};
use std::collections::BTreeSet;

pub(super) fn selected(regions: &[HybridRegion], frame: &FrameKey) -> bool {
    frame.mode == crate::analysis::FrameCode::Runtime
        && frame.process.is_none()
        && regions.iter().any(|region| {
            region.code_address == frame.code_address && region.code_hash == frame.code_hash
        })
}

pub(super) fn select<N: WordElement>(
    original: &EvmWorldAnalysis<N>,
    entry: &EvmEntry<N>,
    limit: usize,
    budget: &mut WorkBudget,
) -> Result<Vec<HybridRegion>, ProjectionError> {
    let world = original.world();
    let declared = original.declared_root_source(budget)?;
    let namespace = match original.root_binding() {
        RootInputBinding::Physical { .. } => None,
        RootInputBinding::Namespace { coordinate } => Some(coordinate),
    };
    pay(
        budget,
        world.accounts().len().saturating_mul(8).saturating_add(128),
    )?;
    let mut regions = Vec::new();
    // Root code receives precision first. Remaining immutable code accounts use
    // the world's stable order, including code sources shared by many owners.
    let (source, mode, _) = crate::analysis::transfer::resolve_entry_code(
        world.fork(),
        entry.address,
        entry.environment.to,
        |address| world.accounts().get(&address).map(|account| &account.code),
    );
    let source = (mode == crate::analysis::FrameCode::Runtime).then_some(source);
    for (address, account) in source
        .and_then(|source| world.accounts().get_key_value(&source))
        .into_iter()
        .chain(
            world
                .accounts()
                .iter()
                .filter(|(address, _)| Some(**address) != source && Some(**address) != namespace),
        )
    {
        if regions.len() >= limit {
            break;
        }
        if let Code::Runtime(program) = &account.code {
            pay(
                budget,
                program.byte_len().saturating_mul(4).saturating_add(8),
            )?;
            let hash = program.bytes_digest();
            let provenance = if Some(*address) == source {
                match declared {
                    ProcessSource::InputProgram { .. } | ProcessSource::Runtime { .. } => declared,
                    _ => ProcessSource::Runtime {
                        source: *address,
                        hash,
                        fork: program.fork(),
                    },
                }
            } else {
                ProcessSource::Runtime {
                    source: *address,
                    hash,
                    fork: program.fork(),
                }
            };
            regions.push(HybridRegion {
                source: provenance,
                code_address: *address,
                code_hash: hash,
            });
        }
    }
    Ok(regions)
}

pub(super) struct Selective<'a, S> {
    pub inner: &'a S,
    pub regions: &'a [HybridRegion],
}

pub(super) struct Context<'a, C>(pub &'a C);
impl<C: crate::analysis::ContextPolicy> crate::analysis::ContextPolicy for Context<'_, C> {
    fn advance(&self, history: &mut Vec<usize>, source: usize, depth: usize) {
        self.0.advance(history, source, depth);
    }
    fn normalize(&self, history: &mut Vec<usize>) {
        self.0.normalize(history);
    }
    fn work(&self, length: usize) -> usize {
        self.0.work(length)
    }
    fn normalization_work(&self, length: usize) -> usize {
        self.0.normalization_work(length)
    }
}

pub(super) struct Partition<'a, P>(pub &'a P);
impl<P: crate::analysis::PartitionPolicy> crate::analysis::PartitionPolicy for Partition<'_, P> {
    type Class = P::Class;
    fn classify(&self, arrival: crate::analysis::Arrival) -> Self::Class {
        self.0.classify(arrival)
    }
    fn classification_work(&self, arrival: crate::analysis::Arrival) -> usize {
        self.0.classification_work(arrival)
    }
}

impl<S> Selective<'_, S> {
    fn census<D: EvmNumericAlgebra>(
        &self,
        frames: usize,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<(), MachineError> {
        cx.reserve(
            frames
                .saturating_mul(self.regions.len().saturating_add(2))
                .saturating_add(1),
        )
    }
    fn erase(&self, frame: &mut FrameKey) {
        if !selected(self.regions, frame) {
            frame.jump_history.clear();
        }
    }
    fn changes(&self, frame: &FrameKey) -> bool {
        !frame.jump_history.is_empty() && !selected(self.regions, frame)
    }
}

impl<D, Tag, S> MachinePartition<D, Tag> for Selective<'_, S>
where
    D: EvmNumericAlgebra,
    S: MachinePartition<D, Tag>,
{
    type Key = S::Key;
    fn prepare(
        &self,
        input: &EvmMachinePayload<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Partitioned<Self::Key, D::Element>, MachineError> {
        self.census(input.call_stack.depth(), cx)?;
        let mut prepared = input.copy_for_policy(cx)?;
        for frame in prepared.call_stack.iter_mut() {
            self.erase(&mut frame.key);
        }
        self.inner
            .prepare_owned(prepared, site, class, cx)
            .map_err(|refusal| refusal.error)
    }
    fn prepare_relative(
        &self,
        input: &RelativeState<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<PartitionedRelative<Self::Key, D::Element>, MachineError> {
        self.census(
            input
                .frames
                .len()
                .saturating_add(input.location.frames.len()),
            cx,
        )?;
        let mut prepared = input.copy_for_policy(cx)?;
        for frame in &mut prepared.frames {
            self.erase(&mut frame.key);
        }
        for frame in &mut prepared.location.frames {
            self.erase(frame);
        }
        prepared.work = prepared.measure_work();
        self.inner
            .prepare_relative_owned(prepared, site, class, cx)
            .map_err(|refusal| refusal.error)
    }
    fn prepare_owned(
        &self,
        input: EvmMachinePayload<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> PartitionResult<Self::Key, D::Element> {
        if let Err(error) = self.census(input.call_stack.depth(), cx) {
            return Err(PartitionRefusal { input, error });
        }
        if !input
            .call_stack
            .iter()
            .any(|frame| self.changes(&frame.key))
        {
            return self.inner.prepare_owned(input, site, class, cx);
        }
        self.prepare(&input, site, class, cx)
            .map_err(|error| PartitionRefusal { input, error })
    }
    fn prepare_relative_owned(
        &self,
        input: RelativeState<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> RelativePartitionResult<Self::Key, D::Element> {
        if let Err(error) = self.census(
            input
                .frames
                .len()
                .saturating_add(input.location.frames.len()),
            cx,
        ) {
            return Err(PartitionRefusal { input, error });
        }
        if !input.frames.iter().any(|frame| self.changes(&frame.key))
            && !input
                .location
                .frames
                .iter()
                .any(|frame| self.changes(frame))
        {
            return self.inner.prepare_relative_owned(input, site, class, cx);
        }
        self.prepare_relative(&input, site, class, cx)
            .map_err(|error| PartitionRefusal { input, error })
    }
}

#[cfg(test)]
mod tests;

impl<D, S> WholeStatePolicy<D> for Selective<'_, S>
where
    D: EvmNumericAlgebra,
    S: WholeStatePolicy<D>,
{
    type Metadata = S::Metadata;
    fn initialize(
        &self,
        input: &EvmMachinePayload<D::Element>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self::Metadata, MachineError> {
        self.inner.initialize(input, cx)
    }
    fn metadata_work(&self, metadata: &Self::Metadata) -> usize {
        self.inner.metadata_work(metadata)
    }
    fn has_bound(&self, metadata: &Self::Metadata) -> bool {
        self.inner.has_bound(metadata)
    }
    fn admit(
        &self,
        current: &EvmMachinePayload<D::Element>,
        incoming: &EvmMachinePayload<D::Element>,
        metadata: &mut Self::Metadata,
        acceleration: Acceleration,
        selectors: &BTreeSet<ExprId>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Option<EvmMachinePayload<D::Element>>, MachineError> {
        self.inner
            .admit(current, incoming, metadata, acceleration, selectors, cx)
    }
    fn retain(&self, metadata: &mut Self::Metadata, incoming: EvmMachinePayload<D::Element>) {
        self.inner.retain(metadata, incoming);
    }
}

impl<S: AccelerationPolicy> AccelerationPolicy for Selective<'_, S> {
    fn decide(&self, input: AccelerationInput) -> Acceleration {
        self.inner.decide(input)
    }
}

impl<D, S> ProcedureSummaryPolicy<D> for Selective<'_, S>
where
    D: EvmNumericAlgebra,
    S: ProcedureSummaryPolicy<D>,
{
    fn runtime_execution(&self) -> RuntimeExecution {
        RuntimeExecution::Native
    }
    fn supports_early_known_admission(&self) -> bool {
        self.inner.supports_early_known_admission()
    }
    fn admit_relative(
        &self,
        current: &RelativeState<D::Element>,
        incoming: &RelativeState<D::Element>,
        site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<RelativeState<D::Element>, MachineError> {
        self.inner
            .admit_relative(current, incoming, site, acceleration, cx)
    }
    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<D::Element>>,
        incoming: &ProcedureOutput<D::Element>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<ProcedureOutput<D::Element>, MachineError> {
        self.inner.admit_output(current, incoming, acceleration, cx)
    }
}
