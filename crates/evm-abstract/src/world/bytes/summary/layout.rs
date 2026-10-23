//! A loop retains a fixed set of byte coordinates, with unknown other cells.

use super::{COPY_FACT_LIMIT, Content, EvmByteArraySummary, EvmValue};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::{
    domain::summary,
    resource::{
        CloneError, WorkBudget,
        clone_payment::{checkpoint, pay},
    },
};
use std::collections::BTreeMap;

use super::payment::{coordinate_work, numeric_equal};

impl<N: WordElement> EvmByteArraySummary<N> {
    /// Forget cell constraints and moving scalar products while retaining the
    /// complete logical length interval and memory interpretation.
    pub(crate) fn forget_loop_content(&self) -> Self {
        let (lower, upper) = crate::world::unsigned_bounds(&self.length);
        Self {
            length: EvmValue::<N>::unsigned_range(lower, upper)
                .expect("summary length bounds are ordered"),
            memory: self.memory,
            words: Default::default(),
            content: if upper == alloy_primitives::U256::ZERO {
                Content::<N>::Zero
            } else {
                Content::<N>::Cells(Default::default())
            },
        }
    }

    /// Quote the actual length query and range constructor independently.
    pub(crate) fn forget_loop_content_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay(budget, 8)?;
        let (lower, upper) = summary::unsigned_bounds(&self.length, budget)?;
        let length = summary::unsigned_range(lower, upper, domain, budget)?;
        checkpoint(budget)?;
        Ok(Self {
            length,
            memory: self.memory,
            words: BTreeMap::new(),
            content: if upper == alloy_primitives::U256::ZERO {
                Content::Zero
            } else {
                Content::Cells(BTreeMap::new())
            },
        })
    }

    /// Keep length independently of content; omitted cells remain arbitrary
    /// bytes. Zero is retained only when the complete logical content is zero.
    pub(crate) fn project_to_layout<D: EvmNumericAlgebra<Element = N>>(
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
        let zero = matches!(self.content, Content::<N>::Zero)
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
            words: self
                .words
                .iter()
                .filter(|(index, _)| layout.words.contains_key(index))
                .map(|(index, value)| (*index, value.clone()))
                .collect(),
            content: if zero {
                Content::<N>::Zero
            } else {
                Content::<N>::Cells(cells)
            },
        }
    }

    /// Freeze the same coordinates as pure projection, paying predicates on
    /// constructed/copied cells separately from those construction/copy fees.
    pub(crate) fn project_to_layout_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        layout: &Self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Self, CloneError> {
        pay(budget, 8)?;
        let keys = layout.content.keys_with_payment(budget)?;
        let mut cells = BTreeMap::new();
        for index in keys.into_iter().take(COPY_FACT_LIMIT) {
            let value = self.content.at_with_payment(index, domain, budget)?;
            let unknown = summary::unknown_byte(domain, budget)?;
            if !numeric_equal(&value, &unknown, domain, budget)? {
                pay(budget, coordinate_work(cells.len()).saturating_add(2))?;
                cells.insert(index, value);
            }
        }
        let mut zero = matches!(self.content, Content::Zero);
        if !zero {
            let (_, upper) = summary::unsigned_bounds(&self.length, budget)?;
            if let Ok(upper) = usize::try_from(upper)
                && upper <= cells.len()
            {
                zero = true;
                for index in 0..upper {
                    pay(budget, coordinate_work(cells.len()))?;
                    let Some(value) = cells.get(&index) else {
                        zero = false;
                        break;
                    };
                    if summary::singleton(value, budget)? != Some(alloy_primitives::U256::ZERO) {
                        zero = false;
                        break;
                    }
                }
            }
        }
        let length = summary::clone_value(&self.length, budget)?;
        pay(budget, self.words.len().saturating_add(1))?;
        let mut words = BTreeMap::new();
        for (index, value) in &self.words {
            pay(budget, coordinate_work(layout.words.len()))?;
            if layout.words.contains_key(index) {
                let value = summary::clone_value(value, budget)?;
                pay(budget, coordinate_work(words.len()).saturating_add(2))?;
                words.insert(*index, value);
            }
        }
        checkpoint(budget)?;
        Ok(Self {
            length,
            memory: self.memory,
            words,
            content: if zero {
                Content::Zero
            } else {
                Content::Cells(cells)
            },
        })
    }
}
