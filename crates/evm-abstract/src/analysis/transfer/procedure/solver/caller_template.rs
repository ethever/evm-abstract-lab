//! Consume a detached final caller snapshot; borrowed consumers still copy.

use crate::{
    analysis::{MachineError, state::EvmPayloadSummary},
    domain::WordElement,
    resource::{
        WorkBudget,
        clone_payment::{checkpoint, pay},
    },
};
use std::borrow::Cow;

#[cfg(test)]
mod tests;

pub(super) fn into_owned_with_payment<N: WordElement>(
    template: Cow<'_, EvmPayloadSummary<N>>,
    budget: &mut WorkBudget,
) -> Result<EvmPayloadSummary<N>, MachineError> {
    checkpoint(budget)?;
    match template {
        Cow::Borrowed(template) => template.clone_with_payment(budget),
        Cow::Owned(template) => {
            // The kernel read already paid for this complete immutable snapshot.
            // Only the cached shape header and ownership transfer happen here.
            pay(budget, 1)?;
            if template.frames.len().checked_sub(1) != Some(template.continuations.len()) {
                return Err(MachineError::Shape);
            }
            Ok(template)
        }
    }
}
