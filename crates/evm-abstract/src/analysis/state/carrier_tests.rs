//! A numeric predicate outside WordObservation must survive executable state,
//! canonical templates, continuations and independently bound checkpoints.

use super::*;
use crate::{
    Fork,
    analysis::{
        machine::{EvmExecutionEvidence, EvmInstructionValue, InstructionProgress, fixtures},
        transfer,
    },
    domain::{
        DomainSpec, EvmNumericAlgebra, NumericReduction, Profile, ValueAlgebra, WordObservation,
        facts::FactError,
    },
    world::{EvmAccount, EvmWorld},
};
use alloy_primitives::U256;
use std::{cell::Cell, fmt, num::NonZeroUsize};

mod joins;
mod numeric;
mod projection;
use numeric::ExcludingWord;

// Intentionally neither Copy nor Clone. Every state operation must borrow the
// one immutable run policy and use its actual scalar semantics.
#[derive(Debug)]
struct Algebra {
    spec: DomainSpec,
    joins: Cell<usize>,
    reductions: Cell<usize>,
}

impl Algebra {
    fn new() -> Self {
        Self {
            spec: DomainSpec::new(
                Profile::Product,
                NonZeroUsize::new(8).unwrap(),
                NonZeroUsize::new(4).unwrap(),
                NonZeroUsize::new(256).unwrap(),
            ),
            joins: Cell::new(0),
            reductions: Cell::new(0),
        }
    }
}

impl EvmNumericAlgebra for Algebra {
    type Element = ExcludingWord;

    fn spec(&self) -> DomainSpec {
        self.spec
    }
    fn project_numeric(&self, value: &ExcludingWord) -> ExcludingWord {
        value.clone()
    }
    fn join_numeric(&self, left: &ExcludingWord, right: &ExcludingWord) -> ExcludingWord {
        self.joins.set(self.joins.get() + 1);
        left.join(right)
    }
    fn widen_numeric(&self, old: &ExcludingWord, next: &ExcludingWord) -> ExcludingWord {
        old.widen(next)
    }
    fn widen_procedure_numeric(&self, old: &ExcludingWord, next: &ExcludingWord) -> ExcludingWord {
        old.widen(next)
    }
    fn reduce_numeric(&self, value: &ExcludingWord) -> NumericReduction<ExcludingWord> {
        self.reductions.set(self.reductions.get() + 1);
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric(&self, _op: u8, _args: &[ExcludingWord]) -> NumericReduction<ExcludingWord> {
        NumericReduction::stable(ExcludingWord::top())
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Self::Element>) -> usize
    where
        Self::Element: 'a,
    {
        args.count().saturating_add(1).saturating_mul(32)
    }
    fn projection_work(&self, value: &ExcludingWord) -> usize {
        value.work_size()
    }
    fn refinement_work(&self, value: &ExcludingWord, facts: usize) -> usize {
        facts
            .saturating_add(1)
            .saturating_mul(value.work_size())
            .saturating_mul(32)
    }
}

fn value(excluded: u64) -> EvmValue<ExcludingWord> {
    EvmValue::from_numeric(ExcludingWord::excluding(excluded))
        .with_expression(ExprId::fresh().unwrap())
}

fn fixture(
    algebra: AlgebraRef<'_, Algebra>,
) -> (EvmEntry<ExcludingWord>, EvmMachinePayload<ExcludingWord>) {
    let owner = fixtures::address(0x101);
    let mut world = EvmWorld::new(Fork::Osaka, "custom state carrier");
    let mut account = EvmAccount::from_hex("00", world.fork()).unwrap();
    account.balance = value(13);
    world.insert(owner, account).unwrap();
    let entry = EvmEntry::new(owner);
    let mut payload = transfer::initial(&world, &entry, algebra).unwrap();
    let original = value(13);
    payload.active_mut().stack = vec![original.clone(), original.clone()];
    payload.active_mut().call_value = original.clone();
    payload.active_mut().address_operand = Some(original.clone());
    payload.active_mut().caller_operand = Some(original);
    (entry, payload)
}

fn excludes(value: &EvmValue<ExcludingWord>, excluded: u64) {
    assert_eq!(value.numeric().excluded(), Some(U256::from(excluded)));
    assert!(!value.contains(U256::from(excluded)));
    assert!(value.contains(U256::from(excluded + 1)));
    // This loss is explicit at immutable source export only. It cannot be used
    // as the stored scalar representation for a frame or numerical summary.
    assert!(value.numeric().observation().contains(U256::from(excluded)));
}

#[test]
fn custom_carrier_survives_full_templates_evidence_and_independent_checkpoints() {
    let algebra = ValueAlgebra::new(Algebra::new());
    let domain = algebra.borrowed();
    let (entry, mut input) = fixture(domain);
    let owner = input.active().key.address;
    input.active_mut().saved_store = input.store.snapshot();
    input.store.write_balance(owner, value(17));
    let mut child = input.active().clone();
    child.saved_store = input.store.snapshot();
    child.key.caller = fixtures::address(0x202).into();
    input.call_stack.push_child(EvmChildFrame {
        state: child,
        continuation: EvmContinuation {
            return_block: None,
            output_offset: value(23),
            output_size: value(29),
            creation: None,
        },
    });
    input.store.write_balance(owner, value(19));

    let bound = form_normalized(&input, &entry, domain).unwrap();
    for frame in &bound.frames {
        for scalar in frame.values() {
            excludes(scalar, 13);
        }
    }
    excludes(&bound.continuations[0].output_offset, 23);
    excludes(&bound.continuations[0].output_size, 29);
    let first = instantiate(&bound, domain).unwrap();
    let second = instantiate(&bound, domain).unwrap();
    excludes(&first.active().stack[0], 13);
    excludes(&first.store.read_balance(owner), 19);
    excludes(
        &first
            .call_stack
            .root()
            .state
            .saved_store
            .state()
            .read_balance(owner),
        13,
    );
    excludes(&first.active().saved_store.state().read_balance(owner), 17);
    assert_eq!(
        first.active().stack[0].expression(),
        first.active().stack[1].expression()
    );
    assert_ne!(
        first.active().stack[0].expression(),
        second.active().stack[0].expression()
    );
    assert!(semantic_eq(
        &bound,
        &form_normalized(&first, &entry, domain).unwrap()
    ));

    let evidence = EvmExecutionEvidence::new(
        vec![InstructionProgress::Completed],
        vec![EvmInstructionValue::new(0, &first.active().stack[0])],
    );
    let retained: &ExcludingWord = evidence.instruction_values().unwrap()[0].numeric();
    assert_eq!(retained.excluded(), Some(U256::from(13)));
    assert!(semantic_eq(
        &bound,
        &form_normalized(&input, &entry, domain).unwrap()
    ));
}

#[test]
fn custom_guarded_join_uses_one_selector_and_borrowed_numeric_policy() {
    use crate::domain::symbolic::ExprKind;
    let algebra = ValueAlgebra::new(Algebra::new());
    let domain = algebra.borrowed();
    let (_, left) = fixture(domain);
    let mut right = left.clone();
    let replacement = value(13);
    right.active_mut().stack = vec![replacement.clone(), replacement.clone()];
    right.active_mut().call_value = replacement.clone();
    right.active_mut().address_operand = Some(replacement.clone());
    right.active_mut().caller_operand = Some(replacement.clone());
    let owner = right.active().key.address;
    right.store.write_balance(owner, replacement);
    let joined = left.join(&right, domain).unwrap();
    let selector = |scalar: &EvmValue<ExcludingWord>| {
        let ExprKind::Ite {
            condition: selector,
            ..
        } = scalar.expression().unwrap().kind()
        else {
            panic!("whole-state scalar has no guarded join");
        };
        selector.clone()
    };
    let guard = selector(&joined.active().stack[0]);
    assert_eq!(guard, selector(&joined.active().call_value));
    assert_eq!(
        guard,
        selector(joined.active().address_operand.as_ref().unwrap())
    );
    assert_eq!(
        guard,
        selector(joined.active().caller_operand.as_ref().unwrap())
    );
    excludes(&joined.active().stack[0], 13);
    assert!(algebra.numeric_algebra().joins.get() > 0);

    let before = left.clone();
    right.active_mut().key.is_static = !left.active().key.is_static;
    assert_eq!(left.join(&right, domain), Err(MachineError::Shape));
    assert_eq!(left, before);
}

#[test]
fn coarse_custom_bound_covers_input_and_refuses_malformed_child_shape() {
    let algebra = ValueAlgebra::new(Algebra::new());
    let domain = algebra.borrowed();
    let (entry, input) = fixture(domain);
    let bound = form_normalized(&input, &entry, domain).unwrap();
    let policy = bounded::EvmBoundedState::freeze(&bound, true, domain).unwrap();
    let mut budget = WorkBudget::new(usize::MAX);
    let (policy, coarse, live) = policy
        .materialize(bound.clone(), domain, &mut budget)
        .unwrap();
    for word in 0u64..=255 {
        if input.active().stack[0].contains(U256::from(word)) {
            assert!(coarse.active().stack[0].contains(U256::from(word)));
            assert!(live.active().stack[0].contains(U256::from(word)));
        }
    }
    let mut next = input.clone();
    next.active_mut().stack[0] = value(17);
    let next = form_normalized(&next, &entry, domain).unwrap();
    let again = policy
        .join(&coarse, &next, domain, &mut budget, false)
        .unwrap();
    assert!(semantic_eq(&coarse, &again));
    assert_eq!(again.active().stack[0].numeric().excluded(), None);
    assert!(again.active().stack[0].contains(U256::from(13)));
    assert!(again.active().stack[0].contains(U256::from(17)));
    let mut malformed = bound.clone();
    malformed.frames.push(bound.frames[0].clone());
    assert!(matches!(
        instantiate(&malformed, domain),
        Err(MachineError::Shape)
    ));
    assert!(semantic_eq(
        &bound,
        &form_normalized(&input, &entry, domain).unwrap()
    ));
}
