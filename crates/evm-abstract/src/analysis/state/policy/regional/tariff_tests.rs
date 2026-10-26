//! Regional admission preserves native predicates and pays independent tariffs.

use super::*;
use crate::{
    Address, Fork, U256,
    analysis::{
        engine::policy::{PolicySite, ProcedureSummaryPolicy, RegionalSummaries},
        transfer,
    },
    domain::{
        DomainSpec, EvmValue, NumericReduction, SummaryConstruction, ValueAlgebra, WordObservation,
        symbolic::ExprId,
    },
    resource::WorkBudget,
    test_instrumentation::Counter,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use serde::Serialize;
use std::fmt;

mod normalization;
mod output;
mod process;

#[derive(Clone, Default, Debug)]
struct Receipt {
    clones: usize,
    singletons: usize,
    clone_quotes: usize,
    singleton_quotes: usize,
}

static RECEIPT: Counter<Option<Receipt>> = Counter::new(None);

fn observe(f: impl FnOnce(&mut Receipt)) {
    RECEIPT.update(|receipt| {
        if let Some(receipt) = receipt {
            f(receipt);
        }
    });
}

fn measure<T>(f: impl FnOnce() -> T) -> (T, Receipt) {
    let _scope = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        assert!(receipt.is_none());
        *receipt = Some(Receipt::default());
    });
    let output = f();
    (output, RECEIPT.replace(None).unwrap())
}

/// The exclusion is intentionally absent from WordObservation. Tariffs are
/// immutable type parameters, independent of the small retained structure.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Word<const QUERY: usize, const COPY: usize> {
    facts: WordObservation,
    excluded: Option<U256>,
}

impl<const Q: usize, const C: usize> Clone for Word<Q, C> {
    fn clone(&self) -> Self {
        observe(|receipt| receipt.clones += 1);
        Self {
            facts: self.facts.clone(),
            excluded: self.excluded,
        }
    }
}

impl<const Q: usize, const C: usize> fmt::Display for Word<Q, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} excluding {:?}", self.facts, self.excluded)
    }
}

impl<const Q: usize, const C: usize> WordElement for Word<Q, C> {
    fn top() -> Self {
        Self {
            facts: WordObservation::top(),
            excluded: None,
        }
    }
    fn constant(value: U256) -> Self {
        Self {
            facts: WordObservation::constant(value),
            excluded: None,
        }
    }
    fn from_observation(facts: &WordObservation) -> Self {
        Self {
            facts: facts.clone(),
            excluded: None,
        }
    }
    fn observation(&self) -> WordObservation {
        self.facts.clone()
    }
    fn requires_retained_binding(&self) -> bool {
        self.excluded.is_some()
    }
    fn finite_constants(&self) -> &crate::domain::FiniteConstantSet {
        self.facts.finite_constants()
    }
    fn known_bits(&self) -> &crate::domain::known_bits::KnownBits {
        self.facts.known_bits()
    }
    fn interval(&self) -> &crate::domain::interval::Interval {
        self.facts.interval()
    }
    fn congruence(&self) -> &crate::domain::congruence::Congruence {
        self.facts.congruence()
    }
    fn contains(&self, value: U256) -> bool {
        self.excluded != Some(value) && self.facts.contains(value)
    }
    fn singleton(&self) -> Option<U256> {
        observe(|receipt| receipt.singletons += 1);
        self.facts
            .singleton()
            .filter(|value| self.excluded != Some(*value))
    }
    fn is_top(&self) -> bool {
        self.excluded.is_none() && self.facts.is_top()
    }
    fn work_size(&self) -> usize {
        self.facts.work_size().saturating_add(1)
    }
    fn clone_work(&self) -> usize {
        observe(|receipt| receipt.clone_quotes += 1);
        C
    }
    fn singleton_work(&self) -> usize {
        observe(|receipt| receipt.singleton_quotes += 1);
        Q
    }
}

#[derive(Debug)]
struct Algebra<const Q: usize, const C: usize>;

impl<const Q: usize, const C: usize> EvmNumericAlgebra for Algebra<Q, C> {
    type Element = Word<Q, C>;
    fn spec(&self) -> DomainSpec {
        DomainSpec::default()
    }
    fn project_numeric(&self, value: &Self::Element) -> Self::Element {
        value.clone()
    }
    fn join_numeric(&self, left: &Self::Element, right: &Self::Element) -> Self::Element {
        if left == right {
            left.clone()
        } else {
            Word {
                facts: WordObservation::top(),
                excluded: (left.excluded == right.excluded)
                    .then_some(left.excluded)
                    .flatten(),
            }
        }
    }
    fn widen_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element {
        self.join_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element {
        self.join_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Self::Element) -> NumericReduction<Self::Element> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        _: u8,
        _: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> NumericReduction<Self::Element> {
        NumericReduction::stable(Word::top())
    }
    fn operation_work<'a>(&self, _: u8, _: impl Iterator<Item = &'a Self::Element>) -> usize {
        usize::MAX
    }
    fn literal_quote_work(&self, _: U256) -> usize {
        512
    }
    fn projection_work(&self, _: &Self::Element) -> usize {
        // The common payer separately funds the actual shared-data clone.
        0
    }
    fn refinement_work(&self, _: &Self::Element, _: usize) -> usize {
        usize::MAX
    }
    fn summary_join_work(&self, left: &Self::Element, right: &Self::Element, _: bool) -> usize {
        // Native Eq visits bounded fact storage; the equal branch shares it.
        left.work_size()
            .saturating_add(right.work_size())
            .saturating_mul(32)
            .saturating_add(C)
            .saturating_add(64)
    }
    fn summary_canonical_work(&self, value: &Self::Element) -> usize {
        // This carrier uses the trait's exact default: one faithful clone.
        value.clone_work()
    }
    fn summary_construction_work(&self, _: SummaryConstruction) -> usize {
        // Top/byte/range factories hold fixed observations and shared handles.
        512
    }
    fn summary_equal_work(&self, left: &Self::Element, right: &Self::Element) -> usize {
        left.work_size()
            .saturating_add(right.work_size())
            .saturating_mul(32)
    }
    fn summary_is_top_work(&self, _: &Self::Element) -> usize {
        64
    }
}

fn fixture<const Q: usize, const C: usize>(
    algebra: &ValueAlgebra<Algebra<Q, C>>,
) -> (
    EvmEntry<Word<Q, C>>,
    EvmPayloadSummary<Word<Q, C>>,
    ProcedureOutput<Word<Q, C>>,
) {
    let domain = algebra.borrowed();
    let owner = Address::repeat_byte(0x11);
    let private = || {
        EvmValue::from_numeric(Word {
            facts: WordObservation::top(),
            excluded: Some(U256::from(17)),
        })
    };
    let mut world = EvmWorld::<Word<Q, C>>::new(Fork::Osaka, "regional native tariff");
    let mut account = EvmAccount::from_hex("00", world.fork()).unwrap();
    account.balance = private();
    world.insert(owner, account).unwrap();
    let entry = EvmEntry::new(owner);
    let mut payload = transfer::initial(&world, &entry, domain).unwrap();
    let value = private().with_expression(ExprId::fresh().unwrap());
    payload.active_mut().stack = vec![value.clone(), value];
    payload.active_mut().key.stack_height = 2;
    let mut setup_budget = WorkBudget::new(10_000_000);
    let mut setup = PolicyContext::new(domain, &entry, &mut setup_budget);
    // Cold source construction is explicitly caller-funded: these controls
    // price the subsequent admission, including carriers refusing projection.
    let relative =
        crate::analysis::state::payload::form_normalized(&payload, &entry, domain).unwrap();
    let output = ProcedureOutput::from_summaries(
        EvmByteArray::empty().summary_projection(domain).unwrap(),
        payload.store.summary_projection(domain).unwrap(),
        &mut setup,
    )
    .unwrap();
    (entry, relative, output)
}

#[test]
fn affordable_native_regional_joins_preserve_unexported_predicates_and_aliases() {
    let algebra = ValueAlgebra::new(Algebra::<64, 64>);
    let (entry, input, output) = fixture(&algebra);
    for policy in [
        RegionalSummaries::new(),
        RegionalSummaries::with_early_known_admission(),
    ] {
        for acceleration in [Acceleration::Join, Acceleration::Widen] {
            let mut budget = WorkBudget::new(10_000_000);
            let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
            let (joined, receipt) = measure(|| {
                policy.admit_relative(&input, &input, PolicySite::Procedure, acceleration, &mut cx)
            });
            let joined = joined.unwrap();
            assert!(receipt.clones > 0 && receipt.clone_quotes > 0);
            assert!(receipt.singletons > 0 && receipt.singleton_quotes > 0);
            assert!(joined.frames[0].stack_aliases.same_class(0, 1));
            let scalar = &joined.frames[0].stack[0];
            assert_eq!(scalar.numeric().excluded, Some(U256::from(17)));
            assert!(!scalar.contains(U256::from(17)));
            assert!(scalar.numeric().observation().contains(U256::from(17)));
            let first = joined.instantiate(&mut cx).unwrap();
            let second = joined.instantiate(&mut cx).unwrap();
            assert_eq!(
                first.active().stack[0].expression(),
                first.active().stack[1].expression()
            );
            assert_ne!(
                first.active().stack[0].expression(),
                second.active().stack[0].expression()
            );
            assert!(!first.active().stack[0].contains(U256::from(17)));

            let joined_output = policy
                .admit_output(None, &output, acceleration, &mut cx)
                .unwrap();
            let store = joined_output
                .store()
                .instantiate(algebra.borrowed())
                .unwrap();
            let balance = store.read_balance(entry.address);
            assert!(!balance.contains(U256::from(17)));
            assert!(balance.numeric().observation().contains(U256::from(17)));
            assert!(budget.used() > 0 && budget.used() < 10_000_000);
        }
    }
}

fn refusal<const Q: usize, const C: usize>(limit: usize, deny_clone: bool) {
    let algebra = ValueAlgebra::new(Algebra::<Q, C>);
    let (entry, input, output) = fixture(&algebra);
    for relative in [true, false] {
        let mut budget = WorkBudget::new(limit);
        let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
        let (result, receipt) = measure(|| {
            if relative {
                input
                    .regional_join(&input, Acceleration::Join, &mut cx)
                    .map(|_| ())
            } else {
                output
                    .regional_join(&output, Acceleration::Join, &mut cx)
                    .map(|_| ())
            }
        });
        assert!(matches!(result, Err(MachineError::Work)), "{result:?}");
        if deny_clone {
            assert_eq!(receipt.clones, 0, "{receipt:?}");
        } else {
            assert_eq!(receipt.singletons, 0, "{receipt:?}");
        }
        assert_eq!(
            input.frames[0].stack[0].numeric().excluded,
            Some(U256::from(17))
        );
    }
}

#[test]
fn max_copy_tariff_refuses_before_any_native_copy() {
    refusal::<64, { usize::MAX }>(10_000_000, true);
}

#[test]
fn max_singleton_tariff_refuses_before_any_native_query() {
    refusal::<{ usize::MAX }, 64>(10_000_000, false);
}

#[test]
fn finite_copy_tariff_is_not_replaced_by_a_structural_envelope() {
    refusal::<64, 10_000_000>(100_000, true);
}

#[test]
fn finite_singleton_tariff_is_not_replaced_by_a_structural_envelope() {
    refusal::<10_000_000, 64>(100_000, false);
}

#[test]
fn zero_budget_rejects_before_native_queries_or_copies() {
    refusal::<64, 64>(0, true);
    refusal::<64, 64>(0, false);
}
