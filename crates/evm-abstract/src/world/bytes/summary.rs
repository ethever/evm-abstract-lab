//! Numerical procedure payloads, independent of immutable runtime identities.
//!
//! Projection deliberately forgets allocation, alias and path correlations. A
//! summary is never an executable array: each instantiation gets fresh sources.

use super::{COPY_FACT_LIMIT, EvmByteArray, RangeError, limits, zero};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue,
    symbolic::{
        ByteStateId, ByteStateKind, ByteView, ExprError, ExprId, SlotStateId, SourceId,
        StateBinding, StateError,
    },
};
use alloy_primitives::U256;
use std::{collections::BTreeMap, sync::Arc};
#[cfg(test)]
use {crate::world::test_support::TestAlgebra, crate::world::test_support::TestValue};

mod layout;
#[cfg(test)]
use crate::world::test_support::TestBytesSummary;
mod presentation;
#[cfg(test)]
mod retention_tests;
mod words;

pub(in crate::world) fn scalar<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    value: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue<N> {
    let projected = domain.project(value);
    EvmValue::<N>::from_numeric(projected.numeric().canonical_for_summary())
}

// The root is authoritative. A stale presentation cell can supply only a dense
// finite allocation proved redundant by the newly evaluated interval; no old
// masks, bounds, congruence, nonzero or metadata enter the current scalar.
fn restore_presentation<N: WordElement>(
    value: EvmValue<N>,
    previous: Option<&EvmValue<N>>,
) -> EvmValue<N> {
    let Some(previous) = previous else {
        return value;
    };
    let numeric = value
        .numeric()
        .restore_redundant_finite_from(previous.numeric());
    value.with_numeric(numeric)
}

pub(in crate::world) fn join_scalar<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    old: &EvmValue<N>,
    incoming: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    widen: bool,
) -> EvmValue<N> {
    let next = domain.join(old, incoming);
    let next = if widen {
        domain.widen_procedure(old, &next)
    } else {
        next
    };
    scalar(&next, domain)
}

/// An independent word with every retained numerical constraint.
pub(in crate::world) fn instantiate_scalar<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    value: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
) -> Result<EvmValue<N>, RangeError> {
    if let Some(constant) = value.singleton() {
        return Ok(value.clone().with_expression(ExprId::constant(constant)));
    }
    if value.numeric().is_top() {
        // Top contributes no numeric constraint. An independent leaf expresses
        // that fact without an observed-array graph copied into every byte read.
        let expression = ExprId::fresh().ok_or(StateError::IdentityExhausted)?;
        return Ok(value.clone().with_expression(expression));
    }
    let source = SourceId::fresh()?;
    let plain = SlotStateId::input(source.clone());
    let raw = ExprId::slot_read(plain, ExprId::constant(U256::ZERO), limits(domain))?;
    let root = SlotStateId::observed(
        source,
        vec![StateBinding {
            index: U256::ZERO,
            value: raw,
            numeric: value.numeric().observation(),
        }],
        limits(domain).into(),
    )?;
    Ok(value.clone().with_expression(ExprId::slot_read(
        root,
        ExprId::constant(U256::ZERO),
        limits(domain),
    )?))
}

/// Node bound for the independent word built by instantiate_scalar.
pub(in crate::world) fn scalar_expression_work<N: WordElement>(value: &EvmValue<N>) -> usize {
    if value.singleton().is_some() || value.numeric().is_top() {
        1
    } else {
        value.numeric().work_size().saturating_add(8)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Content<N: WordElement = crate::domain::NumericValue> {
    Zero,
    Exact(Arc<[u8]>),
    Cells(BTreeMap<usize, EvmValue<N>>),
}

impl<N: WordElement> Content<N> {
    fn exact(bytes: Arc<[u8]>) -> Self {
        match bytes.iter().rposition(|byte| *byte != 0) {
            None => Self::Zero,
            Some(last) if last + 1 == bytes.len() => Self::Exact(bytes),
            Some(last) => Self::Exact(Arc::from(&bytes[..=last])),
        }
    }

    fn at(&self, index: usize) -> EvmValue<N> {
        match self {
            Self::Zero => EvmValue::<N>::from_numeric(N::constant(U256::ZERO)),
            Self::Exact(bytes) => EvmValue::<N>::from_numeric(N::constant(U256::from(
                bytes.get(index).copied().unwrap_or(0),
            ))),
            Self::Cells(cells) => cells
                .get(&index)
                .cloned()
                .unwrap_or_else(EvmValue::<N>::unknown_byte),
        }
    }

    fn keys(&self) -> Vec<usize> {
        match self {
            Self::Zero => Vec::new(),
            Self::Exact(bytes) => (0..bytes.len().min(COPY_FACT_LIMIT)).collect(),
            Self::Cells(cells) => cells.keys().copied().collect(),
        }
    }
}

/// Canonical byte/length upper bound used for procedure input and output joins.
/// It contains no source identities, write history or caller path predicates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmByteArraySummary<N: WordElement> {
    length: EvmValue<N>,
    memory: bool,
    content: Content<N>,
    // Unsupported predicates of complete fixed word windows. Scalar payloads
    // contain no old root identity; instantiation binds them to its fresh root.
    words: BTreeMap<usize, EvmValue<N>>,
}

impl<N: WordElement> EvmByteArray<N> {
    pub(super) fn retained_word_projection_work_with<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        projection: usize,
    ) -> usize {
        if !words::needed(self) {
            return 0;
        }
        let scalar = self
            .operands
            .values()
            .chain([&self.length, &self.default])
            .fold(0usize, |work, value| {
                work.saturating_add(value.singleton_work())
                    .saturating_add(value.clone_work())
            });
        // At most one complete demand per 32 retained bytes. The existing
        // full-read and projection quotes cover evaluation and scalar policy;
        // custom query/copy fees cover the additional retained word payloads.
        self.word_numeric_work(&self.length, domain)
            .saturating_add(projection)
            .saturating_add(scalar)
            .saturating_mul(COPY_FACT_LIMIT / 32)
    }

    /// Evaluate the complete content graph before forgetting its correlations.
    pub fn summary_projection<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
    ) -> Result<EvmByteArraySummary<N>, RangeError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled.into());
        }
        // Projection consumes an existing complete graph; it does not retain
        // that history in the output. Local symbolic-read retention limits apply when
        // the numerical summary is instantiated, not to its discarded input.
        let content = match self.root.kind() {
            ByteStateKind::Zero => Content::<N>::Zero,
            ByteStateKind::Exact(bytes) => Content::<N>::exact(Arc::clone(bytes)),
            _ => {
                if let Some(exact) = self.exact_bytes_bounded(COPY_FACT_LIMIT, domain) {
                    Content::<N>::exact(Arc::from(exact))
                } else {
                    let mut keys = self.bytes.keys().copied().collect::<Vec<_>>();
                    if let Ok(upper) =
                        usize::try_from(crate::world::unsigned_bounds(&self.length).1)
                        && upper <= COPY_FACT_LIMIT
                    {
                        keys.extend(0..upper);
                    }
                    keys.sort_unstable();
                    keys.dedup();
                    let mut cells = BTreeMap::new();
                    let mut reader = super::numeric::Reader::<N, D>::with_context(
                        super::conditional::Context::<N, D>::new(
                            &self.operands,
                            Some(&self.conditional),
                            domain,
                        ),
                        domain,
                    );
                    for index in keys.into_iter().take(COPY_FACT_LIMIT) {
                        if analysis_control::current_cancellation().is_cancelled() {
                            return Err(ExprError::Cancelled.into());
                        }
                        let value = reader.read(&self.root, &ExprId::constant(U256::from(index)));
                        let value = restore_presentation(value, self.bytes.get(&index));
                        // Apply the selected algebra's policy and capacity last,
                        // including presentation retained under a larger capacity.
                        let value = scalar(&value, domain);
                        if value.numeric() != EvmValue::<N>::unknown_byte().numeric() {
                            cells.insert(index, value);
                        }
                    }
                    Content::<N>::Cells(cells)
                }
            }
        };
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled.into());
        }
        Ok(EvmByteArraySummary {
            length: scalar(&self.length, domain),
            memory: self.memory,
            content,
            words: words::project(self, domain)?,
        })
    }
}

impl<N: WordElement> EvmByteArraySummary<N> {
    /// Logical input length or MSIZE projection, without an equality identity.
    pub fn len(&self) -> &EvmValue<N> {
        &self.length
    }

    /// Retained numerical payload plus a conservative executable replay bound.
    /// Forgotten history and symbolic logical byte lengths add no retained work.
    pub fn work_size(&self) -> usize {
        let length_nodes = scalar_expression_work(&self.length);
        let length_work = self
            .length
            .numeric()
            .work_size()
            .saturating_add(length_nodes);
        let mut canonical = self.length.numeric().work_size().saturating_add(2);
        let content_work = match &self.content {
            Content::<N>::Zero => 1,
            Content::<N>::Exact(bytes) => {
                canonical = canonical.saturating_add(bytes.len());
                // One exact root, its shared buffer, and at most one numerical
                // cached constant per already-materialized byte.
                bytes.len().saturating_mul(15).saturating_add(1)
            }
            Content::<N>::Cells(cells) => {
                let mut constant_cells = 0usize;
                let mut chunks = 0usize;
                let mut previous_constant = None;
                let mut root =
                    cells
                        .iter()
                        .fold(length_nodes.saturating_add(2), |nodes, (index, value)| {
                            if value
                                .singleton()
                                .is_some_and(|byte| byte <= U256::from(255))
                            {
                                constant_cells = constant_cells.saturating_add(1);
                                if previous_constant.and_then(|old: usize| old.checked_add(1))
                                    != Some(*index)
                                {
                                    chunks = chunks.saturating_add(1);
                                }
                                previous_constant = Some(*index);
                                nodes
                            } else {
                                previous_constant = None;
                                nodes
                                    .saturating_add(1)
                                    .saturating_add(value.numeric().work_size())
                            }
                        });
                if chunks != 0 {
                    root = root
                        .saturating_add(chunks.saturating_mul(6))
                        .saturating_add(length_nodes.saturating_add(2));
                }
                cells
                    .values()
                    .fold(root.saturating_add(constant_cells), |work, value| {
                        let numeric = value.numeric().work_size();
                        canonical = canonical.saturating_add(numeric.saturating_add(1));
                        work.saturating_add(numeric.saturating_add(1))
                    })
            }
        };
        canonical
            .saturating_add(content_work)
            .saturating_add(length_work.saturating_mul(2))
            .saturating_add(14)
            .saturating_add(words::work(&self.words, content_work, length_nodes))
    }

    /// Complete numerical union. Widening affects growing word fields only.
    pub fn join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        widen: bool,
    ) -> Self {
        let content = if self.content == other.content {
            self.content.clone()
        } else {
            let mut keys = self.content.keys();
            keys.extend(other.content.keys());
            keys.sort_unstable();
            keys.dedup();
            let cells = keys
                .into_iter()
                .take(COPY_FACT_LIMIT)
                .filter_map(|index| {
                    let value = join_scalar(
                        &self.content.at(index),
                        &other.content.at(index),
                        domain,
                        widen,
                    );
                    (value.numeric() != EvmValue::<N>::unknown_byte().numeric())
                        .then_some((index, value))
                })
                .collect();
            Content::<N>::Cells(cells)
        };
        Self {
            length: join_scalar(&self.length, &other.length, domain, widen),
            memory: self.memory && other.memory,
            content,
            words: words::join(&self.words, &other.words, domain, widen),
        }
    }

    /// Coverage of content, logical length and allocation mode together.
    pub fn covers<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
    ) -> bool {
        self.join(other, domain, false) == *self
    }

    /// Build independent symbolic cells and a constrained length for one use.
    pub fn instantiate<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
    ) -> Result<EvmByteArray<N>, RangeError> {
        let length = instantiate_scalar(&self.length, domain)?;
        let mut out = EvmByteArray {
            length,
            bytes: BTreeMap::new(),
            default: EvmValue::<N>::unknown_byte(),
            memory: self.memory,
            root: ByteStateId::zero(),
            allocations: Default::default(),
            operands: BTreeMap::new(),
            conditional: Default::default(),
            failure: None,
        };
        let mut bare_cells = false;
        match &self.content {
            Content::<N>::Zero => out.default = scalar(&zero(), domain),
            Content::<N>::Exact(bytes) => {
                out.root = ByteStateId::exact(Arc::clone(bytes));
                out.default = scalar(&zero(), domain);
                out.bytes = bytes
                    .iter()
                    .enumerate()
                    .filter(|(_, byte)| **byte != 0)
                    .map(|(index, byte)| {
                        (
                            index,
                            scalar(&EvmValue::<N>::constant(U256::from(*byte)), domain),
                        )
                    })
                    .collect();
            }
            Content::<N>::Cells(cells) => {
                let source = SourceId::fresh()?;
                let len = out
                    .length
                    .symbolic_expression()
                    .expect("instantiated length");
                let bindings = cells
                    .iter()
                    .filter(|(_, value)| {
                        !value
                            .singleton()
                            .is_some_and(|byte| byte <= U256::from(255))
                    })
                    .map(|(index, value)| {
                        Ok(StateBinding {
                            index: U256::from(*index),
                            // The observed input itself equates this independent
                            // word to its byte cell and asserts every numeric
                            // component. A nested read would repeat the complete
                            // length/source graph without adding a guarantee.
                            value: ExprId::fresh().ok_or(StateError::IdentityExhausted)?,
                            // Keep the canonical/presentation cell intact; only
                            // its executable carrier drops a proved redundant
                            // finite component, before observed checks its size.
                            // The original full summary work is still prepaid.
                            numeric: value.numeric().compact_for_retention().observation(),
                        })
                    })
                    .collect::<Result<Vec<_>, RangeError>>()?;
                for binding in &bindings {
                    let index = usize::try_from(binding.index).expect("cell index was usize");
                    let value = cells.get(&index).expect("binding was formed from cell");
                    if !value.numeric().requires_retained_binding() {
                        // The immutable binding already contains the complete
                        // default conjunction. Only unsupported guarantees need
                        // a new nonliteral executable operand fact.
                        continue;
                    }
                    let numeric = value.numeric().compact_for_retention();
                    out.operands.insert(
                        binding.value.clone(),
                        EvmValue::<N>::from_numeric(numeric).with_expression(binding.value.clone()),
                    );
                }
                out.root =
                    ByteStateId::observed(source, len.clone(), bindings, limits(domain).into())?;
                // Constant runs need no observed numeric constraints. Retaining
                // them as exact chunks avoids duplicating length/read graphs in
                // one binding for every zero byte of a returned word.
                let mut values = cells.iter().peekable();
                let mut has_constants = false;
                while let Some((index, value)) = values.next() {
                    let Some(byte) = value.singleton().and_then(|value| u8::try_from(value).ok())
                    else {
                        continue;
                    };
                    let start = *index;
                    let mut chunk = vec![byte];
                    while let Some((next, value)) = values.peek() {
                        if start.checked_add(chunk.len()) != Some(**next) {
                            break;
                        }
                        let Some(byte) =
                            value.singleton().and_then(|value| u8::try_from(value).ok())
                        else {
                            break;
                        };
                        chunk.push(byte);
                        values.next();
                    }
                    let size = ExprId::constant(U256::from(chunk.len()));
                    let source = ByteView::new(
                        ByteStateId::exact(Arc::from(chunk)),
                        ExprId::constant(U256::ZERO),
                        size.clone(),
                    );
                    out.root = out.root.copy(
                        ExprId::constant(U256::from(start)),
                        source,
                        size,
                        limits(domain).into(),
                    )?;
                    has_constants = true;
                }
                if has_constants {
                    // Chunk writes are content facts, not a permission to read
                    // past the independently constrained logical input length.
                    out.root =
                        out.root
                            .slice(ExprId::constant(U256::ZERO), len, limits(domain).into())?;
                }
                out.bytes = cells.clone();
                bare_cells = !has_constants;
            }
        }
        if let Some(expression) = out.length.expression() {
            out.operands.insert(expression.clone(), out.length.clone());
        }
        presentation::refresh(&mut out, bare_cells, domain)?;
        words::instantiate(&self.words, &mut out, domain)?;
        out.check_existing(limits(domain))?;
        Ok(out)
    }
}
