//! Project the current root through the shared paid Reader, not cached cells.

use super::super::numeric;
use super::{
    COPY_FACT_LIMIT, Content, EvmByteArray, EvmByteArraySummary, ExprId, RangeError, U256,
    payment::numeric_equal, replay::graph_work,
};
use crate::{
    domain::{
        AlgebraRef, EvmNumericAlgebra, WordElement, summary,
        symbolic::{ByteStateKind, ByteView, ExprLimits},
    },
    resource::{
        SummaryError, WorkBudget,
        clone_payment::{checkpoint, pay},
    },
};
use std::{collections::BTreeMap, sync::Arc};

type Error = SummaryError<RangeError>;

impl<N: WordElement> EvmByteArray<N> {
    pub(crate) fn summary_projection_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<EvmByteArraySummary<N>, Error> {
        pay(budget, 8)?;
        if let Some(error) = &self.failure {
            return Err(SummaryError::Source(error.clone()));
        }
        let content = match self.root.kind() {
            ByteStateKind::Zero => Content::Zero,
            ByteStateKind::Exact(bytes) => {
                pay(budget, graph_work(bytes.len()))?;
                Content::exact(Arc::clone(bytes))
            }
            _ => {
                let length = summary::singleton(&self.length, budget)?;
                if let Some(exact) = self.exact_projection_with_payment(length, domain, budget)? {
                    pay(budget, graph_work(exact.len()))?;
                    Content::exact(Arc::from(exact))
                } else {
                    let upper = match length {
                        Some(length) => length,
                        None => {
                            pay(budget, self.length.numeric().work_size().saturating_add(16))?;
                            self.length.interval().unsigned_bounds().1
                        }
                    };
                    let upper = usize::try_from(upper)
                        .ok()
                        .filter(|upper| *upper <= COPY_FACT_LIMIT);
                    // An unbounded source contributes only retained positions;
                    // it never constructs the full bounded candidate window.
                    pay(
                        budget,
                        graph_work(self.bytes.len().saturating_add(upper.unwrap_or(0))),
                    )?;
                    let mut keys = self.bytes.keys().copied().collect::<Vec<_>>();
                    if let Some(upper) = upper {
                        keys.extend(0..upper);
                    }
                    keys.sort_unstable();
                    keys.dedup();
                    let mut cells = BTreeMap::new();
                    let mut reader = numeric::Reader::with_payment(
                        &self.operands,
                        Some(&self.conditional),
                        domain,
                        budget,
                    )?;
                    for index in keys.into_iter().take(COPY_FACT_LIMIT) {
                        pay(budget, 8)?;
                        let value = reader.read_with_payment(
                            &self.root,
                            &ExprId::constant(U256::from(index)),
                            budget,
                        )?;
                        let value = match self.bytes.get(&index) {
                            Some(previous) => summary::restore(&value, previous, domain, budget)?,
                            None => value,
                        };
                        let value = summary::scalar(&value, domain, budget)?;
                        let unknown = summary::unknown_byte(domain, budget)?;
                        if !numeric_equal(&value, &unknown, domain, budget)? {
                            pay(budget, graph_work(cells.len()))?;
                            cells.insert(index, value);
                        }
                    }
                    Content::Cells(cells)
                }
            }
        };
        checkpoint(budget)?;
        let length = summary::scalar(&self.length, domain, budget)?;
        let words = super::words::project_with_payment(self, domain, budget)?;
        checkpoint(budget)?;
        Ok(EvmByteArraySummary {
            length,
            memory: self.memory,
            content,
            words,
        })
    }

    /// Same bounded exact-materialization attempt as exact_bytes_bounded. Pure
    /// symbolic absence/limits select the original cell path; a paid refusal or
    /// cancellation propagates instead of fabricating an unknown native value.
    fn exact_projection_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        length: Option<U256>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Option<Vec<u8>>, Error> {
        let Some(length) = length
            .and_then(|length| usize::try_from(length).ok())
            .filter(|length| *length <= COPY_FACT_LIMIT)
        else {
            return Ok(None);
        };
        pay(
            budget,
            graph_work(self.root.work_size().saturating_add(2))
                .saturating_mul(length.saturating_add(1)),
        )?;
        let view = ByteView::new(
            self.root.clone(),
            ExprId::constant(U256::ZERO),
            ExprId::constant(U256::from(length)),
        );
        let materialize_work = length.saturating_mul(self.root.nodes().saturating_add(2));
        let materialize_limits = ExprLimits {
            max_nodes: materialize_work.max(view.nodes()),
            max_depth: self.root.depth().saturating_add(2),
        };
        match view.known_bytes(materialize_limits) {
            Ok(Some(bytes)) => return Ok(Some(bytes)),
            Err(_) => {
                checkpoint(budget)?;
                return Ok(None);
            }
            Ok(None) => {}
        }
        let mut output = Vec::new();
        if output.try_reserve_exact(length).is_err() {
            return Ok(None);
        }
        let mut reader =
            numeric::Reader::with_payment(&self.operands, Some(&self.conditional), domain, budget)?;
        for index in 0..length {
            pay(budget, 8)?;
            let value = reader.read_with_payment(
                &self.root,
                &ExprId::constant(U256::from(index)),
                budget,
            )?;
            let Some(byte) = summary::singleton(&value, budget)? else {
                return Ok(None);
            };
            let Ok(byte) = u8::try_from(byte) else {
                return Ok(None);
            };
            output.push(byte);
        }
        checkpoint(budget)?;
        Ok(Some(output))
    }
}
