//! Paid whole-word glue around the shared read plans and numeric evaluator.

use super::payment::{ReadPayment, ReadResult};
use super::{Context, EvmNumericAlgebra, EvmValue, Facts, Reader, WordElement};
use crate::{
    domain::{
        AlgebraRef,
        symbolic::{
            ByteStateId, ByteView, ExprError, ExprId, ExprLimits, SlotStateId, SourceId,
            StateBinding, StateError,
        },
    },
    resource::{SummaryError, WorkBudget},
    world::bytes::{EvmByteArray, check_value, limits, ranges, reads},
};
use alloy_primitives::U256;
use std::collections::BTreeMap;

fn source<E: Into<super::RangeError>>(error: E) -> SummaryError<super::RangeError> {
    SummaryError::Source(error.into())
}

pub(in crate::world) fn word_expression_with_payment<
    N: WordElement,
    D: EvmNumericAlgebra<Element = N>,
>(
    array: &EvmByteArray<N>,
    offset: &ExprId,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> ReadResult<ExprId> {
    if let Some(error) = &array.failure {
        return Err(SummaryError::Source(error.clone()));
    }
    let context = Context::with_payment(&array.operands, Some(&array.conditional), domain, budget)?;
    expression(
        &array.root,
        (!array.memory).then_some(&array.length),
        offset,
        &context,
        domain,
        &mut ReadPayment::new(budget),
    )
}

fn expression<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    root: &ByteStateId,
    length: Option<&EvmValue<N>>,
    offset: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<ExprId> {
    payer.context(context)?;
    payer.reserve(
        root.work_size()
            .saturating_add(offset.work_size())
            .saturating_add(length.map_or(0, EvmValue::work_size))
            .saturating_add(16)
            .saturating_mul(32),
    )?;
    let view = if let Some(length) = length {
        // Match symbolic_expression: discover singleton first, then use the
        // retained expression. No pure accessor executes an unpaid query.
        let length = if let Some(word) = payer.singleton(length)? {
            ExprId::constant(word)
        } else {
            length
                .expression()
                .cloned()
                .expect("byte length has an expression or singleton")
        };
        ByteView::new(root.clone(), ExprId::constant(U256::ZERO), length)
            .slice(
                offset.clone(),
                ExprId::constant(U256::from(32)),
                limits(domain).into(),
            )
            .map_err(source)?
    } else {
        ByteView::new(
            root.clone(),
            offset.clone(),
            ExprId::constant(U256::from(32)),
        )
    };
    reads::word_expression_paid(
        view,
        ExprId::constant(U256::ZERO),
        context,
        domain,
        limits(domain),
        payer,
    )
}

fn offset_expression<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    value: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<ExprId> {
    payer.reserve(value.work_size().saturating_add(16))?;
    let known = ranges::known_singleton(value);
    if value.expression().is_none()
        && known.is_none()
        && let Some(word) = payer.singleton(value)?
    {
        return Ok(ExprId::constant(word));
    }
    if let Some(expression) = value.expression() {
        return Ok(expression.clone());
    }
    if let Some(word) = known {
        return Ok(ExprId::constant(word));
    }
    let fresh = ExprId::fresh().ok_or_else(|| source(StateError::IdentityExhausted))?;
    if payer.is_top(value, domain)? {
        return Ok(fresh);
    }
    // work_size explicitly owns one native observation export. Pay the graph
    // from the ACTUAL exported observation size after the hook returns.
    payer.reserve(value.numeric().work_size())?;
    let observation = value.numeric().observation();
    payer.check()?;
    payer.reserve(
        observation
            .work_size()
            .saturating_add(16)
            .saturating_mul(32),
    )?;
    let observed = SlotStateId::observed(
        SourceId::fresh().map_err(source)?,
        vec![StateBinding {
            index: U256::ZERO,
            value: fresh,
            numeric: observation,
        }],
        ExprLimits::default().into(),
    )
    .map_err(source)?;
    ExprId::slot_read(
        observed,
        ExprId::constant(U256::ZERO),
        ExprLimits::default(),
    )
    .map_err(source)
}

// Paid counterpart of the small operand-retention boundary. It uses the same
// validation/normalization/informativity/union order; each changing native
// intermediate is quoted again instead of prepaying an input-sized multiplier.
fn remember<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    facts: &mut Facts<N>,
    expression: ExprId,
    value: &EvmValue<N>,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<()> {
    let policy = limits(domain);
    check_value(value, policy).map_err(source)?;
    if expression.nodes() > policy.max_nodes || expression.depth() > policy.max_depth {
        return Err(source(ExprError::Limit {
            nodes: expression.nodes(),
            depth: expression.depth(),
        }));
    }
    let mut clean = payer.copy(value)?;
    clean.forget_identity();
    if let Some(word) = expression.as_constant() {
        let literal = payer.literal(word, domain)?;
        clean = payer
            .join(&clean, &literal, context, domain)?
            .with_numeric(payer.literal_numeric(word, domain)?);
    }
    payer.reserve(facts.len().saturating_add(1))?;
    for key in facts.keys() {
        payer.reserve(key.comparison_work(&expression))?;
    }
    let informative = if let Some(word) = expression.as_constant() {
        let literal = payer.literal(word, domain)?;
        !payer.equal(&clean, &literal, domain)?
    } else {
        true
    };
    if informative || facts.contains_key(&expression) {
        let clean = if let Some(old) = facts.get(&expression) {
            payer.join(old, &clean, context, domain)?
        } else {
            clean
        };
        facts.insert(expression, clean);
    }
    if facts.len() > policy.max_nodes {
        return Err(source(ExprError::Limit {
            nodes: facts.len(),
            depth: 1,
        }));
    }
    Ok(())
}

pub(in crate::world) fn read_word_with_payment<
    N: WordElement,
    D: EvmNumericAlgebra<Element = N>,
>(
    array: &EvmByteArray<N>,
    offset: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> ReadResult<EvmValue<N>> {
    if let Some(error) = &array.failure {
        return Err(SummaryError::Source(error.clone()));
    }
    crate::domain::symbolic::state_check(array.root.nodes(), array.root.depth(), limits(domain))
        .map_err(source)?;
    let mut payer = ReadPayment::new(budget);
    let offset_expression = offset_expression(offset, domain, &mut payer)?;
    payer.reserve(array.operands.len().saturating_add(16))?;
    let mut facts = BTreeMap::new();
    for (key, value) in &array.operands {
        payer.reserve(
            key.work_size()
                .saturating_add(facts.len())
                .saturating_add(1),
        )?;
        facts.insert(key.clone(), payer.copy(value)?);
    }
    // An empty context supplies only join metadata laws while the temporary
    // operand map is normalized; it cannot select later conditional scopes.
    let empty = Facts::new();
    payer.reserve(32)?;
    // Empty facts and no conditional scopes make this constructor entirely
    // structural: there is no native value on which it could run a hook.
    let context = Context::new(&empty, None, domain);
    remember(
        &mut facts,
        offset_expression.clone(),
        offset,
        &context,
        domain,
        &mut payer,
    )?;
    array.conditional.check(limits(domain)).map_err(source)?;
    let context = Context::with_payment(&facts, Some(&array.conditional), domain, budget)?;
    let mut payer = ReadPayment::new(budget);
    let expression = expression(
        &array.root,
        (!array.memory).then_some(&array.length),
        &offset_expression,
        &context,
        domain,
        &mut payer,
    )?;
    let retained = context.retained_word_paid(&expression, domain, &mut payer)?;
    let value = match retained {
        Some(value) => value,
        None => Reader::with_context(context.clone(), domain).evaluate_paid(
            super::ReadTask::Word(&array.root, offset_expression),
            &mut payer,
        )?,
    };
    let value = if let Some(word) = expression.as_constant() {
        value.with_numeric(payer.literal_numeric(word, domain)?)
    } else {
        value
    };
    payer.reserve(expression.work_size().saturating_add(16))?;
    payer.context(&context)?;
    Ok(value.with_expression(expression))
}
