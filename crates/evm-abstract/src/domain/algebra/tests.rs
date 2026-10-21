use super::{
    DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, U256, ValueAlgebra, WordElement,
    WorkExhausted, opcode, symbolic,
};
use crate::domain::{
    AlgebraRef, FiniteConstantSet, FiniteSetAlgebra, NumericValue, ProductAlgebra, WordObservation,
    facts::FactError,
    relational::{EvmValueQuery, RelationLimits, RelationState},
};
use serde::Serialize;
use std::{cell::Cell, rc::Rc};

/// Independent carrier with an intentionally unexported oddness guarantee.
/// It exposes no finite/bits/interval/congruence representation at all.
#[derive(Debug, Serialize)]
struct Word {
    exact: Option<U256>,
    odd: bool,
    #[serde(skip)]
    copies: Rc<Cell<usize>>,
    #[serde(skip)]
    payload: Vec<u8>,
}
impl Clone for Word {
    fn clone(&self) -> Self {
        self.copies.set(self.copies.get() + 1);
        Self {
            exact: self.exact,
            odd: self.odd,
            copies: Rc::clone(&self.copies),
            payload: self.payload.clone(),
        }
    }
}
impl PartialEq for Word {
    fn eq(&self, other: &Self) -> bool {
        self.exact == other.exact && self.odd == other.odd
    }
}
impl Eq for Word {}
impl std::fmt::Display for Word {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}/odd={}", self.exact, self.odd)
    }
}
impl WordElement for Word {
    fn top() -> Self {
        Self {
            exact: None,
            odd: false,
            copies: Rc::new(Cell::new(0)),
            payload: Vec::new(),
        }
    }
    fn constant(value: U256) -> Self {
        Self {
            exact: Some(value),
            ..Self::top()
        }
    }
    fn from_observation(o: &WordObservation) -> Self {
        o.singleton().map_or_else(Self::top, Self::constant)
    }
    fn observation(&self) -> WordObservation {
        self.exact
            .map_or_else(WordObservation::top, WordObservation::constant)
    }
    fn contains(&self, v: U256) -> bool {
        self.exact.is_none_or(|x| x == v) && (!self.odd || v.bit(0))
    }
    fn singleton(&self) -> Option<U256> {
        self.exact
    }
    fn is_top(&self) -> bool {
        self.exact.is_none() && !self.odd
    }
    fn work_size(&self) -> usize {
        64usize.saturating_add(self.payload.len())
    }
}

/// Neither Clone nor Copy: every view must retain this original policy object.
#[derive(Debug)]
struct Policy {
    spec: DomainSpec,
    calls: Cell<usize>,
    byte_calls: Cell<usize>,
}
impl Policy {
    fn new() -> Self {
        let limits = RelationLimits {
            enabled: false,
            ..RelationLimits::default()
        };
        Self {
            spec: DomainSpec::default().with_relations(limits),
            calls: Cell::new(0),
            byte_calls: Cell::new(0),
        }
    }
}
impl EvmNumericAlgebra for Policy {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        self.spec
    }
    fn project_numeric(&self, v: &Word) -> Word {
        v.clone()
    }
    fn join_numeric(&self, a: &Word, b: &Word) -> Word {
        if a == b {
            a.clone()
        } else {
            Word {
                odd: a.odd && b.odd,
                ..Word::top()
            }
        }
    }
    fn widen_numeric(&self, a: &Word, b: &Word) -> Word {
        self.join_numeric(a, b)
    }
    fn widen_procedure_numeric(&self, a: &Word, b: &Word) -> Word {
        self.join_numeric(a, b)
    }
    fn reduce_numeric(&self, v: &Word) -> NumericReduction<Word> {
        NumericReduction::stable(v.clone())
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        self.calls.set(self.calls.get() + 1);
        let expected = match op {
            opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
            opcode::ADDMOD | opcode::MULMOD => 3,
            _ => 2,
        };
        if !matches!(op,0x01..=0x0b|0x10..=0x1e) || args.len() != expected {
            return NumericReduction::stable(Word::top());
        }
        let value = if let Some(exact) = args.map(Word::singleton).collect::<Option<Vec<_>>>() {
            Word::constant(crate::domain::evaluate(
                op,
                exact[0],
                exact.get(1).copied().unwrap_or(U256::ZERO),
                exact.get(2).copied().unwrap_or(U256::ZERO),
            ))
        } else {
            Word::top()
        };
        NumericReduction::stable(value)
    }
    fn observe_byte_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> Word {
        self.byte_calls.set(self.byte_calls.get() + 1);
        self.apply_numeric(op, args).value
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Word>) -> usize {
        // Pay both the policy's concrete evaluation and a possible retained
        // expression fold. EXP has up to 512 fixed-width multiplications;
        // widened modular arithmetic and division need their own envelopes.
        let evaluations = match op {
            opcode::EXP => 2 * 16_384,
            opcode::ADDMOD | opcode::MULMOD => 2 * 4096,
            opcode::DIV | opcode::SDIV | opcode::MOD | opcode::SMOD => 2 * 1024,
            _ => 2 * 64,
        };
        args.fold(1024usize + evaluations, |n, v| {
            n.saturating_add(v.singleton_work())
        })
    }
    fn projection_work(&self, _: &Word) -> usize {
        0
    }
    fn refinement_work(&self, v: &Word, _: usize) -> usize {
        // Scalar import may clone the input, its detached wrapper and the
        // constrained value, then reduce, project and copy out the result.
        // Numeric intersection uses a subset of that same six-copy envelope.
        v.clone_work()
            .saturating_mul(6)
            .saturating_add(v.singleton_work())
            .saturating_add(1024)
    }
}

#[test]
fn quote_and_denied_execution_never_clone_numeric_operands() {
    let domain = ValueAlgebra::new(Policy::new());
    let word = Word {
        payload: vec![7; 4096],
        odd: true,
        ..Word::top()
    };
    assert!(word.clone_work() >= 4096);
    assert!(word.singleton_work() >= 4096);
    let copies = Rc::clone(&word.copies);
    let values = [EvmValue::from_numeric(word), EvmValue::constant(U256::ONE)];
    let quote = domain.operation_work(opcode::ADD, &values);
    assert!(quote >= 2 * 64);
    assert_eq!(copies.get(), 0, "quotation must only borrow the carrier");
    let mut denied = crate::resource::WorkBudget::new(quote - 1);
    assert_eq!(
        domain
            .apply_budgeted(opcode::ADD, &values, &mut denied)
            .unwrap_err(),
        WorkExhausted
    );
    assert_eq!(copies.get(), 0, "refusal must precede any carrier work");
    let mut paid = crate::resource::WorkBudget::new(quote);
    domain
        .apply_budgeted(opcode::ADD, &values, &mut paid)
        .unwrap();
    assert_eq!(
        copies.get(),
        0,
        "the native policy only borrows its operands"
    );
    assert_eq!(paid.used(), quote);
    assert_eq!(domain.numeric_algebra().calls.get(), 1);
}

#[test]
fn borrowed_recursive_overlay_keeps_the_original_noncopy_policy() {
    fn recurse<D: EvmNumericAlgebra>(domain: AlgebraRef<'_, D>, n: usize) -> DomainSpec {
        if n == 0 {
            domain.spec()
        } else {
            recurse(domain.without_relations(), n - 1)
        }
    }
    let policy = Policy {
        spec: DomainSpec::default(),
        calls: Cell::new(0),
        byte_calls: Cell::new(0),
    };
    let domain = ValueAlgebra::new(policy);
    let view = domain.borrowed();
    let disabled = recurse(view, 8);
    assert!(!disabled.relations().enabled);
    assert!(view.spec().relations().enabled);
    assert!(std::ptr::eq(
        *view.numeric_algebra(),
        domain.numeric_algebra()
    ));
    let values = [
        EvmValue::constant(U256::from(7)),
        EvmValue::constant(U256::ONE),
    ];
    assert_eq!(
        view.without_relations()
            .apply(opcode::ADD, &values)
            .singleton(),
        Some(U256::from(8))
    );
    assert_eq!(domain.numeric_algebra().calls.get(), 1);
}

#[test]
fn unsupported_observation_refinement_preserves_extra_constraints() {
    let domain = ValueAlgebra::new(Policy::new());
    let word = Word {
        odd: true,
        ..Word::top()
    };
    assert!(word.constants().is_none());
    assert!(
        word.observation().is_top(),
        "the explicit export intentionally forgets custom oddness"
    );
    assert!(Word::from_observation(&word.observation()).is_top());
    let refined = domain
        .constrain_numeric(
            &word,
            &WordObservation::unsigned_range(U256::ONE, U256::from(9)).unwrap(),
        )
        .unwrap();
    assert!(refined.value.odd);
    assert!(!refined.value.contains(U256::from(2)));
    assert!(refined.value.contains(U256::from(3)));
    let exact = Word::constant(U256::from(3));
    assert!(matches!(
        exact.constrain(&WordObservation::constant(U256::from(4))),
        Err(FactError::Contradiction { .. })
    ));
}

#[test]
fn shared_numeric_intersection_keeps_hidden_constraints_and_prepaid_custom_work() {
    let domain = ValueAlgebra::new(Policy::new());
    let left = Word {
        odd: true,
        payload: vec![7; 4096],
        ..Word::top()
    };
    let right = Word::top();
    let quote = domain.numeric_algebra().refinement_work(&left, 1) + 17;
    let denied = domain
        .borrowed()
        .intersect_numeric_with_payment(&left, &right, 17, |work| {
            assert_eq!(work, quote);
            Err(WorkExhausted)
        });
    assert_eq!(denied.unwrap_err(), WorkExhausted);
    assert_eq!(left.copies.get(), 0);
    let mut payments = Vec::new();
    let paid = domain
        .borrowed()
        .intersect_numeric_with_payment(&left, &right, 17, |work| {
            payments.push(work);
            Ok::<_, WorkExhausted>(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(payments, [quote]);
    let pure =
        crate::domain::relational::intersect_numeric(&left, &right, domain.borrowed()).unwrap();
    assert_eq!(paid, pure);
    assert!(paid.odd);
    assert!(paid.contains(U256::from(3)));
    assert!(!paid.contains(U256::from(2)));
}

#[test]
fn relational_queries_dispatch_to_injected_policy_and_keep_singletons_without_finite_views() {
    let domain = ValueAlgebra::new(Policy::new());
    let limits = RelationLimits::default();
    let x = symbolic::ExprId::fresh().unwrap();
    let mut state = RelationState::default();
    let numeric = Word::constant(U256::from(7));
    assert!(numeric.constants().is_none());
    state.observe(&x, &numeric, &limits).unwrap();
    assert!(
        matches!(state.local_value(&x,&limits,domain.borrowed()),EvmValueQuery::Exact(v) if v==U256::from(7))
    );
    let sum = symbolic::ExprId::operation(
        opcode::ADD,
        &[x, symbolic::ExprId::constant(U256::ONE)],
        limits.expressions(),
    )
    .unwrap();
    assert!(
        matches!(state.local_value(&sum,&limits,domain.borrowed()),EvmValueQuery::Exact(v) if v==U256::from(8))
    );
    assert!(domain.numeric_algebra().calls.get() > 0);
}

#[test]
fn procedure_acceleration_keeps_default_metadata_policy_when_numeric_is_already_top() {
    let domain = ValueAlgebra::new(ProductAlgebra::default());
    let old = crate::domain::EvmValue::<NumericValue>::unsigned_range(U256::ONE, U256::from(5))
        .unwrap()
        .with_expression(symbolic::ExprId::fresh().unwrap())
        .with_origin(crate::domain::provenance::Origin::Calldata);
    let next = crate::domain::EvmValue::<NumericValue>::top()
        .with_expression(symbolic::ExprId::fresh().unwrap())
        .with_origin(crate::domain::provenance::Origin::Storage);
    let result = domain.widen_procedure(&old, &next);
    assert_eq!(result, domain.widen(&old, &next));
    assert!(result.expression().is_none());
    let finite = ValueAlgebra::new(FiniteSetAlgebra::from_spec(DomainSpec::default()));
    let result = finite.widen_procedure(
        &EvmValue::<FiniteConstantSet>::constant(U256::ONE),
        &EvmValue::<FiniteConstantSet>::top(),
    );
    assert!(result.numeric().is_top());
    assert!(result.expression().is_none());
}

#[test]
fn procedure_widening_joins_metadata_when_hidden_constraints_share_the_same_top_views() {
    use crate::domain::{
        identity::Symbol,
        provenance::{Origin, OriginSet, Provenance},
    };

    let domain = ValueAlgebra::new(Policy::new());
    let mut old = EvmValue::from_numeric(Word {
        odd: true,
        ..Word::top()
    })
    .with_symbol(Symbol::CallValue, Some(3))
    .with_symbolic_limit();
    old.provenance = Provenance::source(Origin::Storage);
    let mut next = EvmValue::<Word>::top().with_symbol(Symbol::CallValue, Some(3));
    next.provenance = Provenance::source(Origin::Calldata);
    assert_eq!(old.interval(), next.interval());
    assert!(!old.numeric().is_top());
    let widened = domain.widen_procedure(&old, &next);
    assert_eq!(widened.numeric(), next.numeric());
    assert_eq!(
        widened.provenance().origins(),
        &OriginSet::from_sources([Origin::Storage, Origin::Calldata]).unwrap()
    );
    assert_eq!(widened.identity(), old.identity());
    assert_eq!(widened.expression(), old.expression());
    assert!(widened.symbolic_limit_reached());
}

#[test]
fn byte_observation_rejects_invalid_arity_before_the_custom_hook() {
    let domain = ValueAlgebra::new(Policy::new());
    let operand = EvmValue::<Word>::constant(U256::from(42));
    for (op, args) in [
        (
            opcode::BYTE,
            vec![operand.clone(), operand.clone(), operand.clone()],
        ),
        (opcode::BYTE, vec![]),
        (opcode::AND, vec![operand.clone()]),
        (opcode::ADD, vec![operand.clone(), operand.clone()]),
        (0xfe, vec![operand.clone(), operand.clone()]),
    ] {
        let expected = domain.apply(op, &args);
        let calls = domain.numeric_algebra().calls.get();
        let quote = domain.operation_work(op, &args);
        let observed = domain.observe_byte(op, &args);
        assert_eq!(observed, expected);
        assert_eq!(domain.numeric_algebra().byte_calls.get(), 0);
        assert_eq!(
            domain.numeric_algebra().calls.get(),
            calls + 1,
            "ordinary fallback must keep the actual custom D"
        );
        assert_eq!(domain.operation_work(op, &args), quote);
    }
    let args = [EvmValue::<Word>::constant(U256::from(31)), operand];
    let observed = domain.observe_byte(opcode::BYTE, &args);
    assert_eq!(observed.singleton(), Some(U256::from(42)));
    assert_eq!(domain.numeric_algebra().byte_calls.get(), 1);
}

#[test]
fn invalid_byte_arity_preserves_ordinary_default_provenance_and_limits() {
    use crate::domain::provenance::Provenance;
    fn check<D: EvmNumericAlgebra>(domain: ValueAlgebra<D>) {
        let view = domain.borrowed().without_relations();
        let args = [
            EvmValue::<D::Element>::constant(U256::ZERO),
            EvmValue::<D::Element>::unknown_address().with_symbolic_limit(),
            EvmValue::<D::Element>::constant(U256::ZERO),
        ];
        let value = view.observe_byte(opcode::BYTE, &args);
        assert_eq!(value, view.apply(opcode::BYTE, &args));
        assert_eq!(value.provenance(), &Provenance::top());
        assert!(value.symbolic_limit_reached());
    }
    check(ValueAlgebra::new(ProductAlgebra::default()));
    check(ValueAlgebra::new(FiniteSetAlgebra::from_spec(
        DomainSpec::default(),
    )));
}
