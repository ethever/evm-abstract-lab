//! A paid undo record for optional histories, without copying numerical data.

use crate::{
    analysis::{FrameKey, MachineError, PolicyContext},
    domain::EvmNumericAlgebra,
};

pub(super) struct HistoryRollback {
    saved: Vec<(usize, Vec<usize>)>,
    changes: usize,
}

impl HistoryRollback {
    /// The caller has paid the first metadata census. Reserve the second
    /// selection pass, allocation, moves, restoration scans and either outcome's
    /// disposal before taking any history. Refusal needs no further payment.
    pub(super) fn reserve<'a, D: EvmNumericAlgebra>(
        frames: impl Iterator<Item = &'a FrameKey>,
        changes: impl Fn(&FrameKey) -> bool,
        selection_work: usize,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        let mut count = 0usize;
        let mut words = 0usize;
        let mut frame_count = 0usize;
        for frame in frames {
            frame_count = frame_count.saturating_add(1);
            if changes(frame) {
                count = count.saturating_add(1);
                words = words.saturating_add(frame.jump_history.len());
            }
        }
        if count != 0 {
            let record_words =
                std::mem::size_of::<(usize, Vec<usize>)>().div_ceil(std::mem::size_of::<usize>());
            cx.reserve(
                selection_work
                    .saturating_add(frame_count.saturating_mul(4))
                    .saturating_add(count.saturating_mul(record_words).saturating_mul(4))
                    .saturating_add(words.saturating_mul(2))
                    .saturating_add(8),
            )?;
        }
        Ok(Self {
            saved: Vec::with_capacity(count),
            changes: count,
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.changes == 0
    }

    pub(super) fn take<'a>(
        &mut self,
        frames: impl Iterator<Item = &'a mut FrameKey>,
        changes: impl Fn(&FrameKey) -> bool,
    ) {
        for (index, frame) in frames.enumerate() {
            if changes(frame) {
                self.saved
                    .push((index, std::mem::take(&mut frame.jump_history)));
            }
        }
        debug_assert_eq!(self.saved.len(), self.changes);
    }

    pub(super) fn removed_before(&self, frames: usize) -> usize {
        self.saved
            .iter()
            .take_while(|(index, _)| *index < frames)
            .fold(0usize, |work, (_, history)| {
                work.saturating_add(history.len())
            })
    }

    pub(super) fn restore<'a>(self, frames: impl Iterator<Item = &'a mut FrameKey>) {
        let mut saved = self.saved.into_iter().peekable();
        for (index, frame) in frames.enumerate() {
            if saved
                .peek()
                .is_some_and(|(saved_index, _)| *saved_index == index)
            {
                frame.jump_history = saved.next().expect("matching history").1;
            }
        }
        debug_assert!(
            saved.next().is_none(),
            "inner refusal must preserve frame shape"
        );
    }
}
