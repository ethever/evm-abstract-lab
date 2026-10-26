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
use std::cell::{Cell, RefCell};

const LENGTH: usize = 17;
const COPY_WORK: usize = 8192;
const PREFIX: usize = 37;

thread_local! {
    static LENGTH_CLONE_WORK: Cell<usize> = const { Cell::new(COPY_WORK) };
    static HOOKS: Cell<(usize, usize, usize)> = const { Cell::new((0, 0, 0)) };
    static CANCEL_PROJECT: RefCell<Option<CancelToken>> = const { RefCell::new(None) };
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct PrivateWord {
    value: NumericValue,
    #[serde(skip)]
    private: Vec<u8>,
    #[serde(skip)]
    clone_work: usize,
    #[serde(skip)]
    canonical_work: usize,
}

impl PrivateWord {
    fn plain(value: NumericValue) -> Self {
        Self {
            clone_work: value.clone_work().saturating_add(64),
            canonical_work: value.clone_work().saturating_add(64),
            value,
            private: Vec::new(),
        }
    }
}

impl Clone for PrivateWord {
    fn clone(&self) -> Self {
        if !self.private.is_empty() {
            HOOKS.with(|hooks| {
                let (copies, projects, canonical) = hooks.get();
                hooks.set((copies + 1, projects, canonical));
            });
        }
        Self {
            value: self.value.clone(),
            private: self.private.clone(),
            clone_work: self.clone_work,
            canonical_work: self.canonical_work,
        }
    }
}

impl std::fmt::Display for PrivateWord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(formatter)
    }
}

impl WordElement for PrivateWord {
    fn top() -> Self {
        Self::plain(NumericValue::top())
    }
    fn constant(value: U256) -> Self {
        let mut word = Self::plain(NumericValue::constant(value));
        if value == U256::from(LENGTH) {
            // Redundant private representation: observation stays cheap and
            // complete, while copying is an independent native operation.
            word.private = vec![0x17; 4096];
            word.clone_work = LENGTH_CLONE_WORK.with(Cell::get);
            word.canonical_work = word.clone_work;
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
            HOOKS.with(|hooks| {
                let (copies, projects, canonical) = hooks.get();
                hooks.set((copies, projects, canonical + 1));
            });
        }
        self.clone()
    }
}

#[derive(Debug)]
struct Words {
    projected_canonical_work: usize,
}

impl EvmNumericAlgebra for Words {
    type Element = PrivateWord;

    fn spec(&self) -> DomainSpec {
        ProductAlgebra::default().spec()
    }
    fn project_numeric(&self, value: &PrivateWord) -> PrivateWord {
        let mut projected = value.clone();
        if !value.private.is_empty() {
            HOOKS.with(|hooks| {
                let (copies, projects, canonical) = hooks.get();
                hooks.set((copies, projects + 1, canonical));
            });
            projected.canonical_work = self.projected_canonical_work;
            CANCEL_PROJECT.with(|token| {
                if let Some(token) = token.borrow_mut().take() {
                    token.cancel();
                }
            });
        }
        projected
    }
    fn join_numeric(&self, _left: &PrivateWord, _right: &PrivateWord) -> PrivateWord {
        PrivateWord::top()
    }
    fn widen_numeric(&self, old: &PrivateWord, next: &PrivateWord) -> PrivateWord {
        self.join_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &PrivateWord, next: &PrivateWord) -> PrivateWord {
        self.join_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &PrivateWord) -> NumericReduction<PrivateWord> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        _op: u8,
        _args: impl ExactSizeIterator<Item = &'a PrivateWord>,
    ) -> NumericReduction<PrivateWord> {
        NumericReduction::stable(PrivateWord::top())
    }
    fn operation_work<'a>(&self, _op: u8, _args: impl Iterator<Item = &'a PrivateWord>) -> usize {
        usize::MAX
    }
    fn projection_work(&self, _value: &PrivateWord) -> usize {
        1024
    }
    fn refinement_work(&self, _value: &PrivateWord, _facts: usize) -> usize {
        usize::MAX
    }
    fn summary_canonical_work(&self, value: &PrivateWord) -> usize {
        value.canonical_work
    }
}

fn source(clone_work: usize) -> EvmEnvironmentValues<PrivateWord> {
    LENGTH_CLONE_WORK.with(|work| work.set(clone_work));
    EvmEnvironmentValues {
        calldata: EvmByteArray::exact(&[0; LENGTH]),
        ..Default::default()
    }
}

#[test]
fn actual_environment_refuses_before_unfunded_length_and_intermediate_hooks() {
    for (clone_work, canonical_work, limit, expected_hooks) in [
        (usize::MAX, COPY_WORK, 1_000_000, (0, 0, 0)),
        (COPY_WORK, COPY_WORK, 4096, (0, 0, 0)),
        (COPY_WORK, usize::MAX, 1_000_000, (1, 1, 0)),
        (COPY_WORK, 1_000_000, 50_000, (1, 1, 0)),
    ] {
        let source = source(clone_work);
        let before = source.clone();
        let algebra = ValueAlgebra::new(Words {
            projected_canonical_work: canonical_work,
        });
        // The original scalar remains cheap to observe. The independently
        // expensive/MAX canonical quote belongs to the projected result.
        assert!(source.calldata.len().numeric().work_size() < COPY_WORK);
        assert!(algebra.borrowed().projection_work(source.calldata.len()) < COPY_WORK);
        HOOKS.with(|hooks| hooks.set((0, 0, 0)));
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
        HOOKS.with(|hooks| assert_eq!(hooks.get(), expected_hooks));
        assert_eq!(source, before);
    }
}

#[test]
fn actual_environment_funds_private_length_and_preserves_original_input() {
    let source = source(COPY_WORK);
    let before = source.clone();
    let algebra = ValueAlgebra::new(Words {
        projected_canonical_work: COPY_WORK,
    });
    HOOKS.with(|hooks| hooks.set((0, 0, 0)));
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
    HOOKS.with(|hooks| assert_eq!(hooks.get(), (2, 1, 1)));
    assert_eq!(source, before);
}

#[test]
fn actual_environment_checks_both_cancellation_owners_after_native_projection() {
    for captured in [false, true] {
        let source = source(COPY_WORK);
        let algebra = ValueAlgebra::new(Words {
            projected_canonical_work: COPY_WORK,
        });
        let original = Control::new(Default::default());
        let current = Control::new(Default::default());
        let token = if captured { &original } else { &current };
        CANCEL_PROJECT.with(|slot| *slot.borrow_mut() = Some(token.cancellation().clone()));
        HOOKS.with(|hooks| hooks.set((0, 0, 0)));
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
        HOOKS.with(|hooks| assert_eq!(hooks.get(), (1, 1, 0)));
    }
}
