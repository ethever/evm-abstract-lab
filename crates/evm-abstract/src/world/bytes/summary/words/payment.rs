//! Whole-word predicates retain the exact shared read/expression semantics.

use super::super::super::numeric;
use super::super::replay::graph_work;
use super::{
    EvmByteArray, EvmValue, ExprId, ExprKind, RangeError, U256, WORDS, coordinates, insert, needed,
};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, summary},
    resource::{
        SummaryError, WorkBudget,
        clone_payment::{checkpoint, pay},
    },
};
use std::collections::BTreeMap;

pub(in super::super) fn project_with_payment<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    array: &EvmByteArray<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<BTreeMap<usize, EvmValue<N>>, SummaryError<RangeError>> {
    pay(budget, array.operands.len().saturating_add(3))?;
    if !needed(array) {
        return Ok(BTreeMap::new());
    }
    pay(
        budget,
        graph_work(
            array
                .root
                .work_size()
                .saturating_add(array.conditional.work_size())
                .saturating_add(array.operands.len()),
        )
        .saturating_mul(WORDS.saturating_add(2)),
    )?;
    let mut coordinates = coordinates(&array.root);
    let mut candidate = |expression: &ExprId| {
        if let ExprKind::WordRead { view, offset } = expression.kind()
            && let Some(index) = view
                .offset
                .as_constant()
                .zip(offset.as_constant())
                .and_then(|(base, relative)| base.checked_add(relative))
                .and_then(|index| usize::try_from(index).ok())
        {
            insert(&mut coordinates, index);
        }
    };
    for expression in array.operands.keys() {
        candidate(expression);
    }
    array
        .conditional
        .visit(&mut |expression, _| candidate(expression));
    let mut out = BTreeMap::new();
    for index in coordinates {
        checkpoint(budget)?;
        // The old entry is read_word_checked with an ordinary constant operand,
        // including Constant provenance in its remembered offset fact.
        pay(
            budget,
            domain
                .literal_quote_work(U256::from(index))
                .saturating_add(16),
        )?;
        let at = EvmValue::constant(U256::from(index));
        checkpoint(budget)?;
        let value = numeric::read_word_with_payment(array, &at, domain, budget)?;
        if value.numeric().requires_retained_binding()
            && !summary::is_top(&value, domain, budget)?
            && summary::singleton(&value, budget)?.is_none()
        {
            let value = summary::scalar(&value, domain, budget)?;
            pay(budget, graph_work(out.len()))?;
            out.insert(index, value);
        }
    }
    checkpoint(budget)?;
    Ok(out)
}

pub(in super::super) fn instantiate_with_payment<
    N: WordElement,
    D: EvmNumericAlgebra<Element = N>,
>(
    words: &BTreeMap<usize, EvmValue<N>>,
    out: &mut EvmByteArray<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<(), SummaryError<RangeError>> {
    pay(budget, words.len().saturating_add(1))?;
    for (index, value) in words {
        let expression = numeric::word_expression_with_payment(
            out,
            &ExprId::constant(U256::from(*index)),
            domain,
            budget,
        )?;
        let value = summary::clone_value(value, budget)?.with_expression(expression.clone());
        pay(
            budget,
            graph_work(out.operands.len().saturating_add(expression.work_size())),
        )?;
        out.operands.insert(expression, value);
    }
    checkpoint(budget)?;
    Ok(())
}
