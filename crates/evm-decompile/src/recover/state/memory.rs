//! May-write envelopes preserve only observations outside every possible write.
//! Bounds describe the actual unsigned offset word, never unwrapped arithmetic.
use super::{AbstractValue, Address, Memory, U256, cost};

impl Memory {
    /// Structural quote for an additional write and its changed interval list.
    /// Value/key ownership and weak joins are reserved separately by recovery.
    pub fn write_work(&self) -> usize {
        self.work_size()
            .saturating_mul(cost::tree_work(self.words.len().saturating_add(1)).saturating_add(1))
            .saturating_mul(8)
    }

    /// A range summarizes possible starts; it does not strongly update every
    /// point in the hull. Only a trusted exact address can create a new word.
    /// Unknown widths or overflowing endpoints provide no upper exclusion.
    pub fn write_envelope(
        &mut self,
        address: Option<Address>,
        bounds: (U256, U256),
        width: Option<u64>,
        value: Option<AbstractValue>,
    ) {
        if width == Some(0) {
            return;
        }
        let (lower, upper) = bounds;
        let end = width.and_then(|size| upper.checked_add(U256::from(size)));
        self.words.retain(|key, _| {
            let Address::Exact(start) = key else {
                return false;
            };
            start
                .checked_add(U256::from(32))
                .is_some_and(|old_end| old_end <= lower)
                || end.is_some_and(|end| end <= *start)
        });
        if let Some((start, end)) = u64::try_from(lower)
            .ok()
            .zip(end.and_then(|end| u64::try_from(end).ok()))
        {
            self.mark_written(start, end);
        } else {
            self.zero = false;
            self.written.clear();
        }
        if width == Some(32)
            && let (Some(address), Some(value)) = (address, value)
        {
            self.words.insert(address, value);
        }
    }
}

#[cfg(test)]
mod tests;
