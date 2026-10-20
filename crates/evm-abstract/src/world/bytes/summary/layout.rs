//! A loop retains a fixed set of byte coordinates, with unknown other cells.

use super::{COPY_FACT_LIMIT, Content, EvmByteArraySummary, EvmValue, ValueAlgebra};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};

impl<N: WordElement> EvmByteArraySummary<N> {
    /// Forget cell constraints and moving scalar products while retaining the
    /// complete logical length interval and memory interpretation.
    pub(crate) fn forget_loop_content(&self) -> Self {
        let (lower, upper) = crate::world::unsigned_bounds(&self.length);
        Self {
            length: EvmValue::<N>::unsigned_range(lower, upper)
                .expect("summary length bounds are ordered"),
            memory: self.memory,
            content: if upper == alloy_primitives::U256::ZERO {
                Content::<N>::Zero
            } else {
                Content::<N>::Cells(Default::default())
            },
        }
    }

    /// Keep length independently of content; omitted cells remain arbitrary
    /// bytes. Zero is retained only when the complete logical content is zero.
    pub(crate) fn project_to_layout_with<D: EvmNumericAlgebra<Element = N>>(
        &self,
        layout: &Self,
        _domain: AlgebraRef<'_, D>,
    ) -> Self {
        let keys = layout.content.keys();
        let cells = keys
            .into_iter()
            .take(COPY_FACT_LIMIT)
            .filter_map(|index| {
                let value = self.content.at(index);
                (value.numeric() != EvmValue::<N>::unknown_byte().numeric())
                    .then_some((index, value))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let zero_with = matches!(self.content, Content::<N>::Zero)
            || usize::try_from(crate::world::unsigned_bounds(&self.length).1)
                .ok()
                .filter(|upper| *upper <= cells.len())
                .is_some_and(|upper| {
                    (0..upper).all(|index| {
                        cells.get(&index).and_then(EvmValue::<N>::singleton)
                            == Some(alloy_primitives::U256::ZERO)
                    })
                });
        Self {
            length: self.length.clone(),
            memory: self.memory,
            content: if zero_with {
                Content::<N>::Zero
            } else {
                Content::<N>::Cells(cells)
            },
        }
    }
}

impl EvmByteArraySummary<crate::domain::NumericValue> {
    /// Default numeric-policy compatibility wrapper.
    pub(crate) fn project_to_layout(&self, layout: &Self, _domain: crate::domain::Domain) -> Self {
        self.project_to_layout_with(layout, ValueAlgebra::new(_domain).borrowed())
    }
}
