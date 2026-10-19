//! Canonical numerical bounds, finite stack aliases and independent instances.

use super::{
    AbstractValue, ByteArray, CallStack, ChildFrame, Continuation, Domain, Entry, FrameByteKind,
    FrameCode, FrameMapOrder, FrameSummary, MachineError, MachinePayload, NumericValue,
    PayloadSummary, RelationState, RootFrame, Snapshot, identify, stack_alias::StackAliases,
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
    pub(in crate::analysis) fn active(&self) -> &FrameSummary {
        self.frames.last().expect("nonempty relative stack")
    }
    pub(in crate::analysis) fn work_size(&self) -> usize {
        self.work
    }

    pub(in crate::analysis) fn measure_work(&self) -> usize {
        let store_work = self.store.work_size();
        let observation_work = self
            .storage_observations
            .work_size(&self.active().stack, &self.active().stack_aliases);
        let frames = self.frames.iter().fold(
            store_work.saturating_add(observation_work),
            |work, frame| {
                frame.values().fold(
                    frame
                        .byte_arrays()
                        .into_iter()
                        .fold(work, |work, bytes| work.saturating_add(bytes.work_size()))
                        .saturating_add(frame.saved_store.work_size())
                        .saturating_add(frame.stack_aliases.work_size())
                        .saturating_add(frame.metadata_work())
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
pub(in crate::analysis) fn alias_form_work(payload: &MachinePayload) -> usize {
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

pub(in crate::analysis) fn form(
    payload: &MachinePayload,
    entry: &Entry,
    domain: Domain,
) -> Result<PayloadSummary, MachineError> {
    MachineError::check_cancelled()?;
    let storage_observations =
        super::storage_observations::StorageObservations::form(payload, domain)?;
    let frames = payload
        .call_stack
        .iter()
        .map(|frame| {
            let unknown = frame.code == FrameCode::Unknown;
            let history = if unknown {
                Vec::new()
            } else {
                super::context::canonical(&frame.key.jump_history)?
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
            let mut summary = frame.try_zip(
                frame,
                stack,
                FrameMapOrder::BytesFirst,
                |value, _| Ok(scalar(value)),
                |value, _, implicit| {
                    Ok(scalar(&value.clone().unwrap_or_else(|| {
                        entry.environment.address_value(implicit)
                    })))
                },
                |bytes, _, kind| {
                    Ok(if unknown {
                        match kind {
                            FrameByteKind::Memory => ByteArray::memory(),
                            _ => ByteArray::empty(),
                        }
                        .summary_projection(domain)?
                    } else {
                        bytes.summary_projection(domain)?
                    })
                },
                |saved, _| Ok(saved.state().summary_projection(domain)?),
            )?;
            summary.key.jump_history = history;
            summary.key.stack_height = summary.stack.len();
            if unknown {
                summary.key.basic_block_index = 0;
            }
            Ok(FrameSummary {
                frame: summary,
                stack_aliases,
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
    MachineError::check_cancelled()?;
    Ok(summary)
}

pub(in crate::analysis) fn join(
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
    let store = old.store.join(&next.store, domain, widen);
    let storage_observations =
        old.storage_observations
            .join(&next.storage_observations, domain, widen)?;
    let mut frames = Vec::with_capacity(old.frames.len());
    for (left, right) in old.frames.iter().zip(&next.frames) {
        if !left.same_shape(right) {
            return Err(MachineError::Shape);
        }
        let stack = left
            .stack
            .iter()
            .zip(&right.stack)
            .map(|(left, right)| join_scalar(left, right, domain, widen))
            .collect();
        let stack_aliases = left.stack_aliases.intersect(&right.stack_aliases)?;
        frames.push(FrameSummary {
            frame: left.try_zip(
                right,
                stack,
                FrameMapOrder::ScalarsFirst,
                |left, right| Ok(join_scalar(left, right, domain, widen)),
                |left, right, _| Ok(join_scalar(left, right, domain, widen)),
                |left, right, _| Ok(left.join(right, domain, widen)),
                |left, right| Ok(left.join(right, domain, widen)),
            )?,
            stack_aliases,
        });
    }
    let mut continuations = old.continuations.clone();
    for ((output, left), right) in continuations
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
    let mut out = PayloadSummary {
        frames,
        continuations,
        store,
        storage_observations,
        work: 0,
        location: old.location.clone(),
    };
    // The erased histories must not remain a permanent replay cost. The new
    // whole bound itself supplies the conservative construction/traversal size.
    out.work = out.measure_work();
    MachineError::check_cancelled()?;
    Ok(out)
}

pub(in crate::analysis) fn semantic_eq(left: &PayloadSummary, right: &PayloadSummary) -> bool {
    left.frames == right.frames
        && left.continuations == right.continuations
        && left.store == right.store
        && left.storage_observations == right.storage_observations
}

/// Summaries are not executable arrays. Each invocation gets independent source
/// variables; only its own retained stack classes share scalar expressions.
pub(in crate::analysis) fn instantiate(
    payload: &PayloadSummary,
    domain: Domain,
) -> Result<MachinePayload, MachineError> {
    let frames = payload
        .frames
        .iter()
        .map(|frame| {
            let enabled = domain.spec().relations().enabled;
            frame.try_zip(
                frame,
                frame.stack_aliases.instantiate(&frame.stack, enabled)?,
                FrameMapOrder::BytesFirst,
                |value, _| Ok(identify(value.clone(), enabled)),
                |value, _, _| Ok(Some(identify(value.clone(), enabled))),
                |bytes, _, _| Ok(bytes.instantiate(domain)?),
                |saved, _| Ok(Snapshot::from_state(saved.instantiate(domain)?)),
            )
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
    payload
        .storage_observations
        .install(&mut output, &payload.active().stack_aliases, domain)?;
    MachineError::check_cancelled()?;
    Ok(output)
}
