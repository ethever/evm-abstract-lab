//! EVM byte sequences backed by immutable symbolic states.
//!
//! The root is the content model. Sparse facts and operand summaries provide
//! numeric upper bounds and bounded presentation; COPY never expands its range.

mod conditional;
mod copy;
mod metadata;
pub(super) mod native;
pub(super) mod numeric;
pub(super) mod operands;
pub(super) mod refinement;
pub use refinement::GuardRefinementError;
mod ranges;
mod reads;
mod refresh;
mod slice;
pub(in crate::world) mod summary;
pub use summary::ByteArraySummary;
#[cfg(test)]
mod tests;
mod traversal;
pub(super) mod widening;
mod word_plan;
mod write;

use crate::domain::{
    AbstractValue, Domain,
    symbolic::{
        ByteStateId, ByteStateKind, ByteView, ExprError, ExprId, ExprLimits, SourceId, StateError,
    },
};
use alloy_primitives::{U256, keccak256};
use revm_bytecode::opcode;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use thiserror::Error;

/// A finite input/view or zero-initialized memory with a separate logical MSIZE.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ByteArray {
    length: AbstractValue,
    bytes: BTreeMap<usize, AbstractValue>,
    default: AbstractValue,
    memory: bool,
    root: ByteStateId,
    // Must facts: each range was already included in this allocation on every
    // represented path. These normalize repeated symbolic expansion safely.
    #[serde(skip)]
    allocations: BTreeSet<(ExprId, ExprId)>,
    #[serde(serialize_with = "native::serialize_facts")]
    operands: BTreeMap<ExprId, AbstractValue>,
    // Implications retain source bounds without asserting them on other paths.
    conditional: conditional::ConditionalFacts,
    #[serde(serialize_with = "native::serialize_failure")]
    failure: Option<RangeError>,
}

/// A logical range or state operation cannot be represented within its policy.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RangeError {
    /// The caller supplied neither a finite size nor a symbolic size expression.
    #[error("byte range has unknown size without an expression")]
    UnknownSize,
    /// A nonempty target at this offset has no representable U256 logical end.
    #[error("byte range at {0} has no representable logical end")]
    OffsetTooLarge(U256),
    /// Concrete target range overflow.
    #[error("byte range at {offset} of size {size} overflows")]
    Overflow {
        /// Concrete host start offset.
        offset: usize,
        /// Concrete requested byte count.
        size: usize,
    },
    /// Concrete logical range exceeds the caller's range policy.
    #[error("byte range requires {requested} bytes, exceeding limit {limit}")]
    TooLarge {
        /// Required logical allocation or offending requested length.
        requested: U256,
        /// Caller-selected logical byte cap.
        limit: usize,
    },
    /// Scalar expression retention failed.
    #[error("{0}")]
    Expression(ExprError),
    /// Immutable state retention failed.
    #[error("{0}")]
    State(StateError),
}

impl From<ExprError> for RangeError {
    fn from(error: ExprError) -> Self {
        Self::Expression(error)
    }
}

impl From<StateError> for RangeError {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

// This limits duplicate presentation facts, not symbolic semantics or logical bytes.
const COPY_FACT_LIMIT: usize = 256;

pub(super) fn limits(domain: Domain) -> ExprLimits {
    domain.spec().relations().expressions()
}
pub(super) fn zero() -> AbstractValue {
    constant(0)
}
pub(super) fn constant(value: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}
pub(super) fn expr(value: &AbstractValue) -> Result<ExprId, RangeError> {
    // A numeric proof does not replace the source term of the same value.
    // Retain that term; its numeric guarantee is paired in operand facts.
    if let Some(expression) = value.expression() {
        return Ok(expression.clone());
    }
    if let Some(expression) = value.symbolic_expression() {
        return Ok(expression);
    }
    let fresh = ExprId::fresh().ok_or(StateError::IdentityExhausted)?;
    if value.numeric().is_top() {
        return Ok(fresh);
    }
    // A numeric-only finite/bounded operand needs its own semantic constraint,
    // not an unconstrained Fresh which could contradict a precise numeric read.
    let observed = crate::domain::symbolic::SlotStateId::observed(
        SourceId::fresh()?,
        vec![crate::domain::symbolic::StateBinding {
            index: U256::ZERO,
            value: fresh,
            numeric: value.numeric().clone(),
        }],
        ExprLimits::default().into(),
    )?;
    Ok(ExprId::slot_read(
        observed,
        ExprId::constant(U256::ZERO),
        ExprLimits::default(),
    )?)
}

pub(in crate::world) fn check_value(
    value: &AbstractValue,
    limits: ExprLimits,
) -> Result<(), ExprError> {
    if analysis_control::current_cancellation().is_cancelled() {
        return Err(ExprError::Cancelled);
    }
    if value.symbolic_limit_reached() {
        return Err(ExprError::Limit {
            nodes: usize::MAX,
            depth: usize::MAX,
        });
    }
    if let Some(expression) = value.expression()
        && (expression.nodes() > limits.max_nodes || expression.depth() > limits.max_depth)
    {
        return Err(ExprError::Limit {
            nodes: expression.nodes(),
            depth: expression.depth(),
        });
    }
    Ok(())
}

impl ByteArray {
    /// Empty finite input.
    pub fn empty() -> Self {
        Self::exact(&[])
    }
    /// Observed finite input, zero-padded beyond its length.
    pub fn exact(input: &[u8]) -> Self {
        Self {
            length: constant(input.len()),
            bytes: input
                .iter()
                .enumerate()
                .filter(|(_, byte)| **byte != 0)
                .map(|(i, byte)| (i, constant(usize::from(*byte))))
                .collect(),
            default: zero(),
            memory: false,
            root: ByteStateId::exact(Arc::from(input)),
            allocations: BTreeSet::new(),
            operands: BTreeMap::new(),
            conditional: Default::default(),
            failure: None,
        }
    }
    /// Independent unobserved finite input. Length is not inferred from storage.
    pub fn unknown() -> Self {
        Self::unknown_checked().expect("process-private input identities available")
    }
    /// Fallible construction for callers tracking identity exhaustion.
    pub fn unknown_checked() -> Result<Self, RangeError> {
        let length = AbstractValue::top().with_expression(expr(&AbstractValue::top())?);
        Ok(Self {
            root: ByteStateId::input(
                SourceId::fresh()?,
                length.symbolic_expression().expect("bound length"),
            ),
            length,
            bytes: BTreeMap::new(),
            default: AbstractValue::unknown_byte(),
            memory: false,
            allocations: BTreeSet::new(),
            operands: BTreeMap::new(),
            conditional: Default::default(),
            failure: None,
        })
    }
    /// Fresh unbounded zero content with zero logical allocation.
    pub fn memory() -> Self {
        Self {
            memory: true,
            root: ByteStateId::zero(),
            ..Self::empty()
        }
    }
    /// Bind a fresh root calldata input once; existing inputs/views keep identity.
    pub fn bind_input(&mut self, scope: u64) -> Result<(), RangeError> {
        use crate::domain::identity::Symbol;
        if self.memory {
            return Ok(());
        }
        match self.root.kind() {
            ByteStateKind::Exact(_) => return Ok(()),
            ByteStateKind::Input { source, .. } => {
                if matches!(
                    source.token().kind(),
                    crate::domain::symbolic::ExprKind::Input(_)
                ) {
                    return Ok(());
                }
            }
            _ => return Ok(()),
        }
        let length = self
            .length
            .clone()
            .with_expression(ExprId::input(scope, Symbol::CalldataLength));
        let bindings = match self.root.kind() {
            ByteStateKind::Input { bindings, .. } => bindings.clone(),
            _ => unreachable!(),
        };
        self.root = ByteStateId::observed(
            SourceId::input(scope, Symbol::CalldataLength),
            length.expression().expect("input length").clone(),
            bindings,
            ExprLimits::default().into(),
        )?;
        self.length = length;
        Ok(())
    }
    /// Full immutable root, including all ordered writes and source bindings.
    pub fn state_root(&self) -> &ByteStateId {
        &self.root
    }
    /// Full finite view. Memory consumers may capture narrower views for MLOAD.
    pub fn view(&self) -> ByteView {
        ByteView::new(
            self.root.clone(),
            ExprId::constant(U256::ZERO),
            self.length
                .symbolic_expression()
                .expect("every byte length has an expression or singleton"),
        )
    }
    /// Logical input length or memory MSIZE, including its immutable expression.
    pub fn len(&self) -> &AbstractValue {
        &self.length
    }
    /// Explicit current numeric upper bounds. The root also covers unlisted bytes.
    pub fn stored_bytes(&self) -> impl ExactSizeIterator<Item = (usize, &AbstractValue)> {
        self.bytes.iter().map(|(i, v)| (*i, v))
    }
    /// Unconditional numeric guarantees paired with immutable expression identity.
    pub fn symbolic_facts(&self) -> impl ExactSizeIterator<Item = (&ExprId, &AbstractValue)> {
        self.operands.iter()
    }
    /// Numeric upper bound on every unlisted byte; identities live in the root.
    pub fn default_byte(&self) -> &AbstractValue {
        &self.default
    }
    /// Whether logical allocation uses EVM word rounding.
    pub fn is_memory(&self) -> bool {
        self.memory
    }
    /// Retained metadata and scalar work, independent of logical COPY lengths.
    pub fn work_size(&self) -> usize {
        self.bytes
            .values()
            .chain(self.operands.values())
            .chain([&self.length, &self.default])
            .fold(
                self.root
                    .work_size()
                    .saturating_add(traversal::exact_work(&self.root))
                    .saturating_add(self.conditional.work_size())
                    .saturating_add(self.allocations.iter().fold(0usize, |n, (at, size)| {
                        n.saturating_add(at.nodes()).saturating_add(size.nodes())
                    })),
                |n, v| n.saturating_add(v.work_size()),
            )
    }
    pub(crate) fn symbolic_limit_reached(&self) -> bool {
        self.failure.is_some()
            || self.conditional.symbolic_limit_reached()
            || self
                .bytes
                .values()
                .chain(self.operands.values())
                .chain([&self.length, &self.default])
                .any(AbstractValue::symbolic_limit_reached)
    }
    /// Byte read from this immutable state, with finite input zero padding.
    pub fn read_byte_checked(
        &self,
        offset: &AbstractValue,
        domain: Domain,
    ) -> Result<AbstractValue, RangeError> {
        self.read_byte_with_numeric(offset, domain, None)
    }
    // Only A1's writer may share the complete numeric projection. Preparation,
    // authoritative expression construction and checks still run for each byte.
    fn read_byte_with_numeric(
        &self,
        offset: &AbstractValue,
        domain: Domain,
        numeric: Option<&mut Option<AbstractValue>>,
    ) -> Result<AbstractValue, RangeError> {
        let (offset_expr, facts) = self.prepare_byte_read(offset, domain)?;
        let context = conditional::Context::new(&facts, Some(&self.conditional));
        self.read_byte_prepared(&offset_expr, &context, domain, || {
            let read = || {
                #[cfg(test)]
                write::NUMERIC_READS.update(|calls| *calls += 1);
                numeric::Reader::with_context(context.clone(), domain)
                    .read(&self.root, &offset_expr)
            };
            match numeric {
                Some(shared) => shared.get_or_insert_with(read).clone(),
                None => read(),
            }
        })
    }
    // Both refresh paths keep the original checked snapshot and literal remember.
    fn prepare_byte_read(
        &self,
        offset: &AbstractValue,
        domain: Domain,
    ) -> Result<(ExprId, BTreeMap<ExprId, AbstractValue>), RangeError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        crate::domain::symbolic::state_check(self.root.nodes(), self.root.depth(), limits(domain))?;
        let offset_expr = expr(offset)?;
        let mut facts = self.operands.clone();
        operands::remember(
            &mut facts,
            offset_expr.clone(),
            offset,
            domain,
            limits(domain),
        )?;
        self.conditional.check(limits(domain))?;
        Ok((offset_expr, facts))
    }
    fn read_byte_prepared(
        &self,
        offset_expr: &ExprId,
        context: &conditional::Context<'_>,
        domain: Domain,
        read: impl FnOnce() -> AbstractValue,
    ) -> Result<AbstractValue, RangeError> {
        let expression = ExprId::byte_read_with_write_bound(
            self.root.clone(),
            offset_expr.clone(),
            limits(domain),
            &|destination| context.write_lower_bound(destination, domain),
        )?;
        context.check()?;
        let numeric = read();
        context.check()?;
        Ok(numeric::attach(numeric, expression))
    }
    /// Read one byte; retention failure is marked for the analysis boundary.
    pub fn read_byte(&self, offset: &AbstractValue, domain: Domain) -> AbstractValue {
        self.read_byte_checked(offset, domain)
            .unwrap_or_else(|_| AbstractValue::unknown_byte().with_symbolic_limit())
    }
    /// Read a concrete byte index, preserving the current state dependency.
    pub fn byte_at(&self, index: usize, domain: Domain) -> AbstractValue {
        self.read_byte(&constant(index), domain)
    }
    /// A big-endian word, retaining a single state-aware expression.
    pub fn read_word_checked(
        &self,
        offset: &AbstractValue,
        domain: Domain,
    ) -> Result<AbstractValue, RangeError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        crate::domain::symbolic::state_check(self.root.nodes(), self.root.depth(), limits(domain))?;
        let offset_expr = expr(offset)?;
        let mut facts = self.operands.clone();
        operands::remember(
            &mut facts,
            offset_expr.clone(),
            offset,
            domain,
            limits(domain),
        )?;
        self.conditional.check(limits(domain))?;
        let context = conditional::Context::new(&facts, Some(&self.conditional));
        word_plan::evaluate(
            &self.root,
            (!self.memory).then_some(&self.length),
            &offset_expr,
            &context,
            domain,
        )
        .map(|value| value.expect("an unprepared read has no optional cache refusal"))
    }
    /// Read one word; retention failure is marked for the analysis boundary.
    pub fn read_word(&self, offset: &AbstractValue, domain: Domain) -> AbstractValue {
        self.read_word_checked(offset, domain)
            .unwrap_or_else(|_| AbstractValue::top().with_symbolic_limit())
    }
    /// Lazy zero-padded slice with the complete U256 requested length.
    /// `max_bytes` is retained for API compatibility; only physical extraction
    /// (exact_bytes_bounded) applies a host materialization limit.
    pub fn slice(
        &self,
        offset: &AbstractValue,
        size: &AbstractValue,
        _max_bytes: usize,
        domain: Domain,
    ) -> Result<Self, RangeError> {
        if size.singleton() == Some(U256::ZERO) {
            return Ok(Self::empty());
        }
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let offset = &ranges::normalized(offset)?;
        let size = &ranges::normalized(size)?;
        let offset_expr = expr(offset)?;
        let size_expr = expr(size)?;
        let mut operands = self.operands.clone();
        operands::remember(
            &mut operands,
            offset_expr.clone(),
            offset,
            domain,
            limits(domain),
        )?;
        operands::remember(
            &mut operands,
            size_expr.clone(),
            size,
            domain,
            limits(domain),
        )?;
        let bytes = slice::project(
            self,
            offset,
            size,
            &offset_expr,
            &size_expr,
            &operands,
            domain,
        );
        let root = self
            .root
            .slice(offset_expr, size_expr.clone(), limits(domain).into())?;
        Ok(Self {
            length: size.clone().with_expression(size_expr),
            root,
            allocations: BTreeSet::new(),
            operands,
            conditional: self.conditional.clone(),
            bytes,
            // Omitted/uncertified presentation cells and every padding branch
            // are covered without evaluating the authoritative graph.
            default: AbstractValue::unknown_byte(),
            memory: false,
            failure: None,
        })
    }
    /// Expand U256 logical memory after transfer has constrained symbolic ranges.
    /// The compatible max_bytes argument does not cap this lazy allocation.
    pub fn expand(
        &mut self,
        offset: &AbstractValue,
        size: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        if size.singleton() == Some(U256::ZERO) {
            return Ok(());
        }
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        ranges::validate_target(offset, size, self.memory)?;
        let offset = &ranges::normalized(offset)?;
        let size = &ranges::normalized(size)?;
        let allocation = (expr(offset)?, expr(size)?);
        ranges::check_allocation(&allocation, domain)?;
        if self.allocations.contains(&allocation) {
            self.check_existing(limits(domain))?;
            return Ok(());
        }
        let length = ranges::expanded(&self.length, offset, size, max_bytes, self.memory, domain)?;
        check_value(&length, limits(domain))?;
        if self.allocations.len().saturating_add(1) > limits(domain).max_nodes {
            return Err(StateError::Limit {
                nodes: self.allocations.len().saturating_add(1),
                depth: 1,
            }
            .into());
        }
        self.length = length;
        self.allocations.insert(allocation);
        Ok(())
    }
    /// Append a big-endian 32-byte write atomically after range validation.
    pub fn write_word(
        &mut self,
        offset: &AbstractValue,
        value: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        self.write(offset, value, 32, max_bytes, domain)
    }
    /// Append a low-byte write atomically after range validation.
    pub fn write_byte(
        &mut self,
        offset: &AbstractValue,
        value: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        self.write(offset, value, 1, max_bytes, domain)
    }
    /// The transfer keeps its original bulk fee; refresh pays its added work
    /// on that same ledger. False is mandatory Work, with no staged publication.
    pub(crate) fn write_word_budgeted(
        &mut self,
        offset: &AbstractValue,
        value: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<bool, RangeError> {
        self.write_budgeted(offset, value, 32, max_bytes, domain, budget)
    }
    pub(crate) fn write_byte_budgeted(
        &mut self,
        offset: &AbstractValue,
        value: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<bool, RangeError> {
        self.write_budgeted(offset, value, 1, max_bytes, domain, budget)
    }
    fn write_budgeted(
        &mut self,
        offset: &AbstractValue,
        value: &AbstractValue,
        width: usize,
        max_bytes: usize,
        domain: Domain,
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<bool, RangeError> {
        if !refresh::charge(budget, 0)? {
            return Ok(false);
        }
        let uniform = write::eligible(self, offset, value, width, domain);
        let mut out = self.staged_write(offset, value, width, max_bytes, domain)?;
        if uniform {
            write::refresh(&mut out, domain);
        } else if !refresh::run(&mut out, domain, budget)? {
            return Ok(false);
        }
        if !refresh::charge(budget, 0)? {
            return Ok(false);
        }
        *self = out;
        Ok(true)
    }
    fn write(
        &mut self,
        offset: &AbstractValue,
        value: &AbstractValue,
        width: usize,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        let uniform = write::eligible(self, offset, value, width, domain);
        let mut out = self.staged_write(offset, value, width, max_bytes, domain)?;
        if uniform {
            write::refresh(&mut out, domain);
        } else {
            out.refresh(domain);
        }
        *self = out;
        Ok(())
    }
    // Original staging is shared by both refreshes and the full-value oracle.
    fn staged_write(
        &self,
        offset: &AbstractValue,
        value: &AbstractValue,
        width: usize,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<Self, RangeError> {
        let offset = &ranges::normalized(offset)?;
        let mut out = self.clone();
        out.expand(offset, &constant(width), max_bytes, domain)?;
        let at = expr(offset)?;
        let val = expr(value)?;
        out.remember(at.clone(), offset, domain)?;
        out.remember(val.clone(), value, domain)?;
        // Normalized writes may keep a Zero/Exact root. Preserve the source of
        // its derived concrete bytes without materializing any logical range.
        if let Some(singleton) = value.singleton()
            && value.provenance() != AbstractValue::constant(singleton).provenance()
        {
            for index in 0..width {
                let byte = if width == 1 {
                    domain.apply(opcode::AND, &[value.clone(), constant(255)])
                } else {
                    domain.apply(opcode::BYTE, &[constant(index), value.clone()])
                };
                out.remember(expr(&byte)?, &byte, domain)?;
            }
        }
        out.root = if width == 32 {
            out.root.write_word(at, val, limits(domain).into())?
        } else {
            out.root.write_byte(at, val, limits(domain).into())?
        };
        let mut cached_all_offsets = offset.constants().is_some();
        for start in offset.constants().into_iter().flatten() {
            let Ok(start) = usize::try_from(*start) else {
                cached_all_offsets = false;
                continue;
            };
            for i in 0..width {
                if let Some(index) = start.checked_add(i) {
                    out.bytes.entry(index).or_insert_with(zero);
                } else {
                    cached_all_offsets = false;
                    break;
                }
            }
        }
        if !cached_all_offsets {
            let upper = if width == 1 {
                domain.apply(opcode::AND, &[value.clone(), constant(255)])
            } else {
                (0..32).fold(zero(), |upper, index| {
                    domain.join(
                        &upper,
                        &domain.apply(opcode::BYTE, &[constant(index), value.clone()]),
                    )
                })
            };
            out.default = domain.join(&out.default, &upper);
        }
        Ok(out)
    }
    /// One range node, capturing source before destination mutation (MCOPY).
    pub fn copy_from(
        &mut self,
        target_offset: &AbstractValue,
        source: &Self,
        source_offset: &AbstractValue,
        size: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        if size.singleton() == Some(U256::ZERO) {
            return Ok(());
        }
        if let Some(error) = self.failure.as_ref().or(source.failure.as_ref()) {
            return Err(error.clone());
        }
        let target_offset = &ranges::normalized(target_offset)?;
        let source_offset = &ranges::normalized(source_offset)?;
        let size = &ranges::normalized(size)?;
        let mut out = self.clone();
        out.expand(target_offset, size, max_bytes, domain)?;
        let dst = expr(target_offset)?;
        let at = expr(source_offset)?;
        let len = expr(size)?;
        if out.operands.is_empty() {
            out.operands = operands::import_empty(&source.operands, domain, limits(domain))?;
        } else {
            for (expression, value) in &source.operands {
                out.remember(expression.clone(), value, domain)?;
            }
        }
        out.conditional
            .extend(&source.conditional, limits(domain))?;
        out.remember(dst.clone(), target_offset, domain)?;
        out.remember(at.clone(), source_offset, domain)?;
        out.remember(len.clone(), size, domain)?;
        let view = ByteView::new(source.root.clone(), at, len.clone());
        out.root = out.root.copy(dst, view, len, limits(domain).into())?;
        // Presentation is bounded and never recursively reads the new root.
        // Keep coordinates even without a display certificate: procedure
        // summaries reread these positions using the complete root/context.
        out.bytes = copy::project(self, source, target_offset, source_offset, size, &out)?;
        out.default = AbstractValue::unknown_byte();
        *self = out;
        Ok(())
    }
    /// CALL output updates only min(requested,actual), while MSIZE uses requested.
    pub fn copy_return_data(
        &mut self,
        target_offset: &AbstractValue,
        returndata: &Self,
        requested: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        let target_offset = &ranges::normalized(target_offset)?;
        let requested = &ranges::normalized(requested)?;
        // Keep both fallible normalizations before skipping the staged clone;
        // expand, minimum and copy_from already do nothing for this size.
        if requested.singleton() == Some(U256::ZERO) {
            return Ok(());
        }
        let mut out = self.clone();
        out.expand(target_offset, requested, max_bytes, domain)?;
        let copied = ranges::minimum(requested, returndata.len(), domain)?;
        out.copy_from(
            target_offset,
            returndata,
            &zero(),
            &copied,
            max_bytes,
            domain,
        )?;
        *self = out;
        Ok(())
    }
    pub(crate) fn check_existing(&self, limits: ExprLimits) -> Result<(), RangeError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        self.root
            .join_guarded(&self.root, ExprId::constant(U256::ZERO), limits.into())?;
        self.conditional.check(limits)?;
        let mut result = Ok(());
        self.visit_values(&mut |value| {
            if result.is_ok() {
                result = check_value(value, limits).map_err(Into::into);
            }
        });
        result
    }
    /// Guarded merge paired with the selector used for stack values and relations.
    pub fn join_guarded(
        &self,
        other: &Self,
        domain: Domain,
        selector: ExprId,
        expression_limits: ExprLimits,
    ) -> Result<Self, RangeError> {
        self.check_existing(expression_limits)?;
        other.check_existing(expression_limits)?;
        if self == other {
            return Ok(self.clone());
        }
        let operands = operands::join_common(&self.operands, &other.operands, domain);
        let conditional = conditional::ConditionalFacts::join(
            (&self.operands, &self.conditional),
            (&other.operands, &other.conditional),
            &operands,
            selector.clone(),
            expression_limits,
        )?;
        let mut out = Self {
            length: self.length.join_guarded(
                &other.length,
                domain,
                selector.clone(),
                expression_limits,
            )?,
            bytes: BTreeMap::new(),
            default: domain.join(&self.default, &other.default),
            memory: self.memory && other.memory,
            allocations: self
                .allocations
                .intersection(&other.allocations)
                .cloned()
                .collect(),
            root: self
                .root
                .join_guarded(&other.root, selector, expression_limits.into())?,
            operands,
            conditional,
            failure: self.failure.clone().or_else(|| other.failure.clone()),
        };
        for index in self.bytes.keys().chain(other.bytes.keys()) {
            out.bytes.entry(*index).or_insert_with(zero);
        }
        out.refresh(domain);
        Ok(out)
    }
    /// Sound union. Identical wrappers retain the existing root and guard.
    /// Nonidentical states require a selector independent of their old guards.
    pub fn join(&self, other: &Self, domain: Domain) -> Self {
        if self == other {
            return self.clone();
        }
        let result = expr(&AbstractValue::top())
            .and_then(|selector| self.join_guarded(other, domain, selector, limits(domain)));
        result.unwrap_or_else(|error| {
            let mut out = Self::unknown();
            out.memory = self.memory && other.memory;
            out.failure = Some(error);
            out.length = domain.join(&self.length, &other.length);
            if out.length.symbolic_expression().is_none() {
                out.length = out.length.clone().with_expression(
                    expr(&AbstractValue::top()).expect("join fallback source identity"),
                );
            }
            out.root = ByteStateId::input(
                SourceId::fresh().expect("join fallback source identity"),
                out.length.symbolic_expression().expect("retained length"),
            );
            out
        })
    }
    /// Real Keccak for bounded exact data, otherwise a canonical content hash term.
    pub fn keccak(
        &self,
        domain: Domain,
        materialize_limit: usize,
    ) -> Result<AbstractValue, RangeError> {
        if self.length.singleton() != Some(U256::ZERO)
            && let Some(error) = &self.failure
        {
            return Err(error.clone());
        }
        let expression = ExprId::keccak(self.view(), limits(domain))?;
        if let Some(bytes) = self.exact_bytes_bounded(materialize_limit) {
            return Ok(
                AbstractValue::constant(U256::from_be_bytes(keccak256(bytes).0))
                    .with_expression(expression),
            );
        }
        Ok(AbstractValue::top().with_expression(expression))
    }
    /// Recover a completely known input under the default 1 MiB allocation cap.
    pub fn exact_bytes(&self) -> Option<Vec<u8>> {
        self.exact_bytes_bounded(1024 * 1024)
    }
    /// Materialization is a separate explicitly bounded operation.
    pub fn exact_bytes_bounded(&self, max_bytes: usize) -> Option<Vec<u8>> {
        if self.failure.is_some() {
            return None;
        }
        let length = usize::try_from(self.length.singleton()?).ok()?;
        if length > max_bytes {
            return None;
        }
        let materialize_work = length.saturating_mul(self.root.nodes().saturating_add(2));
        let materialize_limits = ExprLimits {
            max_nodes: materialize_work.max(self.view().nodes()),
            max_depth: self.root.depth().saturating_add(2),
        };
        match self.view().known_bytes(materialize_limits) {
            Ok(Some(bytes)) => return Some(bytes),
            Err(_) => return None,
            Ok(None) => {}
        }
        let mut output = Vec::new();
        output.try_reserve_exact(length).ok()?;
        let mut reader = numeric::Reader::with_context(
            conditional::Context::new(&self.operands, Some(&self.conditional)),
            Domain::default(),
        );
        for i in 0..length {
            if i % 256 == 0 && analysis_control::current_cancellation().is_cancelled() {
                return None;
            }
            let byte = reader
                .read(&self.root, &ExprId::constant(U256::from(i)))
                .singleton()?;
            if byte > U256::from(255) {
                return None;
            }
            output.push(byte.to::<u8>());
        }
        Some(output)
    }
    fn remember(
        &mut self,
        expression: ExprId,
        value: &AbstractValue,
        domain: Domain,
    ) -> Result<(), RangeError> {
        Ok(operands::remember(
            &mut self.operands,
            expression,
            value,
            domain,
            limits(domain),
        )?)
    }
    fn refresh(&mut self, domain: Domain) {
        let indices = self.bytes.keys().copied().collect::<Vec<_>>();
        for i in indices {
            self.bytes.insert(i, self.byte_at(i, domain));
        }
    }
    pub(crate) fn word_numeric_work(&self, _offset: &AbstractValue, domain: Domain) -> usize {
        // The original complete fee is also the fallback fee of a private plan.
        self.root
            .work_size()
            .saturating_mul(32)
            .saturating_mul(self.conditional.lookup_work(domain).saturating_add(1))
            .saturating_add(self.conditional.work_size().saturating_mul(32))
            .saturating_add(256 * 32)
    }
    pub(crate) fn project(&self, domain: Domain) -> Self {
        let mut out = self.clone();
        let project = |value: &mut AbstractValue| {
            let expression = value.expression().cloned();
            *value = domain.project(value);
            if let Some(expression) = expression {
                *value = value.clone().with_expression(expression);
            }
        };
        project(&mut out.length);
        project(&mut out.default);
        for value in out.bytes.values_mut().chain(out.operands.values_mut()) {
            project(value);
        }
        out.conditional = self
            .conditional
            .map(&mut |expression, value| {
                let mut value = value.cloned();
                if let Some(value) = &mut value {
                    project(value);
                }
                Ok((expression.clone(), value))
            })
            .expect("numeric projection is infallible");
        out
    }
    pub(crate) fn projection_work(&self, domain: Domain) -> usize {
        let mut work = self
            .bytes
            .values()
            .chain(self.operands.values())
            .chain([&self.length, &self.default])
            .fold(self.root.work_size(), |n, v| {
                n.saturating_add(domain.projection_work(v))
            });
        self.conditional.visit(&mut |_, value| {
            if let Some(value) = value {
                work = work.saturating_add(domain.projection_work(value));
            }
        });
        work
    }
}
