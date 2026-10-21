use super::*;
use crate::domain::{
    DomainSpec, EvmNumericAlgebra, EvmValue, FiniteConstantSet, NumericReduction, NumericValue,
    ValueAlgebra, WordElement,
    cooperation::{KeepBounds, KeepMasks, ProductAlgebra},
};
use std::{cell::Cell, collections::BTreeSet, num::NonZeroUsize};

mod hidden;

fn captured_domain() -> ProductAlgebra {
    ProductAlgebra::from_spec(
        DomainSpec::new(
            NonZeroUsize::new(512).unwrap(),
            NonZeroUsize::new(16).unwrap(),
            NonZeroUsize::new(4096).unwrap(),
        )
        .with_relations(RelationLimits {
            enabled: true,
            max_constraints: 2048,
            max_nodes: 16384,
            max_depth: 256,
        }),
    )
}

fn captured_bytes() -> [NumericValue; 32] {
    let mut bytes = std::array::from_fn(|_| NumericValue::unknown_byte());
    bytes[31] = crate::domain::product::finite(
        FiniteConstantSet::try_from_values((0..8).map(|v| U256::from(v * 32)).collect()).unwrap(),
    );
    bytes
}

fn ordinary<D: EvmNumericAlgebra>(
    algebra: &ValueAlgebra<D>,
    bytes: &[D::Element; 32],
) -> D::Element {
    let mut word = D::Element::constant(U256::ZERO);
    for (i, byte) in bytes.iter().enumerate() {
        let shifted = algebra.apply(
            opcode::SHL,
            &[
                EvmValue::constant(U256::from((31 - i) * 8)),
                EvmValue::from_numeric(byte.clone()),
            ],
        );
        word = algebra
            .apply(
                opcode::OR,
                &[
                    EvmValue::from_numeric(word),
                    EvmValue::from_numeric(shifted.numeric().clone()),
                ],
            )
            .numeric()
            .clone();
    }
    word
}

fn pack<D: EvmNumericAlgebra>(
    algebra: &ValueAlgebra<D>,
    bytes: [D::Element; 32],
    budget: &mut WorkBudget,
) -> super::super::super::LocalResult<D::Element> {
    let limits = algebra.spec().relations();
    let mut work = Work {
        remaining: limits.max_nodes,
        cancellation: analysis_control::current_cancellation(),
        ledger: Some(budget),
    };
    Engine::new(&mut work, &limits, algebra.borrowed()).pack_word(bytes)
}

/// An injected non-Copy policy with observable hooks and independently chosen
/// admission. Delegation here is explicit test policy code, never engine logic.
#[derive(Debug)]
struct Probe {
    domain: ProductAlgebra,
    fee: Option<usize>,
    decline: bool,
    packs: Cell<usize>,
    operations: Cell<usize>,
    cancel: Option<analysis_control::Cancellation>,
}
impl Probe {
    fn new(fee: Option<usize>) -> Self {
        Self {
            domain: captured_domain(),
            fee,
            decline: false,
            packs: Cell::new(0),
            operations: Cell::new(0),
            cancel: None,
        }
    }
}
impl EvmNumericAlgebra for Probe {
    type Element = NumericValue;
    fn spec(&self) -> DomainSpec {
        self.domain.spec()
    }
    fn project_numeric(&self, v: &NumericValue) -> NumericValue {
        self.domain.project_numeric(v)
    }
    fn join_numeric(&self, a: &NumericValue, b: &NumericValue) -> NumericValue {
        self.domain.join_numeric(a, b)
    }
    fn widen_numeric(&self, a: &NumericValue, b: &NumericValue) -> NumericValue {
        self.domain.widen_numeric(a, b)
    }
    fn widen_procedure_numeric(&self, a: &NumericValue, b: &NumericValue) -> NumericValue {
        self.domain.widen_procedure_numeric(a, b)
    }
    fn reduce_numeric(&self, v: &NumericValue) -> NumericReduction<NumericValue> {
        self.domain.reduce_numeric(v)
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a NumericValue>,
    ) -> NumericReduction<NumericValue> {
        self.operations.set(self.operations.get() + 1);
        self.domain.apply_numeric(op, args)
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a NumericValue>) -> usize {
        EvmNumericAlgebra::operation_work(&self.domain, op, args)
    }
    fn literal_quote_work(&self, value: U256) -> usize {
        self.domain.literal_quote_work(value)
    }
    fn projection_work(&self, v: &NumericValue) -> usize {
        EvmNumericAlgebra::projection_work(&self.domain, v)
    }
    fn refinement_work(&self, v: &NumericValue, facts: usize) -> usize {
        EvmNumericAlgebra::refinement_work(&self.domain, v, facts)
    }
    fn pack_word_work(&self, bytes: &[&NumericValue; 32]) -> Option<usize> {
        self.fee.map(|minimum| {
            minimum.max(
                self.domain
                    .pack_word_work(bytes)
                    .unwrap()
                    .saturating_add(64),
            )
        })
    }
    fn pack_word_numeric(&self, bytes: &[&NumericValue; 32]) -> Option<NumericValue> {
        self.packs.set(self.packs.get() + 1);
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
        if self.decline {
            None
        } else {
            self.domain.pack_word_numeric(bytes)
        }
    }
}

#[test]
fn captured_word_reaches_paid_packing_through_source_byte_facts() {
    let domain = captured_domain();
    let algebra = ValueAlgebra::new(domain);
    let limits = domain.spec().relations();
    let bytes = captured_bytes();
    let input = ByteStateId::input(SourceId::fresh().unwrap(), c(32));
    let byte = ExprId::byte_read(input.clone(), c(31), limits.expressions()).unwrap();
    let view = ByteView::new(input, c(0), c(32));
    let mut budget = WorkBudget::new(20_000_000);
    let mut work = Work {
        remaining: limits.max_nodes,
        cancellation: analysis_control::current_cancellation(),
        ledger: Some(&mut budget),
    };
    let mut engine = Engine::new(&mut work, &limits, algebra.borrowed());
    engine.has_source_bytes = true;
    engine.learn(&byte, &bytes[31]).unwrap();
    let result = engine.word(&view, &c(0)).unwrap();
    assert_eq!(result, ordinary(&algebra, &bytes));
    assert_eq!(retained(&engine.facts, &byte), Some(bytes[31].clone()));
    assert!(!budget.exhausted());
}

#[test]
fn default_captured_cube_fees_are_atomic_and_fund_the_complete_result() {
    let algebra = ValueAlgebra::new(captured_domain());
    for bytes in [
        captured_bytes(),
        std::array::from_fn(|_| NumericValue::unknown_byte()),
    ] {
        let fee = algebra
            .pack_word_work(&std::array::from_fn(|i| &bytes[i]))
            .unwrap();
        let expected = ordinary(&algebra, &bytes);
        let mut refused = WorkBudget::new(32 + fee - 1);
        assert!(matches!(
            pack(&algebra, bytes.clone(), &mut refused),
            Err(Stop::Pending(QueryReason::ExpressionLimit))
        ));
        assert_eq!(refused.used(), 32);
        assert!(!refused.exhausted());
        let mut paid = WorkBudget::new(32 + fee);
        assert_eq!(pack(&algebra, bytes, &mut paid).unwrap(), expected);
        assert_eq!(paid.used(), 32 + fee);
    }
}

#[test]
fn borrowed_policy_controls_opt_out_and_paid_decline_preserves_all_operations() {
    for fee in [None, Some(1024)] {
        let mut probe = Probe::new(fee);
        probe.decline = true;
        let algebra = ValueAlgebra::new(&probe);
        let bytes = captured_bytes();
        let expected = ordinary(&algebra, &bytes);
        probe.operations.set(0);
        let mut budget = WorkBudget::new(1_000_000_000_000);
        assert_eq!(pack(&algebra, bytes, &mut budget).unwrap(), expected);
        assert_eq!(probe.operations.get(), 64);
        assert_eq!(probe.packs.get(), usize::from(fee.is_some()));
    }
}

#[test]
fn custom_product_rules_decline_default_packing_and_keep_their_own_result() {
    let spec = DomainSpec::new(
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(16).unwrap(),
        NonZeroUsize::new(4096).unwrap(),
    );
    let policy = ProductAlgebra::new(spec, KeepMasks, KeepBounds);
    let algebra = ValueAlgebra::new(&policy);
    let mut bytes = std::array::from_fn(|_| NumericValue::constant(U256::ZERO));
    bytes[0] = NumericValue::unknown_byte();
    let refs = std::array::from_fn(|i| &bytes[i]);
    assert_eq!(algebra.pack_word_work(&refs), None);
    assert!(algebra.pack_word_numeric(&refs).is_none());
    let expected = ordinary(&algebra, &bytes);
    let default = ProductAlgebra::from_spec(spec)
        .pack_word_numeric(&refs)
        .unwrap();
    assert_ne!(
        expected, default,
        "custom cooperation must remain observable"
    );
    let mut budget = WorkBudget::new(20_000_000);
    assert_eq!(pack(&algebra, bytes, &mut budget).unwrap(), expected);
    assert!(!budget.exhausted());
}

#[test]
fn hook_admission_is_paid_before_execution_and_refusal_never_runs_fallback() {
    for fee in [1024, usize::MAX] {
        let bytes: [_; 32] = std::array::from_fn(|_| NumericValue::constant(U256::ZERO));
        let probe = Probe::new(Some(fee));
        let actual_fee = probe
            .pack_word_work(&std::array::from_fn(|i| &bytes[i]))
            .unwrap();
        for available in [31, 32, actual_fee.saturating_add(32).saturating_sub(1)] {
            let probe = Probe::new(Some(fee));
            let algebra = ValueAlgebra::new(&probe);
            let mut budget = WorkBudget::new(available);
            let bytes = std::array::from_fn(|_| NumericValue::constant(U256::ZERO));
            assert!(matches!(
                pack(&algebra, bytes, &mut budget),
                Err(Stop::Pending(QueryReason::ExpressionLimit))
            ));
            assert_eq!(probe.packs.get(), 0);
            assert_eq!(probe.operations.get(), 0);
            assert_eq!(budget.used(), if available < 32 { 0 } else { 32 });
            assert!(!budget.exhausted());
        }
    }
    let probe = Probe::new(Some(1024));
    let algebra = ValueAlgebra::new(&probe);
    let bytes = std::array::from_fn(|_| NumericValue::constant(U256::ZERO));
    let fee = algebra
        .pack_word_work(&std::array::from_fn(|i| &bytes[i]))
        .unwrap();
    let mut budget = WorkBudget::new(32 + fee);
    assert_eq!(
        pack(&algebra, bytes, &mut budget).unwrap(),
        NumericValue::constant(U256::ZERO)
    );
    assert_eq!(probe.packs.get(), 1);
    assert_eq!(probe.operations.get(), 0);
    assert_eq!(budget.used(), 32 + fee);
}

#[test]
fn refused_and_cancelled_word_queries_publish_no_memo_or_term() {
    for case in 0..3 {
        let cancellation = analysis_control::Cancellation::new();
        let mut probe = Probe::new(Some(if case == 0 { usize::MAX } else { 1024 }));
        if case == 2 {
            probe.cancel = Some(cancellation.clone());
        }
        let algebra = ValueAlgebra::new(&probe);
        let limits = limits();
        let read = ExprId::word_read(
            ByteView::new(
                ByteStateId::input(SourceId::fresh().unwrap(), c(32)),
                c(0),
                c(32),
            ),
            c(0),
            limits.expressions(),
        )
        .unwrap();
        let mut budget = WorkBudget::new(20_000_000);
        let mut work = Work {
            remaining: limits.max_nodes,
            cancellation: cancellation.clone(),
            ledger: Some(&mut budget),
        };
        let mut engine = Engine::new(&mut work, &limits, algebra.borrowed());
        let saved = ExprId::fresh().unwrap();
        engine
            .learn(&saved, &NumericValue::constant(U256::from(7)))
            .unwrap();
        if case == 1 {
            cancellation.cancel();
        }
        assert!(
            matches!(engine.eval(&read), Err(Stop::Pending(reason)) if reason == if case == 0 { QueryReason::ExpressionLimit } else { QueryReason::Cancelled })
        );
        assert_eq!(probe.packs.get(), usize::from(case == 2));
        assert_eq!(probe.operations.get(), 0);
        assert!(retained(&engine.memo, &read).is_none());
        assert!(retained(&engine.terms, &read).is_none());
        assert_eq!(
            retained(&engine.facts, &saved),
            Some(NumericValue::constant(U256::from(7)))
        );
        assert!(!budget.exhausted());
    }
}
