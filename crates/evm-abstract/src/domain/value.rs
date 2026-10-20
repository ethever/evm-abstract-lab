//! An abstract value separates numeric constraints, source metadata and identity.

use super::{
    AlgebraRef, EvmNumericAlgebra, NumericValue, ValueAlgebra, WordElement,
    identity::{InputIdentity, RuntimeIdentity, Symbol, ValueIdentity},
    provenance::{Origin, OriginSet, Provenance},
    symbolic::{ExprError, ExprId, ExprLimits},
};
use alloy_primitives::U256;
use serde::{Serialize, Serializer, ser::SerializeMap};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

/// Machine value whose independent layers have explicit accessors.
/// Neither equal sources nor equal numeric summaries create an identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmValue<N: WordElement> {
    pub(super) numeric: N,
    pub(super) provenance: Provenance,
    pub(super) identity: ValueIdentity,
    pub(super) expression: Option<ExprId>,
    pub(super) symbolic_limit: bool,
}

impl<N: WordElement> EvmValue<N> {
    /// Numeric value with unknown source and no identity or expression.
    pub fn from_numeric(numeric: N) -> Self {
        Self {
            numeric,
            provenance: Provenance::top(),
            identity: ValueIdentity::none(),
            expression: None,
            symbolic_limit: false,
        }
    }
    /// Exact word; its source is a supplied constant.
    pub fn constant(value: U256) -> Self {
        let mut out = Self::from_numeric(N::constant(value));
        out.provenance = Provenance::constant();
        out
    }
    /// No numeric constraints, source facts, identity or expression.
    pub fn top() -> Self {
        Self::from_numeric(N::top())
    }
    /// Unknown byte with the format's numeric width guarantee.
    pub fn unknown_byte() -> Self {
        Self::from_numeric(N::unknown_byte())
    }
    /// Unsigned interval with no source or identity assertion.
    pub fn unsigned_range(lower: U256, upper: U256) -> Option<Self> {
        N::unsigned_range(lower, upper).map(Self::from_numeric)
    }
    /// Unknown address with a numeric width guarantee and Address source label.
    pub fn unknown_address() -> Self {
        let mut value = Self::from_numeric(N::unknown_address());
        value.provenance = Provenance::source(Origin::Address);
        value
    }
    /// Numeric constraints; callers must explicitly select this layer.
    pub fn numeric(&self) -> &N {
        &self.numeric
    }
    /// Complete value-copy envelope, retaining shared expression handles. Numeric
    /// deep copies use the actual carrier quote, not the default product tariff.
    pub fn clone_work(&self) -> usize {
        8usize
            .saturating_add(self.numeric.clone_work())
            .saturating_add(self.provenance.origins().work_size().saturating_mul(4))
    }
    /// Numeric singleton-query quote; evaluating an expression is separate work.
    pub fn singleton_work(&self) -> usize {
        self.numeric.singleton_work()
    }
    /// Replace numeric facts after a caller has proved a sound refinement; all
    /// source, identity and expression layers retain the same semantic value.
    pub(crate) fn with_numeric(mut self, numeric: N) -> Self {
        self.numeric = numeric;
        if self.singleton().is_some() {
            self.symbolic_limit = false;
        }
        self
    }
    /// Copy metadata around an already owned replacement without cloning and
    /// discarding the old numeric payload.
    pub(super) fn copy_metadata(&self, numeric: N) -> Self {
        Self {
            numeric,
            provenance: self.provenance.clone(),
            identity: self.identity,
            expression: self.expression.clone(),
            symbolic_limit: self.symbolic_limit,
        }
    }
    /// Construct only the result union, without cloning operand provenance into
    /// a temporary vector. Arithmetic discards the operands' must-address role.
    pub(super) fn transfer_provenance(args: &[Self]) -> Provenance {
        if args.iter().any(|value| value.provenance.origins().is_top()) {
            return Provenance::top();
        }
        let sources = std::iter::once(Origin::Arithmetic).chain(args.iter().flat_map(|value| {
            value
                .provenance
                .origins()
                .sources()
                .expect("known operand origins")
                .iter()
                .copied()
        }));
        Provenance::from_origins(OriginSet::from_sources(sources).expect("arithmetic origin"))
    }
    /// Possible sources and a code-address role, without equality identities.
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
    /// Scoped immutable input and temporary definition identities.
    pub fn identity(&self) -> &ValueIdentity {
        &self.identity
    }
    /// Persistent symbolic expression, if available.
    pub fn expression(&self) -> Option<&ExprId> {
        self.expression.as_ref()
    }
    /// Attach an already budgeted, valid symbolic expression.
    pub fn with_expression(mut self, expression: ExprId) -> Self {
        self.expression = Some(expression);
        self
    }
    /// Rename only a retained term; numeric facts and immutable input identity
    /// survive unchanged. A refused term leaves the complete value untouched.
    pub(crate) fn rename_fresh(
        &mut self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<(), ExprError> {
        if let Some(expression) = &self.expression {
            self.expression = Some(expression.rename_fresh(map, limits)?);
        }
        Ok(())
    }
    /// Forget optional symbolic precision without discarding numeric facts.
    pub fn forget_expression(&mut self) {
        self.expression = None;
    }
    /// Forget a result's optional expression after handling its retention limit.
    /// The caller must rule out inherited limits; all other value layers survive.
    pub(crate) fn forget_expression_after_limit(&mut self) {
        self.expression = None;
        self.symbolic_limit = false;
    }
    /// Obtain the value's exact expression. Numeric singletons need no stored
    /// expression handle; unknown values can only use an existing trusted term.
    pub fn symbolic_expression(&self) -> Option<ExprId> {
        self.singleton()
            .map(ExprId::constant)
            .or_else(|| self.expression.clone())
    }
    /// Numeric join plus a symbolic ITE using the machine's shared branch guard.
    /// Equal values keep their original representation. Anonymous values retain
    /// numeric facts while their missing symbolic correlation remains explicit.
    pub fn join_guarded_with<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
        selector: ExprId,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        if self == other {
            super::symbolic::state_check(
                self.expression.as_ref().map_or(1, ExprId::nodes),
                self.expression.as_ref().map_or(1, ExprId::depth),
                limits,
            )?;
            return Ok(self.clone());
        }
        let mut joined = domain.join(self, other);
        if let (Some(left), Some(right)) = (self.symbolic_expression(), other.symbolic_expression())
        {
            joined.expression = Some(ExprId::ite(selector, left, right, limits)?);
        }
        Ok(joined)
    }
    /// Symbolic construction exceeded a resource bound in this value or a
    /// numeric operation it depends on; numeric facts remain sound.
    pub fn symbolic_limit_reached(&self) -> bool {
        self.symbolic_limit
    }
    /// Mark a failed optional symbolic construction, retaining numeric facts
    /// and any previously valid expression for the same value.
    pub fn with_symbolic_limit(mut self) -> Self {
        self.symbolic_limit = true;
        self
    }

    /// Trusted alias test; source categories do not participate.
    pub fn same_identity(&self, other: &Self) -> bool {
        self.identity.same_identity(&other.identity)
    }
    /// Complete finite numeric candidates, when available.
    pub fn constants(&self) -> Option<&BTreeSet<U256>> {
        self.numeric.constants()
    }
    /// Finite candidate component.
    pub fn finite_constants(&self) -> &super::FiniteConstantSet {
        self.numeric.finite_constants()
    }
    /// Numeric mask component.
    pub fn known_bits(&self) -> &super::known_bits::KnownBits {
        self.numeric.known_bits()
    }
    /// Numeric interval component.
    pub fn interval(&self) -> &super::interval::Interval {
        self.numeric.interval()
    }
    /// Numeric congruence component.
    pub fn congruence(&self) -> &super::congruence::Congruence {
        self.numeric.congruence()
    }
    /// A concrete word is not ruled out by any numeric component.
    pub fn contains(&self, value: U256) -> bool {
        self.numeric.contains(value)
    }
    /// Exact numeric singleton, regardless of the finite component.
    pub fn singleton(&self) -> Option<U256> {
        self.numeric.singleton()
    }
    /// Zero is not excluded by numeric facts.
    pub fn may_be_zero(&self) -> bool {
        self.numeric.may_be_zero()
    }
    /// Numeric facts do not establish that the value must be zero.
    pub fn may_be_nonzero(&self) -> bool {
        self.numeric.may_be_nonzero()
    }
    /// Numeric copy cost plus source metadata and a cached expression-traversal bound.
    pub fn work_size(&self) -> usize {
        self.numeric
            .work_size()
            .saturating_add(self.provenance.origins().work_size())
            .saturating_add(usize::from(self.identity.input().is_some()))
            .saturating_add(self.expression.as_ref().map_or(0, ExprId::nodes))
    }
    /// Add a possible source without changing identity or expression.
    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.provenance = self.provenance.join(&Provenance::source(origin));
        self
    }
    pub(crate) fn set_origin(&mut self, origin: Origin) {
        self.provenance.set_origin(origin);
    }
    pub(crate) fn with_symbol(mut self, symbol: Symbol, scope: Option<u64>) -> Self {
        if self.singleton().is_none()
            && let Some(scope) = scope
        {
            self.identity = self.identity.with_input(InputIdentity::new(scope, symbol));
            if self.expression.is_none() {
                self.expression = Some(ExprId::input(scope, symbol));
            }
        }
        self
    }
    pub(crate) fn with_identity(mut self, scope: u64, definition: u32) -> Self {
        self.identity = self
            .identity
            .with_runtime(RuntimeIdentity::new(scope, definition));
        self
    }
    pub(crate) fn forget_identity(&mut self) {
        self.identity.forget_runtime();
    }
    pub(super) fn numeric_top(&self) -> bool {
        self.numeric.is_top()
    }
}
impl<N: WordElement> Serialize for EvmValue<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !N::SERIALIZE_NATIVE_PAYLOAD
            && self.numeric_top()
            && self.provenance == Provenance::top()
            && self.identity.persistent_empty()
            && self.expression.is_none()
            && !self.symbolic_limit
        {
            return serializer.serialize_str("Top");
        }
        let mut map = serializer.serialize_map(None)?;
        if N::SERIALIZE_NATIVE_PAYLOAD {
            map.serialize_entry("numeric", &self.numeric)?;
        }
        if let Some(constants) = self.numeric.constants() {
            map.serialize_entry("Constants", constants)?;
        }
        map.serialize_entry("known_bits", self.numeric.known_bits())?;
        map.serialize_entry("interval", self.numeric.interval())?;
        map.serialize_entry("congruence", self.numeric.congruence())?;
        map.serialize_entry("provenance", &self.provenance)?;
        map.serialize_entry("nonzero", &self.numeric.observation().nonzero())?;
        if !self.identity.persistent_empty() {
            map.serialize_entry("identity", &self.identity)?;
        }
        if let Some(expression) = &self.expression {
            map.serialize_entry("expression", expression)?;
        }
        if self.symbolic_limit {
            map.serialize_entry("symbolic_limit", &true)?;
        }
        map.end()
    }
}
impl<N: WordElement> fmt::Display for EvmValue<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        std::fmt::Display::fmt(&self.numeric, formatter)
    }
}

/// Default executable value, preserving legacy constructor inference.
pub type AbstractValue = EvmValue<NumericValue>;

impl EvmValue<NumericValue> {
    /// Default-domain compatibility entrypoint for a shared guarded join.
    pub fn join_guarded(
        &self,
        other: &Self,
        domain: super::Domain,
        selector: ExprId,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        self.join_guarded_with(
            other,
            ValueAlgebra::new(domain).borrowed(),
            selector,
            limits,
        )
    }
}

#[cfg(test)]
mod tests;
