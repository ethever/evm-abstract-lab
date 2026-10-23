//! Fresh graph construction and native scalar materialization use separate fees.

use super::{
    Content, EvmByteArray, EvmByteArraySummary, EvmValue, ExprId, RangeError, SlotStateId,
    SourceId, StateBinding, StateError, limits,
};
use crate::{
    U256,
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, WordObservation, summary},
    resource::{
        CloneError, SummaryError, WorkBudget,
        clone_payment::{checkpoint, pay},
    },
};
use std::{collections::BTreeMap, sync::Arc};

type Error = SummaryError<RangeError>;

fn source(error: impl Into<RangeError>) -> Error {
    SummaryError::Source(error.into())
}

/// Immutable symbolic/observation traversal only. Native execution has already
/// paid its own quote; this fee never authorizes a numeric hook.
pub(super) fn graph_work(size: usize) -> usize {
    size.saturating_add(8)
        .saturating_mul((size.saturating_add(1).ilog2() as usize).saturating_add(2))
        .saturating_mul(16)
}

pub(super) fn observation<N: WordElement>(
    value: &EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<WordObservation, CloneError> {
    checkpoint(budget)?;
    // WordElement's structural quote explicitly owns one observation export.
    pay(budget, value.numeric().work_size())?;
    let observation = value.numeric().observation();
    checkpoint(budget)?;
    Ok(observation)
}

fn compact_plain<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    value: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<N>, CloneError> {
    checkpoint(budget)?;
    // Bindings carry only the compact N. The shared compact helper preserves
    // wrapper metadata, while pure replay intentionally starts a plain wrapper.
    let work = domain
        .numeric_algebra()
        .summary_compact_work(value.numeric());
    pay(budget, work.saturating_add(16))?;
    let out = EvmValue::from_numeric(value.numeric().compact_for_retention());
    checkpoint(budget)?;
    Ok(out)
}

pub(super) fn expression<N: WordElement>(
    value: &EvmValue<N>,
    budget: &mut WorkBudget,
) -> Result<ExprId, CloneError> {
    let singleton = summary::singleton(value, budget)?;
    pay(budget, 8)?;
    Ok(singleton
        .map(ExprId::constant)
        .or_else(|| value.expression().cloned())
        .expect("instantiated length has an expression"))
}

pub(in crate::world) fn instantiate_scalar_with_payment<
    N: WordElement,
    D: EvmNumericAlgebra<Element = N>,
>(
    value: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmValue<N>, Error> {
    pay(budget, 8)?;
    if let Some(constant) = summary::singleton(value, budget)? {
        return Ok(summary::clone_value(value, budget)?.with_expression(ExprId::constant(constant)));
    }
    if summary::is_top(value, domain, budget)? {
        let expression = ExprId::fresh().ok_or_else(|| source(StateError::IdentityExhausted))?;
        return Ok(summary::clone_value(value, budget)?.with_expression(expression));
    }
    let observed = observation(value, budget)?;
    pay(budget, graph_work(observed.work_size()))?;
    let source_id = SourceId::fresh().map_err(source)?;
    let plain = SlotStateId::input(source_id.clone());
    let raw =
        ExprId::slot_read(plain, ExprId::constant(U256::ZERO), limits(domain)).map_err(source)?;
    let root = SlotStateId::observed(
        source_id,
        vec![StateBinding {
            index: U256::ZERO,
            value: raw,
            numeric: observed,
        }],
        limits(domain).into(),
    )
    .map_err(source)?;
    let expression =
        ExprId::slot_read(root, ExprId::constant(U256::ZERO), limits(domain)).map_err(source)?;
    let out = summary::clone_value(value, budget)?.with_expression(expression);
    checkpoint(budget)?;
    Ok(out)
}

impl<N: WordElement> EvmByteArraySummary<N> {
    pub(crate) fn instantiate_with_payment<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<EvmByteArray<N>, Error> {
        pay(budget, 16)?;
        let length = instantiate_scalar_with_payment(&self.length, domain, budget)?;
        let default = summary::unknown_byte(domain, budget)?;
        let mut out = EvmByteArray {
            length,
            bytes: BTreeMap::new(),
            default,
            memory: self.memory,
            root: super::ByteStateId::zero(),
            allocations: Default::default(),
            operands: BTreeMap::new(),
            conditional: Default::default(),
            failure: None,
        };
        let mut bare_cells = false;
        match &self.content {
            Content::Zero => {
                let zero = summary::constant(U256::ZERO, domain, budget)?;
                out.default = summary::scalar(&zero, domain, budget)?;
            }
            Content::Exact(bytes) => {
                pay(budget, graph_work(bytes.len()))?;
                out.root = super::ByteStateId::exact(Arc::clone(bytes));
                let zero = summary::constant(U256::ZERO, domain, budget)?;
                out.default = summary::scalar(&zero, domain, budget)?;
                for (index, byte) in bytes.iter().enumerate().filter(|(_, byte)| **byte != 0) {
                    let value = summary::constant(U256::from(*byte), domain, budget)?;
                    let value = summary::scalar(&value, domain, budget)?;
                    out.bytes.insert(index, value);
                }
            }
            Content::Cells(cells) => {
                pay(budget, graph_work(cells.len()))?;
                let source_id = SourceId::fresh().map_err(source)?;
                let len = expression(&out.length, budget)?;
                let mut bindings = Vec::new();
                let mut bytes = BTreeMap::new();
                for (index, value) in cells {
                    let byte =
                        summary::singleton(value, budget)?.and_then(|word| u8::try_from(word).ok());
                    bytes.insert(*index, byte);
                    if byte.is_some() {
                        continue;
                    }
                    let compact = compact_plain(value, domain, budget)?;
                    let observed = observation(&compact, budget)?;
                    pay(budget, graph_work(observed.work_size()))?;
                    bindings.push(StateBinding {
                        index: U256::from(*index),
                        value: ExprId::fresh()
                            .ok_or_else(|| source(StateError::IdentityExhausted))?,
                        numeric: observed,
                    });
                }
                for binding in &bindings {
                    let index = usize::try_from(binding.index).expect("cell index was usize");
                    let value = &cells[&index];
                    if !value.numeric().requires_retained_binding() {
                        continue;
                    }
                    let value = compact_plain(value, domain, budget)?;
                    out.operands.insert(
                        binding.value.clone(),
                        value.with_expression(binding.value.clone()),
                    );
                }
                let binding_work = bindings.iter().fold(len.work_size(), |work, binding| {
                    work.saturating_add(binding.numeric.work_size())
                        .saturating_add(2)
                });
                pay(budget, graph_work(binding_work))?;
                out.root = super::ByteStateId::observed(
                    source_id,
                    len.clone(),
                    bindings,
                    limits(domain).into(),
                )
                .map_err(source)?;
                let mut values = bytes.iter().peekable();
                let mut has_constants = false;
                while let Some((index, byte)) = values.next() {
                    let Some(byte) = *byte else {
                        continue;
                    };
                    let start = *index;
                    let mut chunk = vec![byte];
                    while let Some((next, byte)) = values.peek() {
                        if start.checked_add(chunk.len()) != Some(**next) {
                            break;
                        }
                        let Some(byte) = **byte else {
                            break;
                        };
                        chunk.push(byte);
                        values.next();
                    }
                    pay(
                        budget,
                        graph_work(out.root.work_size().saturating_add(chunk.len())),
                    )?;
                    let size = ExprId::constant(U256::from(chunk.len()));
                    let view = super::ByteView::new(
                        super::ByteStateId::exact(Arc::from(chunk)),
                        ExprId::constant(U256::ZERO),
                        size.clone(),
                    );
                    out.root = out
                        .root
                        .copy(
                            ExprId::constant(U256::from(start)),
                            view,
                            size,
                            limits(domain).into(),
                        )
                        .map_err(source)?;
                    has_constants = true;
                }
                if has_constants {
                    pay(
                        budget,
                        graph_work(out.root.work_size().saturating_add(len.work_size())),
                    )?;
                    out.root = out
                        .root
                        .slice(ExprId::constant(U256::ZERO), len, limits(domain).into())
                        .map_err(source)?;
                }
                for (index, value) in cells {
                    out.bytes
                        .insert(*index, summary::clone_value(value, budget)?);
                }
                bare_cells = !has_constants;
            }
        }
        if let Some(expression) = out.length.expression() {
            pay(budget, 8)?;
            out.operands.insert(
                expression.clone(),
                summary::clone_value(&out.length, budget)?,
            );
        }
        super::presentation::refresh_with_payment(&mut out, bare_cells, domain, budget)?;
        super::words::instantiate_with_payment(&self.words, &mut out, domain, budget)?;
        check(&out, domain, budget)?;
        checkpoint(budget)?;
        Ok(out)
    }
}

/// Same final retention checks as check_existing, without that pure visitor's
/// temporary native Top values for expression-only graph nodes.
fn check<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    out: &EvmByteArray<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<(), Error> {
    pay(
        budget,
        graph_work(
            out.root
                .work_size()
                .saturating_add(out.bytes.len())
                .saturating_add(out.operands.len()),
        ),
    )?;
    out.root
        .join_guarded(
            &out.root,
            ExprId::constant(U256::ZERO),
            limits(domain).into(),
        )
        .map_err(source)?;
    out.conditional.check(limits(domain)).map_err(source)?;
    for value in [&out.length, &out.default]
        .into_iter()
        .chain(out.bytes.values())
        .chain(out.operands.values())
    {
        pay(
            budget,
            graph_work(value.expression().map_or(1, ExprId::work_size)),
        )?;
        super::super::check_value(value, limits(domain)).map_err(source)?;
    }
    for expression in super::super::traversal::expressions(&out.root) {
        crate::domain::symbolic::state_check(
            expression.nodes(),
            expression.depth(),
            limits(domain),
        )
        .map_err(source)?;
    }
    Ok(())
}
