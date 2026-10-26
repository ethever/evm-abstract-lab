//! Exercise the actual original-environment boundary, including native length
//! copies and quotes on the policy projection's intermediate carrier.

use super::super::{Pools, environment, symbolic};
use evm_abstract::{
    U256,
    analysis::control::{CancelToken, Control},
    domain::{
        DomainSpec, EvmNumericAlgebra, NumericReduction, NumericValue, ProductAlgebra,
        ValueAlgebra, WordElement, WordObservation,
    },
    resource::WorkBudget,
    world::{EvmByteArray, EvmEnvironmentValues},
};
use evm_abstract_protocol as api;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const LENGTH: usize = 17;
const COPY_WORK: usize = 8192;
const PREFIX: usize = 37;

// The static literal constructor owns this probe. The fixture and algebra
// borrow its handle from the actual length; clones/intermediates retain it.
// Mutable observations and cancellation controls are not carrier metadata.
#[derive(Debug, Default)]
struct Hooks {
    calls: Cell<(usize, usize, usize)>,
    cancel_project: RefCell<Option<CancelToken>>,
}

#[derive(Debug, serde::Serialize)]
struct PrivateWord<const LENGTH_CLONE_WORK: usize> {
    value: NumericValue,
    #[serde(skip)]
    private: Vec<u8>,
    #[serde(skip)]
    clone_work: usize,
    #[serde(skip)]
    canonical_work: usize,
    #[serde(skip)]
    hooks: Option<Rc<Hooks>>,
}

impl<const LENGTH_CLONE_WORK: usize> PartialEq for PrivateWord<LENGTH_CLONE_WORK> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
            && self.private == other.private
            && self.clone_work == other.clone_work
            && self.canonical_work == other.canonical_work
    }
}
impl<const LENGTH_CLONE_WORK: usize> Eq for PrivateWord<LENGTH_CLONE_WORK> {}

impl<const LENGTH_CLONE_WORK: usize> PrivateWord<LENGTH_CLONE_WORK> {
    fn plain(value: NumericValue) -> Self {
        Self {
            clone_work: value.clone_work().saturating_add(64),
            canonical_work: value.clone_work().saturating_add(64),
            value,
            private: Vec::new(),
            hooks: None,
        }
    }
}

impl<const LENGTH_CLONE_WORK: usize> Clone for PrivateWord<LENGTH_CLONE_WORK> {
    fn clone(&self) -> Self {
        if !self.private.is_empty() {
            let hooks = self.hooks.as_ref().expect("private literal owns hooks");
            let (copies, projects, canonical) = hooks.calls.get();
            hooks.calls.set((copies + 1, projects, canonical));
        }
        Self {
            value: self.value.clone(),
            private: self.private.clone(),
            clone_work: self.clone_work,
            canonical_work: self.canonical_work,
            hooks: self.hooks.as_ref().map(Rc::clone),
        }
    }
}

impl<const LENGTH_CLONE_WORK: usize> std::fmt::Display for PrivateWord<LENGTH_CLONE_WORK> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(formatter)
    }
}

impl<const LENGTH_CLONE_WORK: usize> WordElement for PrivateWord<LENGTH_CLONE_WORK> {
    fn top() -> Self {
        Self::plain(NumericValue::top())
    }
    fn constant(value: U256) -> Self {
        let mut word = Self::plain(NumericValue::constant(value));
        if value == U256::from(LENGTH) {
            // Redundant private representation: observation stays cheap and
            // complete, while copying is an independent native operation.
            word.private = vec![0x17; 4096];
            word.clone_work = LENGTH_CLONE_WORK;
            word.canonical_work = word.clone_work;
            word.hooks = Some(Rc::new(Hooks::default()));
        }
        word
    }
    fn from_observation(observation: &WordObservation) -> Self {
        Self::plain(NumericValue::from_observation(observation))
    }
    fn observation(&self) -> WordObservation {
        self.value.observation()
    }
    fn requires_retained_binding(&self) -> bool {
        false
    }
    fn is_top(&self) -> bool {
        self.value.is_top()
    }
    fn work_size(&self) -> usize {
        self.value.work_size().saturating_add(64)
    }
    fn clone_work(&self) -> usize {
        self.clone_work
    }
    fn canonical_for_summary(&self) -> Self {
        if !self.private.is_empty() {
            let hooks = self.hooks.as_ref().expect("private literal owns hooks");
            let (copies, projects, canonical) = hooks.calls.get();
            hooks.calls.set((copies, projects, canonical + 1));
        }
        self.clone()
    }
}

#[derive(Debug)]
struct Words<const LENGTH_CLONE_WORK: usize> {
    projected_canonical_work: usize,
    hooks: Rc<Hooks>,
}

impl<const LENGTH_CLONE_WORK: usize> EvmNumericAlgebra for Words<LENGTH_CLONE_WORK> {
    type Element = PrivateWord<LENGTH_CLONE_WORK>;

    fn spec(&self) -> DomainSpec {
        ProductAlgebra::default().spec()
    }
    fn project_numeric(&self, value: &Self::Element) -> Self::Element {
        let mut projected = value.clone();
        if !value.private.is_empty() {
            assert!(Rc::ptr_eq(
                value.hooks.as_ref().expect("private literal owns hooks"),
                &self.hooks,
            ));
            let (copies, projects, canonical) = self.hooks.calls.get();
            self.hooks.calls.set((copies, projects + 1, canonical));
            projected.canonical_work = self.projected_canonical_work;
            if let Some(token) = self.hooks.cancel_project.borrow_mut().take() {
                token.cancel();
            }
        }
        projected
    }
    fn join_numeric(&self, _left: &Self::Element, _right: &Self::Element) -> Self::Element {
        PrivateWord::top()
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
        _op: u8,
        _args: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> NumericReduction<Self::Element> {
        NumericReduction::stable(PrivateWord::top())
    }
    fn operation_work<'a>(&self, _op: u8, _args: impl Iterator<Item = &'a Self::Element>) -> usize {
        usize::MAX
    }
    fn projection_work(&self, _value: &Self::Element) -> usize {
        1024
    }
    fn refinement_work(&self, _value: &Self::Element, _facts: usize) -> usize {
        usize::MAX
    }
    fn summary_canonical_work(&self, value: &Self::Element) -> usize {
        value.canonical_work
    }
}

fn source<const LENGTH_CLONE_WORK: usize>() -> (
    EvmEnvironmentValues<PrivateWord<LENGTH_CLONE_WORK>>,
    Rc<Hooks>,
) {
    let source: EvmEnvironmentValues<PrivateWord<LENGTH_CLONE_WORK>> = EvmEnvironmentValues {
        calldata: EvmByteArray::exact(&[0; LENGTH]),
        ..Default::default()
    };
    let hooks = Rc::clone(
        source
            .calldata
            .len()
            .numeric()
            .hooks
            .as_ref()
            .expect("static length constructor owns hooks"),
    );
    (source, hooks)
}

#[test]
fn actual_environment_refuses_before_unfunded_length_and_intermediate_hooks() {
    fn refuses<const LENGTH_CLONE_WORK: usize>(
        canonical_work: usize,
        limit: usize,
        expected_hooks: (usize, usize, usize),
    ) {
        let (source, hooks) = source::<LENGTH_CLONE_WORK>();
        let before = source.clone();
        let algebra = ValueAlgebra::new(Words {
            projected_canonical_work: canonical_work,
            hooks: Rc::clone(&hooks),
        });
        // The original scalar remains cheap to observe. The independently
        // expensive/MAX canonical quote belongs to the projected result.
        assert!(source.calldata.len().numeric().work_size() < COPY_WORK);
        assert!(algebra.borrowed().projection_work(source.calldata.len()) < COPY_WORK);
        hooks.calls.set((0, 0, 0));
        let control = Control::default();
        let mut budget = WorkBudget::new(limit);
        assert!(budget.charge(PREFIX));
        let mut pools = Pools::new(&control);
        pools.symbolic = symbolic::Pool::minimum(&control, &mut budget);
        let error = environment(&source, &algebra, &mut pools).unwrap_err();
        let api::ErrorDetails::Export(failure) = error.details else {
            panic!("missing typed projection refusal")
        };
        assert_eq!(failure.kind, api::ExportFailureKind::Work);
        assert_eq!(failure.usage, pools.symbolic.usage);
        let paid = pools.symbolic.usage.work;
        assert!(paid > 0, "retain the successful prefix on refusal");
        assert!(pools.symbolic.graph.nodes.is_empty());
        drop(pools);
        assert_eq!(budget.used(), PREFIX + paid as usize);
        assert_eq!(budget.limit(), limit);
        assert_eq!(hooks.calls.get(), expected_hooks);
        assert_eq!(source, before);
    }
    refuses::<{ usize::MAX }>(COPY_WORK, 1_000_000, (0, 0, 0));
    refuses::<COPY_WORK>(COPY_WORK, 4096, (0, 0, 0));
    refuses::<COPY_WORK>(usize::MAX, 1_000_000, (1, 1, 0));
    refuses::<COPY_WORK>(1_000_000, 50_000, (1, 1, 0));
}

#[test]
fn actual_environment_funds_private_length_and_preserves_original_input() {
    let (source, hooks) = source::<COPY_WORK>();
    let before = source.clone();
    let algebra = ValueAlgebra::new(Words {
        projected_canonical_work: COPY_WORK,
        hooks: Rc::clone(&hooks),
    });
    hooks.calls.set((0, 0, 0));
    let control = Control::default();
    let mut budget = WorkBudget::new(1_000_000);
    assert!(budget.charge(PREFIX));
    let mut pools = Pools::new(&control);
    pools.symbolic = symbolic::Pool::minimum(&control, &mut budget);
    let projected = environment(&source, &algebra, &mut pools).unwrap();
    assert_eq!(
        projected.calldata.length.constants,
        Some(vec!["0x11".into()])
    );
    assert_eq!(
        projected.input_scope,
        source.input_scope_id().map(api::DecimalU64)
    );
    assert!(matches!(
        projected.calldata.content,
        api::MinimumByteContent::Zero
    ));
    let paid = pools.symbolic.usage.work;
    assert!(pools.symbolic.graph.nodes.is_empty());
    drop(pools);
    assert_eq!(budget.used(), PREFIX + paid as usize);
    assert_eq!(hooks.calls.get(), (2, 1, 1));
    assert_eq!(source, before);
}

#[test]
fn actual_environment_checks_both_cancellation_owners_after_native_projection() {
    for captured in [false, true] {
        let (source, hooks) = source::<COPY_WORK>();
        let algebra = ValueAlgebra::new(Words {
            projected_canonical_work: COPY_WORK,
            hooks: Rc::clone(&hooks),
        });
        let original = Control::new(Default::default());
        let current = Control::new(Default::default());
        let token = if captured { &original } else { &current };
        *hooks.cancel_project.borrow_mut() = Some(token.cancellation().clone());
        hooks.calls.set((0, 0, 0));
        let mut budget = original.scope(|| WorkBudget::new(1_000_000));
        assert!(budget.charge(PREFIX));
        let mut pools = Pools::new(&current);
        pools.symbolic = symbolic::Pool::minimum(&current, &mut budget);
        let error = environment(&source, &algebra, &mut pools).unwrap_err();
        let api::ErrorDetails::Export(failure) = error.details else {
            panic!("missing typed cancellation")
        };
        assert_eq!(failure.kind, api::ExportFailureKind::Cancelled);
        let paid = pools.symbolic.usage.work;
        assert_eq!(failure.usage.work, paid);
        drop(pools);
        assert_eq!(budget.used(), PREFIX + paid as usize);
        assert_eq!(hooks.calls.get(), (1, 1, 0));
    }
}
