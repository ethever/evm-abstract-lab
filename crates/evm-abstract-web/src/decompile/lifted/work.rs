//! One bounded allowance for semantic preparation and a display batch.
use evm_abstract_protocol::decompile_model::MAX_LIFTED_RECORDS;
use std::cell::Cell;

pub(in crate::decompile) struct Work {
    remaining: Cell<u64>,
}

impl Work {
    pub fn new(requested: u64) -> Self {
        // The model's convenience renderer uses this ceiling too. An artifact
        // may request less work, but cannot remove the consumer's hard bound.
        let ceiling = u64::try_from(MAX_LIFTED_RECORDS)
            .unwrap_or(u64::MAX)
            .saturating_mul(256);
        Self {
            remaining: Cell::new(requested.min(ceiling)),
        }
    }

    pub fn reserve(&self, work: usize) -> bool {
        let work = u64::try_from(work).unwrap_or(u64::MAX);
        let Some(remaining) = self.remaining.get().checked_sub(work) else {
            return false;
        };
        self.remaining.set(remaining);
        true
    }
}
