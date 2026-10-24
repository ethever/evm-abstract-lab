//! The constructor shortcut and shared Reader use the same native paid facts.

use super::super::replay::graph_work;
use super::{ByteStateKind, EvmByteArray, EvmValue, ExprId, ExprKind, RangeError, U256, numeric};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, summary},
    resource::{
        SummaryError, WorkBudget,
        clone_payment::{checkpoint, pay},
    },
};

pub(in super::super) fn refresh_with_payment<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    out: &mut EvmByteArray<N>,
    bare_cells: bool,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<(), SummaryError<RangeError>> {
    pay(
        budget,
        graph_work(out.bytes.len().saturating_add(out.operands.len())),
    )?;
    let input = if bare_cells
        && !out.bytes.is_empty()
        && let ByteStateKind::Input {
            length, bindings, ..
        } = out.root.kind()
        && bindings.len() == out.bytes.len()
    {
        let length = numeric::scalar_with_payment(
            length,
            &out.operands,
            numeric::numeric_domain(domain),
            budget,
        )?;
        Some((bindings, summary::unsigned_bounds(&length, budget)?))
    } else {
        None
    };
    // The pure refresh uses numeric::read with no conditional environment.
    let mut reader = None;
    pay(budget, out.bytes.len().saturating_mul(8))?;
    let previous = std::mem::take(&mut out.bytes);
    let mut refreshed = Vec::with_capacity(previous.len());
    for (index, previous) in previous {
        checkpoint(budget)?;
        let at = U256::from(index);
        let mut direct = None;
        if let Some((bindings, (lower, upper))) = input
            && !(lower <= at && at < upper)
        {
            pay(budget, graph_work(bindings.len()))?;
            if let Ok(position) = bindings.binary_search_by_key(&at, |binding| binding.index) {
                let binding = &bindings[position];
                if matches!(binding.value.kind(), ExprKind::Fresh(_))
                    && binding.numeric.interval().unsigned_bounds().1 <= U256::from(255)
                {
                    direct = Some(if at >= upper {
                        summary::constant(U256::ZERO, domain, budget)?
                    } else if let Some(value) = out.operands.get(&binding.value) {
                        checkpoint(budget)?;
                        let work = value.numeric().import_binding_work(&binding.numeric);
                        pay(budget, work.saturating_add(16))?;
                        let imported = EvmValue::from_numeric(
                            value.numeric().import_binding(&binding.numeric),
                        );
                        checkpoint(budget)?;
                        imported
                    } else {
                        summary::from_observation(&binding.numeric, domain, budget)?
                    });
                }
            }
        }
        let value = match direct {
            Some(value) => value,
            None => {
                if reader.is_none() {
                    reader = Some(numeric::Reader::with_payment(
                        &out.operands,
                        None,
                        domain,
                        budget,
                    )?);
                }
                reader
                    .as_mut()
                    .expect("initialized Reader")
                    .read_with_payment(&out.root, &ExprId::constant(at), budget)?
            }
        };
        let value = summary::restore(&value, &previous, domain, budget)?;
        let value = summary::scalar(&value, domain, budget)?;
        refreshed.push((index, value));
    }
    // Original native reads/restoration retain their own fees. Rebuild the
    // staged presentation once from its sorted owned values, without N copies.
    pay(budget, graph_work(refreshed.len()))?;
    out.bytes = refreshed.into_iter().collect();
    checkpoint(budget)?;
    Ok(())
}
