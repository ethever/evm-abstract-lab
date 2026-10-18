//! Prepare the original root's terminal relation before detailed execution.
//!
//! This module is reachable only from the engine's paid `initial()` seam. The
//! owned anchor is authority from that seam, not a Store/World equality test or
//! a hash recovered from a later state. Every optional stage checks its complete
//! fee before charging, and every actual finalizer uses the original ledger.

use crate::{
    analysis::{
        ExecutionConfig, FrameCode, FrontierReason, MachinePayload, OutcomeKind,
        RootTerminalCoverage, RootTerminalInputBinding, RootTerminalOutcome, RootTerminalScope,
        RootTerminalTrigger, layers,
    },
    domain::{Domain, symbolic::ByteStateKind},
    resource::{AdmissionStop, CompletionPermit, WorkBudget},
    world::{ByteArray, Entry, RangeError, SnapshotIdentity, Store, StoreError, World},
};
use alloy_primitives::{Address, U256};
use analysis_control::Cancellation;
use thiserror::Error;

pub(super) mod boundary;
#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod tests;

#[derive(Debug, Error)]
pub(super) enum PrepareError {
    #[error("root terminal preparation requires the actual session identity")]
    MissingIdentity,
    #[error("unsupported original root: {0:?}")]
    Unsupported(UnsupportedRoot),
    #[error("optional root terminal admission refused: {0:?}")]
    Admission(AdmissionStop),
    #[error("checked root terminal Store: {0}")]
    Store(StoreError),
    #[error("checked complete terminal bytes: {0}")]
    Bytes(RangeError),
    #[error("checked root transaction finalization: {0:?}")]
    Finalization(FrontierReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UnsupportedRoot {
    ActiveChild,
    NotRuntime,
    NotEntry,
    InputByteView,
    Configuration,
}

impl From<AdmissionStop> for PrepareError {
    fn from(stop: AdmissionStop) -> Self {
        Self::Admission(stop)
    }
}

impl From<StoreError> for PrepareError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<RangeError> for PrepareError {
    fn from(error: RangeError) -> Self {
        Self::Bytes(error)
    }
}

/// Neither this authority nor the ready publication capability can be cloned.
/// Captured Stores include all World assumptions without cloning the World.
struct RootAnchor {
    input: Box<RootTerminalInputBinding>,
}

/// Entirely constructed, finalized and lifetime-funded before detail starts.
pub(super) struct ReadyRoot {
    anchor: RootAnchor,
    outcomes: Box<[RootTerminalOutcome; 3]>,
    permit: CompletionPermit,
}

impl ReadyRoot {
    /// No walks, allocations, charges, refunds or ledger access after detail.
    pub(super) fn publish(self, trigger: RootTerminalTrigger) -> Option<RootTerminalCoverage> {
        let Self {
            anchor,
            outcomes,
            permit,
        } = self;
        // Consume the original payer's cancellation identity immediately before
        // the atomic move into the result; ambient TLS cannot grant permission.
        if !permit.publish_allowed() {
            return None;
        }
        Some(RootTerminalCoverage {
            scope: RootTerminalScope::AllInputStates,
            input: anchor.input,
            outcomes,
            trigger,
        })
    }
}

/// Called only between the real paid root initialization and states[0] install.
pub(super) fn prepare(
    initial: &MachinePayload,
    world: &World,
    entry: &Entry,
    config: &ExecutionConfig,
    domain: Domain,
    identity: Option<(&layers::SnapshotId, layers::FactRevision)>,
    budget: &mut WorkBudget,
) -> Result<ReadyRoot, PrepareError> {
    let cancellation = budget.cancellation();
    // Checked byte/state APIs and the real finalizer consult cancellation TLS.
    // Bind them to the payer too, rather than trusting the caller's current scope.
    analysis_control::with_cancellation(&cancellation, || {
        prepare_with_bytes(
            initial,
            world,
            entry,
            config,
            domain,
            identity,
            budget,
            &cancellation,
            ByteArray::unknown_checked,
        )
    })
}

fn finite(work: usize) -> Result<usize, PrepareError> {
    if work == usize::MAX {
        Err(AdmissionStop::Overflow.into())
    } else {
        Ok(work)
    }
}

fn available(
    budget: &WorkBudget,
    cancellation: &Cancellation,
    work: usize,
) -> Result<(), PrepareError> {
    if cancellation.is_cancelled() {
        return Err(AdmissionStop::Cancelled.into());
    }
    if budget.exhausted() {
        return Err(AdmissionStop::AlreadyExhausted.into());
    }
    finite(work)?;
    if !budget.can_afford(work) {
        return Err(if cancellation.is_cancelled() {
            AdmissionStop::Cancelled
        } else {
            AdmissionStop::Insufficient
        }
        .into());
    }
    Ok(())
}

fn reserve(
    budget: &mut WorkBudget,
    cancellation: &Cancellation,
    work: usize,
) -> Result<(), PrepareError> {
    available(budget, cancellation, work)?;
    // Exclusive ledger access makes cancellation the only race after the check.
    if !budget.charge(work) {
        return Err(AdmissionStop::Cancelled.into());
    }
    Ok(())
}

fn tree_work(entries: usize) -> usize {
    entries
        .saturating_add(1)
        .saturating_mul((usize::BITS - entries.leading_zeros()) as usize + 1)
}

/// Base calldata only: quoting a view must not first walk its unpaid DAG.
fn input_headers(bytes: &ByteArray) -> Result<(usize, usize), PrepareError> {
    let physical = match bytes.state_root().kind() {
        ByteStateKind::Exact(bytes) => bytes.len(),
        ByteStateKind::Input { .. } => 0,
        _ => return Err(PrepareError::Unsupported(UnsupportedRoot::InputByteView)),
    };
    Ok((
        bytes
            .metadata_len()
            .saturating_add(bytes.state_root().nodes()),
        physical,
    ))
}

/// Expanded field cost and raw code are separate: byte length never becomes a
/// map cardinality or a multiplier for account cleanup. Discovery is prepaid.
#[derive(Clone, Copy, Debug)]
struct StoreShape {
    fields: usize,
    code_bytes: usize,
}

impl StoreShape {
    fn measure(store: &Store) -> Result<Self, PrepareError> {
        let code_bytes = finite(store.code_identity_bytes())?;
        let work = finite(store.work_size())?;
        let fields = store
            .symbolic_facts(false)
            .chain(store.symbolic_facts(true))
            .fold(
                work.saturating_sub(code_bytes)
                    .saturating_add(store.finalization_entries()),
                |fields, (expression, _)| fields.saturating_add(expression.work_size()),
            );
        Ok(Self {
            fields: finite(fields)?,
            code_bytes,
        })
    }
}

/// The genuine initial Store has no pending destruction. Its sole definite
/// touch belongs to nonempty runtime/delegating code. Future address-3 touch
/// adds MAY cleanup only; the independent Return Store has no explicit owners.
/// Thus the finalizer uses its owner-local no-delete plan: fields and candidate
/// unions are visited once, plus fixed per-owner zero/root/selector metadata
/// and ordered-map heights.
///
/// Four field allowances and 256 per owner cover the post-cleanup roots, scalar
/// unions, new map keys and both planes. 128 ordered passes dominate all four
/// Footprint discoveries, staging/restore, plan setup/validation/COW/local/maps,
/// final validation and partial drops. This is an affordability guard only;
/// `finalize_transaction_checked` charges its actual receipts, never this quote.
fn finalization_bound(current: StoreShape, saved: Option<StoreShape>, owners: usize) -> usize {
    let fields = current
        .fields
        .saturating_add(saved.map_or(0, |shape| shape.fields));
    let grown = fields
        .saturating_mul(4)
        .saturating_add(owners.saturating_mul(256))
        .saturating_add(128);
    tree_work(grown)
        .saturating_mul(128)
        .saturating_add(current.code_bytes.saturating_mul(8))
}

#[expect(
    clippy::too_many_arguments,
    reason = "private original-entry preparation and local checked-byte injection"
)]
fn prepare_with_bytes(
    initial: &MachinePayload,
    world: &World,
    entry: &Entry,
    config: &ExecutionConfig,
    domain: Domain,
    identity: Option<(&layers::SnapshotId, layers::FactRevision)>,
    budget: &mut WorkBudget,
    cancellation: &Cancellation,
    mut bytes: impl FnMut() -> Result<ByteArray, RangeError>,
) -> Result<ReadyRoot, PrepareError> {
    let (snapshot, revision) = identity.ok_or(PrepareError::MissingIdentity)?;
    reserve(budget, cancellation, 1)?;
    if initial.call_stack.active_child().is_some() {
        return Err(PrepareError::Unsupported(UnsupportedRoot::ActiveChild));
    }
    let frame = &initial.call_stack.root().state;
    let Some(program) = frame.program.as_ref().filter(|program| {
        frame.code == FrameCode::Runtime
            && frame.key.mode == FrameCode::Runtime
            && program.byte_len() != 0
            && program.fork() == world.fork()
    }) else {
        return Err(PrepareError::Unsupported(UnsupportedRoot::NotRuntime));
    };
    if !frame.stack.is_empty()
        || frame.key.stack_height != 0
        || frame.key.basic_block_index != 0
        || !frame.key.jump_history.is_empty()
        || !frame.environment_calldata
        || frame.address_operand.is_some()
        || frame.caller_operand.is_some()
        || frame.key.address != entry.address
        || frame.key.address_value != entry.environment.to
        || frame.key.caller != entry.environment.caller
        || frame.key.is_static != entry.environment.is_static
        || !frame.memory.is_memory()
        || frame.memory.len().singleton() != Some(U256::ZERO)
        || !matches!(frame.memory.state_root().kind(), ByteStateKind::Zero)
        || !matches!(frame.returndata.state_root().kind(), ByteStateKind::Exact(bytes) if bytes.is_empty())
        || frame.memory.metadata_len() != 2
        || frame.returndata.metadata_len() != 2
        || !initial.relations.is_empty()
        || !initial.store.possible_logs().is_empty()
        || initial.store.logs_unknown()
        || !frame.saved_store.state().possible_logs().is_empty()
        || frame.saved_store.state().logs_unknown()
    {
        return Err(PrepareError::Unsupported(UnsupportedRoot::NotEntry));
    }
    if config
        .domain()
        .map_or(true, |configured| configured.spec() != domain.spec())
    {
        return Err(PrepareError::Unsupported(UnsupportedRoot::Configuration));
    }

    let (calldata_headers, calldata_bytes) = input_headers(&frame.calldata)?;
    let (request_headers, request_bytes) = input_headers(&entry.environment.calldata)?;
    let text = match snapshot.identity() {
        SnapshotIdentity::Offline { label } => label.len(),
        SnapshotIdentity::Chain { .. } => 0,
    };
    // Only O(1) headers above. Pay before Store/value/byte metadata walks or any
    // owned clone; offline labels are unbounded strings and are paid linearly.
    let headers = initial
        .store
        .finalization_entries()
        .saturating_add(frame.saved_store.state().finalization_entries())
        .saturating_add(calldata_headers)
        .saturating_add(request_headers)
        .saturating_add(32);
    reserve(
        budget,
        cancellation,
        tree_work(headers)
            .saturating_mul(32)
            .saturating_add(
                calldata_bytes
                    .saturating_add(request_bytes)
                    .saturating_mul(4),
            )
            .saturating_add(text.saturating_mul(4)),
    )?;
    let entered_shape = StoreShape::measure(&initial.store)?;
    let saved_shape = StoreShape::measure(frame.saved_store.state())?;
    let input_fields = entered_shape
        .fields
        .saturating_add(saved_shape.fields)
        .saturating_add(frame.calldata.work_size())
        .saturating_add(frame.call_value.work_size())
        .saturating_add(entry.environment.work_size());
    let input_code = entered_shape
        .code_bytes
        .saturating_add(saved_shape.code_bytes)
        .saturating_add(program.byte_len());
    let owners = world.accounts().len().saturating_add(1);
    // Covers anchor + branch cloning, checked fresh roots/bytes, all metadata
    // checks/censuses below and partial-result drops. Existing Program/checkpoint
    // clones share backing; raw code and label retention/drop remain linear.
    reserve(
        budget,
        cancellation,
        tree_work(input_fields.saturating_add(owners.saturating_mul(256)))
            .saturating_mul(128)
            .saturating_add(input_code.saturating_mul(4))
            .saturating_add(text.saturating_mul(4))
            .saturating_add(8192),
    )?;

    // Sole authority mint. No recomputed initial Store, input lift or equality
    // check can substitute for the real initialization call at the engine seam.
    let anchor = RootAnchor {
        input: Box::new(RootTerminalInputBinding {
            snapshot: snapshot.clone(),
            revision,
            storage_source: world.storage_source_token(),
            input_scope: entry.environment.input_scope.id(),
            fork: world.fork(),
            entry: entry.clone(),
            config: config.clone(),
            domain_spec: domain.spec(),
            frame: frame.key.clone(),
            program: program.clone(),
            calldata: frame.calldata.clone(),
            call_value: frame.call_value.clone(),
            saved_store: frame.saved_store.clone(),
            initial_store: initial.store.clone(),
        }),
    };
    // Static suffixes can also delete touched, omitted empty owners at cleanup.
    // Independent all-account effects cover those changes under the same input.
    let mut returned = Store::root_terminal_all_effects_checked(domain)?;
    let return_data = bytes()?;
    return_data.check_existing(domain.spec().relations().expressions())?;
    let revert_data = bytes()?;
    revert_data.check_existing(domain.spec().relations().expressions())?;
    let mut reverted = initial.store.clone();
    reverted.may_touch_account(Address::from_word(U256::from(3).into()));
    let mut failed = reverted.clone();
    let mut guards = [0; 3];
    for (index, (store, rollback, branch_owners)) in [
        (&mut returned, None, 0),
        (&mut reverted, Some(&anchor.input.saved_store), owners),
        (&mut failed, Some(&anchor.input.saved_store), owners),
    ]
    .into_iter()
    .enumerate()
    {
        let current = StoreShape::measure(store)?;
        let guard = finalization_bound(current, rollback.map(|_| saved_shape), branch_owners);
        available(budget, cancellation, guard)?;
        let finalized = store.finalize_transaction_checked(domain, rollback, budget);
        if let Err(error) = finalized {
            return Err(if cancellation.is_cancelled() || error.is_cancelled() {
                PrepareError::Admission(AdmissionStop::Cancelled)
            } else if let Some(error) = error.into_store_error() {
                PrepareError::Store(error)
            } else {
                PrepareError::Finalization(FrontierReason::Work)
            });
        }
        guards[index] = guard;
    }
    let outcomes = Box::new([
        RootTerminalOutcome {
            kind: OutcomeKind::Return,
            data: return_data,
            store: returned,
        },
        RootTerminalOutcome {
            kind: OutcomeKind::Revert,
            data: revert_data,
            store: reverted,
        },
        RootTerminalOutcome {
            kind: OutcomeKind::Failure,
            data: ByteArray::empty(),
            store: failed,
        },
    ]);
    let (result_fields, result_code) = outcomes.iter().try_fold(
        (input_fields, input_code),
        |(fields, code), outcome| -> Result<_, PrepareError> {
            let shape = StoreShape::measure(&outcome.store)?;
            Ok((
                fields
                    .saturating_add(shape.fields)
                    .saturating_add(outcome.data.work_size()),
                code.saturating_add(shape.code_bytes),
            ))
        },
    )?;
    // Lifetime owns the complete retained/drop bound plus constant publication.
    // Keeping it >= each guard also means a budget equal to the final actual
    // receipt admits every earlier guard. Guards themselves were never charged.
    let lifetime = tree_work(result_fields)
        .saturating_mul(16)
        .saturating_add(result_code.saturating_mul(4))
        .saturating_add(text.saturating_mul(4))
        .saturating_add(64)
        .max(guards.into_iter().max().unwrap_or(0));
    let permit = budget.try_completion_escrow(lifetime)?;
    Ok(ReadyRoot {
        anchor,
        outcomes,
        permit,
    })
}
