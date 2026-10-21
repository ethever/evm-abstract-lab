use super::*;
use crate::{
    Fork,
    analysis::{
        engine::policy::PolicyContext,
        machine::{EvmChildFrame, EvmContinuation, EvmMachinePayload, fixtures},
        state::{EvmPayloadSummary, payload},
        transfer,
    },
    domain::{ValueAlgebra, symbolic::ExprId},
    world::{EvmAccount, EvmEntry, EvmWorld},
};
use analysis_control::{Cancellation, with_cancellation};
use std::rc::Rc;

pub(in crate::analysis) mod carrier;
use carrier::{Algebra, Probe, Word};

fn watched(probe: &Rc<Probe>) -> EvmValue<Word> {
    EvmValue::from_numeric(Word::constant(U256::from(7)).watched(probe))
}

#[test]
fn required_query_charges_once_and_refuses_before_unfunded_hook() {
    for (fee, available) in [(61, 60), (usize::MAX, usize::MAX)] {
        let probe = Rc::new(Probe::new(fee));
        let value = watched(&probe);
        assert_eq!(value.numeric().work_size(), 1);
        let mut budget = WorkBudget::new(available);
        assert_eq!(singleton(&value, &mut budget), Err(MachineError::Work));
        assert_eq!(probe.counts(), (1, 0, 0));
        assert_eq!(budget.used(), 0);
    }
    let probe = Rc::new(Probe::new(61));
    let value = watched(&probe);
    let mut budget = WorkBudget::new(61);
    assert_eq!(singleton(&value, &mut budget), Ok(Some(U256::from(7))));
    assert_eq!(probe.counts(), (1, 1, 0));
    assert_eq!(budget.used(), 61);

    let mut exhausted = WorkBudget::new(61);
    assert!(!exhausted.charge(62));
    assert_eq!(singleton(&value, &mut exhausted), Err(MachineError::Work));
    assert_eq!(probe.counts(), (1, 1, 0));
}

#[test]
fn either_cancellation_owner_stops_before_quotation() {
    for cancel_captured in [false, true] {
        let owner = Cancellation::new();
        let current = Cancellation::new();
        let mut budget = with_cancellation(&owner, || WorkBudget::new(61));
        let probe = Rc::new(Probe::new(61));
        let value = watched(&probe);
        if cancel_captured {
            owner.cancel();
        } else {
            current.cancel();
        }
        let result = with_cancellation(&current, || singleton(&value, &mut budget));
        assert_eq!(result, Err(MachineError::Relations(QueryReason::Cancelled)));
        assert_eq!(probe.counts(), (0, 0, 0));
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn quotation_cancellation_never_pays_or_calls_the_hook() {
    for cancel_captured in [false, true] {
        let owner = Cancellation::new();
        let current = Cancellation::new();
        let mut budget = with_cancellation(&owner, || WorkBudget::new(61));
        let probe = Rc::new(Probe {
            cancel_quote: Some(if cancel_captured {
                owner
            } else {
                current.clone()
            }),
            ..Probe::new(61)
        });
        let value = watched(&probe);
        let result = with_cancellation(&current, || singleton(&value, &mut budget));
        assert_eq!(result, Err(MachineError::Relations(QueryReason::Cancelled)));
        assert_eq!(probe.counts(), (1, 0, 1));
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn hook_cancellation_keeps_payment_but_does_not_publish_a_proof() {
    for cancel_captured in [false, true] {
        let owner = Cancellation::new();
        let current = Cancellation::new();
        let mut budget = with_cancellation(&owner, || WorkBudget::new(61));
        let probe = Rc::new(Probe {
            cancel_query: Some(if cancel_captured {
                owner
            } else {
                current.clone()
            }),
            ..Probe::new(61)
        });
        let value = watched(&probe);
        let result = with_cancellation(&current, || singleton(&value, &mut budget));
        assert_eq!(result, Err(MachineError::Relations(QueryReason::Cancelled)));
        assert_eq!(probe.counts(), (1, 1, 1));
        assert_eq!(budget.used(), 61);
    }
}

fn input(algebra: &ValueAlgebra<Algebra>) -> (EvmEntry<Word>, EvmMachinePayload<Word>) {
    let owner = fixtures::address(0x101);
    let mut world = EvmWorld::new(Fork::Osaka, "optional state metadata query regression");
    world
        .insert(owner, EvmAccount::from_hex("00", world.fork()).unwrap())
        .unwrap();
    let entry = EvmEntry::new(owner);
    let input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    (entry, input)
}

#[test]
fn complete_projection_replay_and_content_comparison_do_not_discover_singletons() {
    for fee in [61, usize::MAX] {
        let algebra = ValueAlgebra::new(Algebra);
        let (entry, mut input) = input(&algebra);
        let probe = Rc::new(Probe::new(fee));
        let shared = watched(&probe).with_expression(ExprId::fresh().unwrap());
        let independent = watched(&probe);
        let mut hidden = Word::from_observation(
            &crate::domain::WordObservation::unsigned_range(U256::ZERO, U256::from(255)).unwrap(),
        );
        hidden.excluded = Some(U256::from(13));
        let hidden = EvmValue::from_numeric(hidden.watched(&probe));
        input.active_mut().stack = vec![shared.clone(), shared, independent];
        input.active_mut().call_value = hidden.clone();
        input.active_mut().address_operand = Some(watched(&probe));
        input.active_mut().caller_operand = Some(watched(&probe));
        let child = input.active().clone();
        input.call_stack.push_child(EvmChildFrame {
            state: child,
            continuation: EvmContinuation {
                return_block: None,
                output_offset: watched(&probe),
                output_size: watched(&probe),
                creation: None,
            },
        });
        let before = input.clone();
        let mut budget = WorkBudget::new(10_000_000);
        let mut cx = PolicyContext::new(algebra.borrowed(), &entry, &mut budget);
        let summary = EvmPayloadSummary::project(&input, &mut cx).unwrap();
        let first = summary.instantiate(&mut cx).unwrap();
        let second = summary.instantiate(&mut cx).unwrap();
        let stack = &first.active().stack;
        assert!(
            stack
                .iter()
                .all(|value| value.numeric() == before.active().stack[0].numeric())
        );
        assert_eq!(stack[0].expression(), stack[1].expression());
        assert_ne!(stack[0].expression(), stack[2].expression());
        assert_ne!(stack[0].expression(), second.active().stack[0].expression());
        assert_eq!(first.active().call_value.numeric(), hidden.numeric());
        assert!(!first.active().call_value.contains(U256::from(13)));
        assert_eq!(
            summary.continuations[0].output_size.numeric(),
            watched(&probe).numeric()
        );
        assert_eq!(
            first.call_stack.children()[0]
                .continuation
                .output_offset
                .numeric(),
            watched(&probe).numeric()
        );
        let again = EvmPayloadSummary::project(&first, &mut cx).unwrap();
        assert!(summary.semantic_eq(&again, &mut cx).unwrap());
        assert_eq!(
            input.content_form().unwrap(),
            before.content_form().unwrap()
        );
        assert!(payload::semantic_eq(&summary, &again));
        assert_eq!(probe.counts(), (0, 0, 0));
        assert_eq!(input, before);
    }
}

#[test]
fn content_comparison_keeps_literal_expressions_around_numeric_top() {
    let algebra = ValueAlgebra::new(Algebra);
    let (_, mut left) = input(&algebra);
    let probe = Rc::new(Probe::new(usize::MAX));
    left.active_mut().stack = vec![
        EvmValue::from_numeric(Word::top().watched(&probe))
            .with_expression(ExprId::constant(U256::from(7))),
    ];
    let mut right = left.clone();
    right.active_mut().stack[0] = EvmValue::from_numeric(Word::top().watched(&probe))
        .with_expression(ExprId::constant(U256::from(9)));
    assert_ne!(left.content_form().unwrap(), right.content_form().unwrap());
    right.active_mut().stack[0].forget_expression();
    assert_ne!(left.content_form().unwrap(), right.content_form().unwrap());
    assert_eq!(probe.counts(), (0, 0, 0));
}
