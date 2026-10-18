//! Native-root completion owns only complete output buffers and the live Store.
//!
//! The unchanged input retains frames, relations, checkpoints and their shared
//! graphs. Discovery is admitted before metadata scans; the error-key receipt
//! permits one lazy original key, without hashing any code on a successful path.

use super::{Execution, TransferContext, success_data};
use crate::{
    analysis::{FrameCode, FrontierReason, MachinePayload, OutcomeKind},
    domain::{AbstractValue, Domain, provenance::MAX_ORIGINS},
    world::{ByteArray, Store},
};
use revm_precompile::PrecompileId;

use super::super::{ExecutionFrontier, Outcome, WorkBudget, boundary_on, boundary_reason, calls};

#[cfg(test)]
pub(super) mod tests;

// The same logical bound applies to std's owned maps and imbl's shared maps.
// These include temporary-container drops; shared contents stay alive in input.
const HANDLE_WORK: usize = 2;
const MAP_ENTRY_WORK: usize = 4;
const VALUE_INLINE_WORK: usize = 20;
const ARRAY_FIXED_WORK: usize = 16;
const STORE_ENTRY_WORK: usize = 12;
const FRAME_KEY_WORK: usize = 20;
const MACHINE_KEY_WORK: usize = 16; // outer Vec, hash word, allocation and disposal
const TOUCH_QUERY_WORK: usize = (usize::BITS as usize + 1) * 16;

// Unknown/empty each construct <=4 scalar wrappers, <=8 scalar/source/state
// handles and <=2 two-entry container temporaries. Scalar units include the
// maximum owned OriginSet and single-constant numeric allocation/drop; neither
// constructor has an arbitrary finite set or inherited graph to traverse.
const MAX_VALUE_WORK: usize = VALUE_INLINE_WORK + MAX_ORIGINS * MAP_ENTRY_WORK;
const FRESH_BUFFER_WORK: usize =
    ARRAY_FIXED_WORK + 4 * MAX_VALUE_WORK + 8 * HANDLE_WORK + 2 * ((2 + 1) * (2 + 1) * 16); // two <=2-entry ordered-tree temporaries
// Three Vec entries/moves, all quote headers/arithmetic, cancellation dispatch,
// buffered-iterator disposal and one frontier's fixed fields/allocation.
const DISPATCH_WORK: usize = 64;

fn reserve(budget: &mut WorkBudget, work: usize) -> Option<()> {
    (!budget.exhausted() && work != usize::MAX && budget.charge(work)).then_some(())
}

fn tree_units(entries: usize) -> usize {
    let height = (usize::BITS - entries.leading_zeros()) as usize + 1;
    entries.saturating_add(1).saturating_mul(height)
}

fn tree_work(entries: usize) -> usize {
    tree_units(entries).saturating_mul(16)
}

pub(super) fn applicable(payload: &MachinePayload) -> bool {
    payload.call_stack.active_child().is_none()
        && matches!(payload.active().code, FrameCode::Precompile(_))
}

fn value_clone_work(value: &AbstractValue) -> usize {
    VALUE_INLINE_WORK.saturating_add(
        value
            .provenance()
            .origins()
            .work_size()
            .saturating_mul(MAP_ENTRY_WORK),
    )
}

/// Only directly owned metadata is copied: Exact data, finite candidates,
/// expressions, byte roots and conditional scopes keep their shared contents.
/// The caller must reserve a census before reading any scalar metadata.
fn array_clone_work(array: &ByteArray) -> usize {
    let bytes = array.stored_bytes();
    let operands = array.symbolic_facts();
    let cells = bytes.len().saturating_add(operands.len());
    let expressions = operands.len();
    let allocations = array.allocation_facts().len();
    let values = bytes
        .map(|(_, value)| value)
        .chain(operands.map(|(_, value)| value))
        .chain([array.len(), array.default_byte()])
        .fold(0usize, |work, value| {
            work.saturating_add(value_clone_work(value))
        });
    ARRAY_FIXED_WORK
        .saturating_add(values)
        .saturating_add(cells.saturating_mul(MAP_ENTRY_WORK))
        .saturating_add(expressions.saturating_mul(HANDLE_WORK))
        .saturating_add(allocations.saturating_mul(MAP_ENTRY_WORK + 2 * HANDLE_WORK))
        .saturating_add(
            array
                .cloned_conditional_branches()
                .saturating_mul(HANDLE_WORK + 2),
        )
}

/// Complete validation is priced from constructed data, never from a guessed
/// id-specific validation shape. Construction/drop has already been admitted.
#[derive(Debug)]
struct BufferWork {
    validation: usize,
}

impl BufferWork {
    fn measure(data: &ByteArray, budget: &mut WorkBudget) -> Option<Self> {
        // work_size visits local scalar/allocation metadata and exact_work's
        // seen-set/root census. Exact contents contribute a cached length only.
        reserve(
            budget,
            data.metadata_len()
                .saturating_add(1)
                .saturating_mul(2)
                .saturating_add(tree_work(data.state_root().work_size())),
        )?;
        let allocations = data.allocation_facts().len();
        let conditional_entries = data
            .metadata_len()
            .saturating_sub(data.cloned_value_count())
            .saturating_sub(allocations);
        let conditional_census = if conditional_entries == 0 {
            0
        } else {
            tree_work(conditional_entries)
        };
        // visit_values builds a temporary top wrapper per root expression,
        // per otherwise unrepresented conditional expression, and twice per
        // allocation witness. Each root expression consumes a child node, so
        // root.nodes - 1 bounds their count (root itself consumes one node).
        let wrappers = data
            .state_root()
            .work_size()
            .saturating_sub(1)
            .saturating_add(conditional_entries)
            .saturating_add(allocations.saturating_mul(2));
        Some(Self {
            validation: data
                .work_size()
                .saturating_add(tree_work(data.state_root().work_size()))
                .saturating_add(data.metadata_len().saturating_mul(2))
                .saturating_add(conditional_census)
                .saturating_add(wrappers.saturating_mul(VALUE_INLINE_WORK)),
        })
    }

    fn total(&self) -> usize {
        self.validation
    }
}

/// The slice quote specialized algebraically to a fresh unknown source:
/// root.nodes=3, no facts/cells/conditional scopes, constant range work=28.
/// This is exactly bytes/slice.rs::slice_work's bound (T has no *16 factor).
fn fresh_slice_work(domain: Domain) -> usize {
    let union = tree_units(domain.capacity().saturating_add(1))
        .saturating_add(tree_units(MAX_ORIGINS.saturating_mul(2)))
        .saturating_add(12);
    418usize.saturating_add(union.saturating_mul(4))
}

#[derive(Debug)]
struct ConstructionWork {
    returned: usize,
    other_buffers: usize,
}

impl ConstructionWork {
    fn measure(
        input: &ByteArray,
        id: &PrecompileId,
        domain: Domain,
        budget: &mut WorkBudget,
    ) -> Option<Self> {
        let returned = match id {
            PrecompileId::Identity => {
                // ByteArray's maps are std maps even with the imbl Store.
                reserve(budget, input.cloned_value_count().saturating_add(1))?;
                array_clone_work(input)
            }
            PrecompileId::EcRec | PrecompileId::P256Verify => {
                // Empty and sliced-word inputs have no facts, allocations or
                // conditional scopes. The join performs two input validations,
                // two scalar joins (length/default), one length ITE and one
                // byte ITE; scalar joins contain at most two constants. Pay
                // graph/temporary disposal even if retention rejects the ITE.
                let input_validation = tree_work(1)
                    .saturating_add(tree_work(6))
                    .saturating_add(7 * VALUE_INLINE_WORK)
                    .saturating_add(2 * (ARRAY_FIXED_WORK + 2 * MAX_VALUE_WORK));
                let joined_values = 2usize.saturating_mul(
                    tree_work(2)
                        // Both inputs have only Constant/Top provenance;
                        // neither join can create more than two origin entries.
                        .saturating_add(tree_work(2))
                        .saturating_add(2 * MAX_VALUE_WORK),
                );
                let joined_roots = 2 * (ARRAY_FIXED_WORK + 4 * HANDLE_WORK);
                FRESH_BUFFER_WORK
                    .saturating_mul(2)
                    .saturating_add(fresh_slice_work(domain))
                    .saturating_add(input_validation)
                    .saturating_add(joined_values)
                    .saturating_add(joined_roots)
            }
            PrecompileId::Sha256
            | PrecompileId::Ripemd160
            | PrecompileId::Bn254Pairing
            | PrecompileId::Bls12Pairing
            | PrecompileId::Bn254Add
            | PrecompileId::Bn254Mul
            | PrecompileId::Blake2F
            | PrecompileId::KzgPointEvaluation
            | PrecompileId::Bls12G1Add
            | PrecompileId::Bls12G1Msm
            | PrecompileId::Bls12MapFpToGp1
            | PrecompileId::Bls12G2Add
            | PrecompileId::Bls12G2Msm
            | PrecompileId::Bls12MapFp2ToGp2 => {
                FRESH_BUFFER_WORK.saturating_add(fresh_slice_work(domain))
            }
            // Includes ModExp and future provider ids, matching success_data.
            _ => FRESH_BUFFER_WORK,
        };
        Some(Self {
            returned,
            other_buffers: FRESH_BUFFER_WORK.saturating_mul(2),
        })
    }

    fn total(&self) -> usize {
        self.returned.saturating_add(self.other_buffers)
    }
}

#[derive(Debug)]
struct StoreWork {
    clone_and_drop: usize,
}

impl StoreWork {
    fn measure(store: &Store, budget: &mut WorkBudget) -> Option<Self> {
        let entries = store.finalization_entries();
        // O(1) lengths prepay scalar and log metadata before either is visited.
        reserve(budget, entries.saturating_add(1).saturating_mul(2))?;
        let mut clone_and_drop = store
            .cloned_values()
            .fold(entries.saturating_mul(STORE_ENTRY_WORK), |work, value| {
                work.saturating_add(value_clone_work(value))
            });
        for log in store.possible_logs().values() {
            reserve(budget, log.topics.len().saturating_add(1))?;
            reserve(budget, log.data.cloned_value_count().saturating_add(1))?;
            clone_and_drop = log.topics.iter().fold(
                clone_and_drop.saturating_add(array_clone_work(&log.data)),
                |work, value| work.saturating_add(value_clone_work(value)),
            );
        }
        Some(Self { clone_and_drop })
    }
}

#[derive(Debug)]
struct CompletionWork {
    live_stores: usize,
    relative_rollbacks: usize,
    buffers: ConstructionWork,
    dispatch: usize,
}

impl CompletionWork {
    fn measure(
        payload: &MachinePayload,
        id: &PrecompileId,
        context: &mut TransferContext<'_>,
    ) -> Option<Self> {
        let current = StoreWork::measure(&payload.store, context.budget)?;
        let relative_rollbacks = if context.procedure {
            let saved = StoreWork::measure(
                payload.call_stack.root().state.saved_store.state(),
                context.budget,
            )?;
            // Each shared checkpoint restores one Store and makes three fixed
            // address tree queries/updates. The flat saved-Store bound also
            // pays imbl's first-write COW and its temporary node drops. Input
            // retains the checkpoint, so try_unwrap cannot move its Store.
            saved
                .clone_and_drop
                .saturating_add(TOUCH_QUERY_WORK.saturating_mul(3))
                .saturating_add(HANDLE_WORK)
                .saturating_add(STORE_ENTRY_WORK)
                .saturating_mul(2)
        } else {
            // Transaction finalization owns staging, rollback, sticky-touch
            // updates and postvalidation. Its existing ledger stays intact.
            0
        };
        Some(Self {
            live_stores: current.clone_and_drop.saturating_mul(3),
            relative_rollbacks,
            buffers: ConstructionWork::measure(
                &payload.active().calldata,
                id,
                context.domain,
                context.budget,
            )?,
            dispatch: DISPATCH_WORK,
        })
    }

    fn total(&self) -> usize {
        self.live_stores
            .saturating_add(self.relative_rollbacks)
            .saturating_add(self.buffers.total())
            .saturating_add(self.dispatch)
    }
}

#[derive(Debug)]
struct KeyWork {
    owner_trees: usize,
    record_bytes: usize,
    runtime_bytes: usize,
    frame: usize,
}

impl KeyWork {
    fn measure(payload: &MachinePayload, budget: &mut WorkBudget) -> Option<Self> {
        let entries = payload.store.code_identity_entries();
        // Actual code-identity map lengths, excluding finalization's 16 fixed
        // headers. Admit the runtime-length census before code iteration.
        reserve(budget, entries.saturating_add(1))?;
        let runtime_bytes = payload.store.code_identity_bytes();
        Some(Self {
            // Address-set insertion and at most eight code/lifecycle lookups.
            // The set and record buffer are temporary, with disposal included.
            owner_trees: tree_work(entries).saturating_mul(9),
            record_bytes: entries
                .saturating_mul(20 + 1 + 32 + 3)
                .saturating_add(2)
                .saturating_mul(4),
            runtime_bytes: runtime_bytes.saturating_mul(2),
            frame: FRAME_KEY_WORK
                .saturating_add(MACHINE_KEY_WORK)
                .saturating_add(payload.active().key.jump_history.len().saturating_mul(2)),
        })
    }

    fn total(&self) -> usize {
        self.owner_trees
            .saturating_add(self.record_bytes)
            .saturating_add(self.runtime_bytes)
            .saturating_add(self.frame)
    }
}

/// A prepaid allowance, not a precomputed key. Consuming it at the first
/// error is the only key/hash operation in this module.
struct ErrorKeyAllowance;

impl ErrorKeyAllowance {
    fn refuse(self, result: &mut Execution, reason: FrontierReason) {
        let key = result.payload.key();
        boundary_on(result, 0, reason, key);
    }
}

fn refuse_without_key(result: &mut Execution) {
    result.frontiers.push(ExecutionFrontier {
        pc: 0,
        reason: boundary_reason(FrontierReason::Work),
        target: None,
        range: None,
        payload: None,
    });
}

pub(super) fn summarize(
    result: &mut Execution,
    id: &PrecompileId,
    context: &mut TransferContext<'_>,
) {
    // The caller proves actual native code and no child. finish_creation's
    // first child lookup would return None; root and checkpoint stay borrowed.
    let Some(key_work) = KeyWork::measure(&result.payload, context.budget) else {
        refuse_without_key(result);
        return;
    };
    if reserve(context.budget, key_work.total()).is_none() {
        refuse_without_key(result);
        return;
    }
    let error_key = ErrorKeyAllowance;
    let Some(work) = CompletionWork::measure(&result.payload, id, context) else {
        error_key.refuse(result, FrontierReason::Work);
        return;
    };
    if reserve(context.budget, work.total()).is_none() {
        error_key.refuse(result, FrontierReason::Work);
        return;
    }
    let data = (|| {
        let returned = success_data(&result.payload.active().calldata, id, context.domain)?;
        let reverted = ByteArray::unknown_checked()?;
        let failed = ByteArray::empty();
        Ok::<_, crate::world::RangeError>([returned, reverted, failed])
    })();
    let data = match data {
        Ok(data) => data,
        Err(error) => {
            error_key.refuse(result, super::super::errors::bytes(error));
            return;
        }
    };
    // Complete metadata discovery/validation of ALL buffers precedes the first
    // Store clone/publication. Refused discovery still drops prepaid buffers.
    let mut validation = 0usize;
    for data in &data {
        let Some(work) = BufferWork::measure(data, context.budget) else {
            error_key.refuse(result, FrontierReason::Work);
            return;
        };
        validation = validation.saturating_add(work.total());
    }
    if reserve(context.budget, validation).is_none() {
        error_key.refuse(result, FrontierReason::Work);
        return;
    }
    let limits = context.domain.spec().relations().expressions();
    for data in &data {
        if let Err(error) = data.check_existing(limits) {
            error_key.refuse(result, super::super::errors::bytes(error));
            return;
        }
    }
    for (kind, data) in [
        OutcomeKind::Return,
        OutcomeKind::Revert,
        OutcomeKind::Failure,
    ]
    .into_iter()
    .zip(data)
    {
        if reserve(context.budget, 0).is_none() {
            error_key.refuse(result, FrontierReason::Work);
            return;
        }
        let mut store = result.payload.store.clone();
        if let Err(reason) = calls::finish_root_store(
            &mut store,
            &result.payload.call_stack.root().state.saved_store,
            kind,
            context,
        ) {
            // Checked finalization is atomic. Its error leaves this clone at
            // the original Store identity, as in ordinary finish's error key.
            error_key.refuse(result, reason);
            return;
        }
        result.outcomes.push(Outcome { kind, data, store });
    }
}
