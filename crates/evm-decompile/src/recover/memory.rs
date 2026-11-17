//! Paid memory writes retain finite alternatives before losing their correlation.
//! Each concrete alternative starts from the same input snapshot. Their merge
//! is a weak update, not a sequence of strong writes to every possible address.
use super::{
    Recovery,
    state::{AbstractValue, Address, Memory},
};
use crate::{Progress, U256};
use analysis_kernel::cost;

impl<P: FnMut(Progress)> Recovery<'_, P> {
    /// The instruction reservation funds one query/write/value clone. Finite
    /// alternatives and merge factories have separate reservations below.
    pub(super) fn write_memory(
        &mut self,
        memory: &mut Memory,
        offset: &AbstractValue,
        width: Option<u64>,
        value: Option<&AbstractValue>,
    ) -> Option<()> {
        if width == Some(0) {
            return self.owner.pay(0).then_some(());
        }
        let address = Address::of(offset);
        if let (Some(Address::Exact(_)), Some(width)) = (&address, width) {
            memory.write(address, width, value.cloned());
            return self.owner.pay(0).then_some(());
        }
        let Some(candidates) = offset.constants().filter(|set| !set.is_empty()) else {
            if !self.owner.pay(memory.write_work()) {
                return None;
            }
            memory.write_envelope(
                address,
                offset.interval().unsigned_bounds(),
                width,
                value.cloned(),
            );
            return self.owner.pay(0).then_some(());
        };
        let count = candidates.len();
        let clones = memory.work_size().saturating_mul(4).saturating_mul(count);
        let additional = count.saturating_sub(1).saturating_mul(
            memory
                .write_work()
                .saturating_add(value.map_or(0, AbstractValue::clone_work)),
        );
        if !self
            .owner
            .pay(count.saturating_add(clones).saturating_add(additional))
        {
            return None;
        }
        let mut joined: Option<Memory> = None;
        for candidate in candidates {
            if !self.owner.pay(0) {
                return None;
            }
            let mut branch = memory.clone();
            let exact = Some(Address::Exact(*candidate));
            if let Some(width) = width {
                branch.write(exact, width, value.cloned());
            } else {
                branch.write_envelope(exact, (*candidate, *candidate), None, None);
            }
            joined = Some(if let Some(mut previous) = joined {
                let factories = previous
                    .words
                    .len()
                    .saturating_add(branch.words.len())
                    .saturating_mul(2)
                    .saturating_mul(self.algebra.literal_quote_work(U256::ZERO));
                if !self
                    .owner
                    .pay(previous.merge_work(&branch).saturating_add(factories))
                {
                    return None;
                }
                previous.merge(&branch, |left, right| self.join_value(left, right, false))?;
                previous
            } else {
                branch
            });
        }
        let mut joined = joined?;
        // Even when its numeric cover has several addresses, a trusted symbolic
        // store is exactly known to a later load of that same immutable address.
        if width == Some(32)
            && let (Some(address @ Address::Expression(_)), Some(value)) = (address, value)
        {
            if !self.owner.pay(
                cost::key_insert_work(joined.words.len(), offset.work_size(), 0)
                    .saturating_add(value.clone_work()),
            ) {
                return None;
            }
            joined.words.insert(address, value.clone());
        }
        if !self.owner.pay(0) {
            return None;
        }
        *memory = joined;
        Some(())
    }
}
