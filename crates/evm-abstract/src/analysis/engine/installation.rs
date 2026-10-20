//! Preparation and replacement own different work at the publication boundary.

use super::{EvmEntryAlternatives, EvmMachinePayload};
use crate::domain::{WordElement, symbolic::ExprId};
use std::{collections::BTreeSet, mem::size_of};

pub(super) struct JoinWork {
    pub preparation: usize,
    pub installation: usize,
}

impl JoinWork {
    pub fn quote<N: WordElement>(
        entry: usize,
        incoming: usize,
        joined: usize,
        prepared_alternatives: usize,
        old_alternatives: usize,
        selectors: usize,
    ) -> Self {
        // The prepared values cross two closure boundaries (with_run and then
        // publish_prepared_with) BEFORE the kernel's reservation. Prepay both
        // transports here; final destination moves belong to installation.
        // Arbitrary N may own large inline fields as well as deep allocations.
        let bytes = size_of::<Option<EvmMachinePayload<N>>>()
            .saturating_add(size_of::<EvmEntryAlternatives<N>>())
            .saturating_add(size_of::<BTreeSet<ExprId>>());
        let transport = if bytes == usize::MAX {
            usize::MAX
        } else {
            bytes.div_ceil(size_of::<usize>())
        };
        let preparation = entry
            .saturating_add(incoming)
            .saturating_add(joined)
            .saturating_add(prepared_alternatives)
            .saturating_add(selectors)
            .saturating_mul(4)
            .saturating_add(transport.saturating_mul(2));
        // Assignment releases OLD live data, not the newly prepared payloads.
        // Their retained-size bounds include deep N and potentially sole Arc
        // histories. Promotion may shrink prepared alternatives independently.
        // Selector keys are Fresh leaves: one key release and one tree entry.
        // invalidate_execution retains the evidence and only clears its flag.
        let installation = entry
            .saturating_add(old_alternatives)
            .saturating_add(selectors.saturating_mul(2))
            .saturating_add(transport)
            .saturating_add(1);
        Self {
            preparation,
            installation,
        }
    }
}
