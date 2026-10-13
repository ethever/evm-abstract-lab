//! Scalar widening applies only inside an otherwise invariant whole payload.
//! Root/source identities, byte views, rollback checkpoints and continuations
//! must remain identical. Changed stack words become numeric abstractions;
//! the resulting path contains only common guarantees, so no lost scalar
//! correlation is presented as a retained guarded disjunction.

use super::{MachineError, checkpoint};
use crate::{
    analysis::MachinePayload,
    domain::{Domain, NumericValue},
};

pub(super) fn stack(
    previous: &MachinePayload,
    incoming: &MachinePayload,
    domain: Domain,
) -> Result<Option<MachinePayload>, MachineError> {
    checkpoint()?;
    let mut old_shape = previous.content_form()?;
    let mut new_shape = incoming.content_form()?;
    for frame in old_shape.call_stack.iter_mut() {
        frame.stack.clear();
    }
    for frame in new_shape.call_stack.iter_mut() {
        frame.stack.clear();
    }
    old_shape.relations.forget();
    new_shape.relations.forget();
    if old_shape != new_shape {
        return Ok(None);
    }

    let mut result = previous.clone();
    for ((output, old), new) in result
        .call_stack
        .iter_mut()
        .zip(previous.call_stack.iter())
        .zip(incoming.call_stack.iter())
    {
        checkpoint()?;
        for ((output, old), new) in output.stack.iter_mut().zip(&old.stack).zip(&new.stack) {
            let joined = domain.join(old, new);
            let mut widened = domain.widen(old, &joined);
            if joined.numeric() != old.numeric() {
                // Finite sets and bit masks can ascend one bit at a time for a
                // Word256 counter. At this repeated-growth boundary retain the
                // widened unsigned interval and discard those scalar-only
                // components. This is a finite-height numerical upper bound;
                // unchanged words retain all their original product facts.
                let (lower, upper) = widened.interval().unsigned_bounds();
                widened = widened.with_numeric(
                    NumericValue::unsigned_range(lower, upper)
                        .expect("widening preserves ordered unsigned bounds"),
                );
            }
            *output = widened;
        }
    }
    result.relations = previous.relations.join(&incoming.relations);
    checkpoint()?;
    Ok(Some(result))
}
