//! Numerical procedure payloads, independent of immutable runtime identities.
//!
//! Projection deliberately forgets allocation, alias and path correlations. A
//! summary is never an executable array: each instantiation gets fresh sources.

use super::{ByteArray, COPY_FACT_LIMIT, RangeError, limits, zero};
use crate::domain::{
    AbstractValue, Domain,
    symbolic::{
        ByteStateId, ByteStateKind, ByteView, ExprError, ExprId, SlotStateId, SourceId,
        StateBinding, StateError,
    },
};
use alloy_primitives::U256;
use std::{collections::BTreeMap, sync::Arc};

mod layout;
#[cfg(test)]
mod retention_tests;

pub(in crate::world) fn scalar(value: &AbstractValue, domain: Domain) -> AbstractValue {
    let projected = domain.project(value);
    if let Some(constant) = projected.singleton() {
        AbstractValue::from_numeric(crate::domain::NumericValue::constant(constant))
    } else {
        AbstractValue::from_numeric(projected.numeric().clone())
    }
}

// The root is authoritative. A stale presentation cell can supply only a dense
// finite allocation proved redundant by the newly evaluated interval; no old
// masks, bounds, congruence, nonzero or metadata enter the current scalar.
fn restore_presentation(
    value: AbstractValue,
    previous: Option<&AbstractValue>,
    domain: Domain,
) -> AbstractValue {
    let Some(previous) = previous else {
        return value;
    };
    // Reprojection to a smaller capacity must not resurrect a finite component
    // that the immediately preceding scalar/profile projection would discard.
    if previous
        .finite_constants()
        .cardinality()
        .is_some_and(|count| count > domain.capacity())
    {
        return value;
    }
    let numeric = value
        .numeric()
        .restore_redundant_finite_from(previous.numeric());
    value.with_numeric(numeric)
}

pub(in crate::world) fn join_scalar(
    old: &AbstractValue,
    incoming: &AbstractValue,
    domain: Domain,
    widen: bool,
) -> AbstractValue {
    let next = domain.join(old, incoming);
    let next = if widen {
        domain.widen_procedure(old, &next)
    } else {
        next
    };
    scalar(&next, domain)
}

/// An independent word with every retained numerical constraint.
pub(in crate::world) fn instantiate_scalar(
    value: &AbstractValue,
    domain: Domain,
) -> Result<AbstractValue, RangeError> {
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
            numeric: value.numeric().clone(),
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
pub(in crate::world) fn scalar_expression_work(value: &AbstractValue) -> usize {
    if value.singleton().is_some() || value.numeric().is_top() {
        1
    } else {
        value.numeric().work_size().saturating_add(8)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Content {
    Zero,
    Exact(Arc<[u8]>),
    Cells(BTreeMap<usize, AbstractValue>),
}

impl Content {
    fn exact(bytes: Arc<[u8]>) -> Self {
        match bytes.iter().rposition(|byte| *byte != 0) {
            None => Self::Zero,
            Some(last) if last + 1 == bytes.len() => Self::Exact(bytes),
            Some(last) => Self::Exact(Arc::from(&bytes[..=last])),
        }
    }

    fn at(&self, index: usize) -> AbstractValue {
        match self {
            Self::Zero => scalar(&zero(), Domain::default()),
            Self::Exact(bytes) => scalar(
                &AbstractValue::constant(U256::from(bytes.get(index).copied().unwrap_or(0))),
                Domain::default(),
            ),
            Self::Cells(cells) => cells
                .get(&index)
                .cloned()
                .unwrap_or_else(AbstractValue::unknown_byte),
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
pub struct ByteArraySummary {
    length: AbstractValue,
    memory: bool,
    content: Content,
}

impl ByteArray {
    /// Evaluate the complete content graph before forgetting its correlations.
    pub fn summary_projection(&self, domain: Domain) -> Result<ByteArraySummary, RangeError> {
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
            ByteStateKind::Zero => Content::Zero,
            ByteStateKind::Exact(bytes) => Content::exact(Arc::clone(bytes)),
            _ => {
                if let Some(exact) = self.exact_bytes_bounded(COPY_FACT_LIMIT) {
                    Content::exact(Arc::from(exact))
                } else {
                    let mut keys = self.bytes.keys().copied().collect::<Vec<_>>();
                    if let Ok(upper) = usize::try_from(self.length.interval().unsigned_bounds().1)
                        && upper <= COPY_FACT_LIMIT
                    {
                        keys.extend(0..upper);
                    }
                    keys.sort_unstable();
                    keys.dedup();
                    let mut cells = BTreeMap::new();
                    let mut reader = super::numeric::Reader::with_context(
                        super::conditional::Context::new(&self.operands, Some(&self.conditional)),
                        domain,
                    );
                    for index in keys.into_iter().take(COPY_FACT_LIMIT) {
                        if analysis_control::current_cancellation().is_cancelled() {
                            return Err(ExprError::Cancelled.into());
                        }
                        let value = scalar(
                            &reader.read(&self.root, &ExprId::constant(U256::from(index))),
                            domain,
                        );
                        let value = restore_presentation(value, self.bytes.get(&index), domain);
                        if value.numeric() != AbstractValue::unknown_byte().numeric() {
                            cells.insert(index, value);
                        }
                    }
                    Content::Cells(cells)
                }
            }
        };
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled.into());
        }
        Ok(ByteArraySummary {
            length: scalar(&self.length, domain),
            memory: self.memory,
            content,
        })
    }
}

impl ByteArraySummary {
    /// Logical input length or MSIZE projection, without an equality identity.
    pub fn len(&self) -> &AbstractValue {
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
            Content::Zero => 1,
            Content::Exact(bytes) => {
                canonical = canonical.saturating_add(bytes.len());
                // One exact root, its shared buffer, and at most one numerical
                // cached constant per already-materialized byte.
                bytes.len().saturating_mul(15).saturating_add(1)
            }
            Content::Cells(cells) => {
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
    }

    /// Complete numerical union. Widening affects growing word fields only.
    pub fn join(&self, other: &Self, domain: Domain, widen: bool) -> Self {
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
                    (value.numeric() != AbstractValue::unknown_byte().numeric())
                        .then_some((index, value))
                })
                .collect();
            Content::Cells(cells)
        };
        Self {
            length: join_scalar(&self.length, &other.length, domain, widen),
            memory: self.memory && other.memory,
            content,
        }
    }

    /// Coverage of content, logical length and allocation mode together.
    pub fn covers(&self, other: &Self, domain: Domain) -> bool {
        self.join(other, domain, false) == *self
    }

    /// Build independent symbolic cells and a constrained length for one use.
    pub fn instantiate(&self, domain: Domain) -> Result<ByteArray, RangeError> {
        let length = instantiate_scalar(&self.length, domain)?;
        let mut out = ByteArray {
            length,
            bytes: BTreeMap::new(),
            default: AbstractValue::unknown_byte(),
            memory: self.memory,
            root: ByteStateId::zero(),
            allocations: Default::default(),
            operands: BTreeMap::new(),
            conditional: Default::default(),
            failure: None,
        };
        match &self.content {
            Content::Zero => out.default = scalar(&zero(), domain),
            Content::Exact(bytes) => {
                out.root = ByteStateId::exact(Arc::clone(bytes));
                out.default = scalar(&zero(), domain);
                out.bytes = bytes
                    .iter()
                    .enumerate()
                    .filter(|(_, byte)| **byte != 0)
                    .map(|(index, byte)| {
                        (
                            index,
                            scalar(&AbstractValue::constant(U256::from(*byte)), domain),
                        )
                    })
                    .collect();
            }
            Content::Cells(cells) => {
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
                            numeric: if domain.spec().profile() == crate::domain::Profile::Product {
                                value.numeric().compact_for_retention()
                            } else {
                                // ConstantsOnly joins inspect finite alone;
                                // retain its existing executable representation.
                                value.numeric().clone()
                            },
                        })
                    })
                    .collect::<Result<Vec<_>, RangeError>>()?;
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
            }
        }
        if let Some(expression) = out.length.expression() {
            out.operands.insert(expression.clone(), out.length.clone());
        }
        // A numerical presentation cache needs no second copy of the root in
        // every byte expression. Actual reads attach their state term on demand.
        for index in out.bytes.keys().copied().collect::<Vec<_>>() {
            if analysis_control::current_cancellation().is_cancelled() {
                return Err(ExprError::Cancelled.into());
            }
            let value = scalar(
                &super::numeric::read(
                    &out.root,
                    &ExprId::constant(U256::from(index)),
                    &out.operands,
                    domain,
                ),
                domain,
            );
            let value = restore_presentation(value, out.bytes.get(&index), domain);
            out.bytes.insert(index, value);
        }
        out.check_existing(limits(domain))?;
        Ok(out)
    }
}
