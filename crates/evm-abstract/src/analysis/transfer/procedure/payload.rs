//! Canonical numerical bounds, finite stack aliases and independent instances.

use super::{
    AbstractValue, ByteArray, CallStack, ChildFrame, Continuation, Domain, Entry, FrameCode,
    FrameState, FrameSummary, MachineError, MachinePayload, NumericValue, PayloadSummary,
    RelationState, RootFrame, Snapshot, relations, stack_alias::StackAliases,
};

fn scalar(value: &AbstractValue) -> AbstractValue {
    AbstractValue::from_numeric(
        value
            .singleton()
            .map_or_else(|| value.numeric().clone(), NumericValue::constant),
    )
}

fn join_scalar(
    old: &AbstractValue,
    next: &AbstractValue,
    domain: Domain,
    widen: bool,
) -> AbstractValue {
    let joined = scalar(&domain.join(old, next));
    if widen {
        scalar(&domain.widen_procedure(old, &joined))
    } else {
        joined
    }
}

impl PayloadSummary {
    pub(super) fn active(&self) -> &FrameSummary {
        self.frames.last().expect("nonempty relative stack")
    }
    pub(super) fn work_size(&self) -> usize {
        self.work
    }

    pub(super) fn measure_work(&self) -> usize {
        let store_work = self.store.work_size();
        let observation_work = self.storage_observations.work_size(&self.active().stack);
        let frames = self.frames.iter().fold(
            store_work.saturating_add(observation_work),
            |work, frame| {
                let program = frame.program.as_ref().map_or(0, |program| {
                    program
                        .blocks()
                        .iter()
                        .fold(program.byte_len(), |work, block| {
                            work.saturating_add(block.instructions.len())
                        })
                });
                frame.stack.iter().fold(
                    work.saturating_add(frame.memory.work_size())
                        .saturating_add(frame.calldata.work_size())
                        .saturating_add(frame.returndata.work_size())
                        .saturating_add(frame.saved_store.work_size())
                        .saturating_add(frame.call_value.work_size().saturating_mul(2))
                        .saturating_add(frame.address_operand.work_size().saturating_mul(2))
                        .saturating_add(frame.caller_operand.work_size().saturating_mul(2))
                        .saturating_add(frame.key.jump_history.len())
                        .saturating_add(frame.stack_aliases.work_size())
                        .saturating_add(program)
                        .saturating_add(4),
                    |work, value| work.saturating_add(value.work_size().saturating_mul(2)),
                )
            },
        );
        self.continuations
            .iter()
            .fold(frames, |work, continuation| {
                work.saturating_add(continuation.output_offset.work_size())
                    .saturating_add(continuation.output_size.work_size())
                    .saturating_add(1)
            })
    }
}

/// Tree lookup/allocation and numeric Eq are reserved before alias formation.
/// Numeric Eq short-circuits unequal finite lengths; equal bounds have equal
/// numeric traversal sizes. Source expression comparison is never required.
pub(super) fn alias_form_work(payload: &MachinePayload) -> usize {
    payload.call_stack.iter().fold(0usize, |work, frame| {
        if frame.code == FrameCode::Unknown {
            work
        } else {
            frame.stack.iter().fold(
                work.saturating_add(StackAliases::work_for_len(frame.stack.len())),
                |work, value| work.saturating_add(value.work_size().saturating_mul(2)),
            )
        }
    })
}

pub(super) fn form(
    payload: &MachinePayload,
    entry: &Entry,
    domain: Domain,
) -> Result<PayloadSummary, MachineError> {
    MachineError::check_cancelled()?;
    let storage_observations =
        super::storage_observations::StorageObservations::form(payload, domain)?;
    let frames =
        payload
            .call_stack
            .iter()
            .map(|frame| {
                let mut key = frame.key.clone();
                let unknown = frame.code == FrameCode::Unknown;
                key.jump_history = if unknown {
                    Vec::new()
                } else {
                    super::context::canonical(&key.jump_history)?
                };
                let stack = if unknown {
                    Vec::new()
                } else {
                    frame.stack.iter().map(scalar).collect()
                };
                let stack_aliases = StackAliases::form(
                    if unknown { &[] } else { &frame.stack },
                    domain.spec().relations().enabled,
                )?;
                key.stack_height = stack.len();
                if unknown {
                    key.basic_block_index = 0;
                }
                Ok(FrameSummary {
                    key,
                    code: frame.code,
                    program: frame.program.clone(),
                    stack,
                    stack_aliases,
                    memory: if unknown {
                        ByteArray::memory().summary_projection(domain)?
                    } else {
                        frame.memory.summary_projection(domain)?
                    },
                    calldata: if unknown {
                        ByteArray::empty().summary_projection(domain)?
                    } else {
                        frame.calldata.summary_projection(domain)?
                    },
                    environment_calldata: frame.environment_calldata,
                    returndata: if unknown {
                        ByteArray::empty().summary_projection(domain)?
                    } else {
                        frame.returndata.summary_projection(domain)?
                    },
                    call_value: scalar(&frame.call_value),
                    address_operand: scalar(&frame.address_operand.clone().unwrap_or_else(|| {
                        entry.environment.address_value(frame.key.address_value)
                    })),
                    caller_operand: scalar(
                        &frame
                            .caller_operand
                            .clone()
                            .unwrap_or_else(|| entry.environment.address_value(frame.key.caller)),
                    ),
                    saved_store: frame.saved_store.state().summary_projection(domain)?,
                })
            })
            .collect::<Result<Vec<_>, MachineError>>()?;
    let continuations = payload
        .call_stack
        .children()
        .iter()
        .map(|child| Continuation {
            output_offset: scalar(&child.continuation.output_offset),
            output_size: scalar(&child.continuation.output_size),
            ..child.continuation.clone()
        })
        .collect();
    let mut summary = PayloadSummary {
        frames,
        continuations,
        store: payload.store.summary_projection(domain)?,
        storage_observations,
        work: 0,
        location: payload.key(),
    };
    summary.work = summary.measure_work();
    Ok(summary)
}

pub(super) fn join(
    old: &PayloadSummary,
    next: &PayloadSummary,
    domain: Domain,
    widen: bool,
) -> Result<PayloadSummary, MachineError> {
    MachineError::check_cancelled()?;
    if old.frames.len() != next.frames.len() || old.continuations.len() != next.continuations.len()
    {
        return Err(MachineError::Shape);
    }
    let mut out = old.clone();
    out.store = old.store.join(&next.store, domain, widen);
    out.storage_observations =
        old.storage_observations
            .join(&next.storage_observations, domain, widen)?;
    for ((output, left), right) in out.frames.iter_mut().zip(&old.frames).zip(&next.frames) {
        if left.key != right.key
            || left.code != right.code
            || left.program != right.program
            || left.stack.len() != right.stack.len()
        {
            return Err(MachineError::Shape);
        }
        for ((output, left), right) in output.stack.iter_mut().zip(&left.stack).zip(&right.stack) {
            *output = join_scalar(left, right, domain, widen);
        }
        output.stack_aliases = left.stack_aliases.intersect(&right.stack_aliases)?;
        output.call_value = join_scalar(&left.call_value, &right.call_value, domain, widen);
        output.address_operand =
            join_scalar(&left.address_operand, &right.address_operand, domain, widen);
        output.caller_operand =
            join_scalar(&left.caller_operand, &right.caller_operand, domain, widen);
        output.memory = left.memory.join(&right.memory, domain, widen);
        output.calldata = left.calldata.join(&right.calldata, domain, widen);
        output.returndata = left.returndata.join(&right.returndata, domain, widen);
        output.saved_store = left.saved_store.join(&right.saved_store, domain, widen);
    }
    for ((output, left), right) in out
        .continuations
        .iter_mut()
        .zip(&old.continuations)
        .zip(&next.continuations)
    {
        if left.return_block != right.return_block || left.creation != right.creation {
            return Err(MachineError::Shape);
        }
        output.output_offset =
            join_scalar(&left.output_offset, &right.output_offset, domain, widen);
        output.output_size = join_scalar(&left.output_size, &right.output_size, domain, widen);
    }
    // The erased histories must not remain a permanent replay cost. The new
    // whole bound itself supplies the conservative construction/traversal size.
    out.work = out.measure_work();
    Ok(out)
}

pub(super) fn semantic_eq(left: &PayloadSummary, right: &PayloadSummary) -> bool {
    left.frames == right.frames
        && left.continuations == right.continuations
        && left.store == right.store
        && left.storage_observations == right.storage_observations
}

/// Summaries are not executable arrays. Each invocation gets independent source
/// variables; only its own retained stack classes share scalar expressions.
pub(super) fn instantiate(
    payload: &PayloadSummary,
    domain: Domain,
) -> Result<MachinePayload, MachineError> {
    let frames = payload
        .frames
        .iter()
        .map(|frame| {
            Ok(FrameState {
                key: frame.key.clone(),
                code: frame.code,
                program: frame.program.clone(),
                stack: frame
                    .stack_aliases
                    .instantiate(&frame.stack, domain.spec().relations().enabled)?,
                memory: frame.memory.instantiate(domain)?,
                calldata: frame.calldata.instantiate(domain)?,
                environment_calldata: frame.environment_calldata,
                returndata: frame.returndata.instantiate(domain)?,
                call_value: relations::identify(
                    frame.call_value.clone(),
                    domain.spec().relations().enabled,
                ),
                address_operand: Some(relations::identify(
                    frame.address_operand.clone(),
                    domain.spec().relations().enabled,
                )),
                caller_operand: Some(relations::identify(
                    frame.caller_operand.clone(),
                    domain.spec().relations().enabled,
                )),
                saved_store: Snapshot::from_state(frame.saved_store.instantiate(domain)?),
            })
        })
        .collect::<Result<Vec<_>, MachineError>>()?;
    let mut frames = frames.into_iter();
    let root = RootFrame {
        state: frames.next().expect("nonempty procedure stack"),
    };
    let children = frames
        .zip(&payload.continuations)
        .map(|(state, continuation)| ChildFrame {
            state,
            continuation: continuation.clone(),
        })
        .collect();
    let mut output = MachinePayload {
        call_stack: CallStack::from_parts(root, children),
        store: payload.store.instantiate(domain)?,
        relations: RelationState::default(),
    };
    payload.storage_observations.install(&mut output, domain)?;
    MachineError::check_cancelled()?;
    Ok(output)
}
