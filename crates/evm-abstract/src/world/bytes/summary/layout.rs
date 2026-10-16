//! A loop retains a fixed set of byte coordinates, with unknown other cells.

use super::{AbstractValue, ByteArraySummary, COPY_FACT_LIMIT, Content, Domain};

impl ByteArraySummary {
    /// Forget cell constraints and moving scalar products while retaining the
    /// complete logical length interval and memory interpretation.
    pub(crate) fn forget_loop_content(&self) -> Self {
        let (lower, upper) = self.length.interval().unsigned_bounds();
        Self {
            length: AbstractValue::unsigned_range(lower, upper)
                .expect("summary length bounds are ordered"),
            memory: self.memory,
            content: if upper == alloy_primitives::U256::ZERO {
                Content::Zero
            } else {
                Content::Cells(Default::default())
            },
        }
    }

    /// Keep length independently of content; omitted cells remain arbitrary
    /// bytes. Zero is retained only when the complete logical content is zero.
    pub(crate) fn project_to_layout(&self, layout: &Self, _domain: Domain) -> Self {
        let keys = layout.content.keys();
        let cells = keys
            .into_iter()
            .take(COPY_FACT_LIMIT)
            .filter_map(|index| {
                let value = self.content.at(index);
                (value.numeric() != AbstractValue::unknown_byte().numeric())
                    .then_some((index, value))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let zero = matches!(self.content, Content::Zero)
            || usize::try_from(self.length.interval().unsigned_bounds().1)
                .ok()
                .filter(|upper| *upper <= cells.len())
                .is_some_and(|upper| {
                    (0..upper).all(|index| {
                        cells.get(&index).and_then(AbstractValue::singleton)
                            == Some(alloy_primitives::U256::ZERO)
                    })
                });
        Self {
            length: self.length.clone(),
            memory: self.memory,
            content: if zero {
                Content::Zero
            } else {
                Content::Cells(cells)
            },
        }
    }
}
