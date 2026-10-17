//! Same-F hash recovery from joined immutable byte views and conservative histories.

use super::super::{Engine, QueryReason, RelationLimits, Stop, Work, keys::KeyMap};
use crate::{
    domain::{
        Domain,
        relational::{CheckResult, Constraint, RelationState, ValueQuery},
        symbolic::{ByteStateId, ByteView, ExprId, ExprKind, SourceId},
    },
    resource::WorkBudget,
};
use alloy_primitives::{U256, keccak256};
use revm_bytecode::opcode;
use std::sync::Arc;

fn limits() -> RelationLimits {
    // Existing analysis allowance from the captured failure, not a new policy.
    RelationLimits {
        max_nodes: 8192,
        ..RelationLimits::default()
    }
}

fn c(value: usize) -> ExprId {
    ExprId::constant(U256::from(value))
}

fn op(code: u8, args: &[ExprId]) -> ExprId {
    ExprId::operation(code, args, limits().expressions()).unwrap()
}

fn exact(bytes: &[u8]) -> ByteStateId {
    ByteStateId::exact(Arc::from(bytes))
}

fn payload(length: usize) -> Vec<u8> {
    let mut bytes = vec![0; length];
    bytes[..32].copy_from_slice(&U256::from(7).to_be_bytes::<32>());
    for (offset, byte) in bytes.iter_mut().enumerate().skip(32) {
        *byte = (offset % 251) as u8;
    }
    bytes
}

fn digest(bytes: &[u8]) -> U256 {
    U256::from_be_bytes(keccak256(bytes).0)
}

fn hash(view: &ByteView) -> ExprId {
    ExprId::keccak(view.clone(), limits().expressions()).unwrap()
}

struct Echo {
    selector: ExprId,
    length: ExprId,
    first: ByteView,
    returned: ByteView,
}

fn joined_echo(bytes: &[u8], returned: ByteStateId) -> Echo {
    let states = limits().expressions().into();
    let selector = ExprId::fresh().unwrap();
    let length = ExprId::ite(
        selector.clone(),
        c(0),
        c(bytes.len()),
        limits().expressions(),
    )
    .unwrap();
    let returned = exact(&[])
        .join_guarded(&returned, selector.clone(), states)
        .unwrap();
    let memory = ByteStateId::zero()
        .copy(
            c(0),
            ByteView::new(exact(bytes), c(0), c(bytes.len())),
            c(bytes.len()),
            states,
        )
        .unwrap()
        .copy(
            c(1024),
            ByteView::new(returned, c(0), length.clone()),
            length.clone(),
            states,
        )
        .unwrap();
    Echo {
        selector,
        first: ByteView::new(memory.clone(), c(0), c(bytes.len())),
        returned: ByteView::new(
            memory.slice(c(1024), length.clone(), states).unwrap(),
            c(0),
            c(bytes.len()),
        ),
        length,
    }
}

fn full_f(length: &ExprId) -> RelationState {
    let mut state = RelationState::default();
    let guard = op(opcode::ISZERO, &[length.clone()]);
    assert_eq!(
        state.assume(&guard, false, &limits()),
        CheckResult::MayBeFeasible
    );
    assert_eq!(state.len(), 1);
    assert_eq!(state.constraints.len(), 1);
    assert!(matches!(
        state.constraints.iter().next().unwrap(),
        Constraint::Truth { expression, nonzero: false } if expression == &guard
    ));
    state
}

fn retained<T: Clone>(map: &KeyMap<T>, key: &ExprId) -> Option<T> {
    let mut work = Work {
        remaining: limits().max_nodes,
        cancellation: analysis_control::Cancellation::default(),
        ledger: None,
    };
    map.get(key, &mut work).unwrap().cloned()
}

#[test]
fn residual_echo_511_hashes_and_equality_use_only_the_composite_length_guard() {
    let bytes = payload(511);
    let echo = joined_echo(&bytes, exact(&bytes));
    let state = full_f(&echo.length);
    let before = state.clone();
    let expected = U256::from_be_bytes([
        0x93, 0xc0, 0x15, 0xa1, 0xdc, 0x41, 0x87, 0x8a, 0xc2, 0x1d, 0xb5, 0xc9, 0x78, 0xd5, 0x19,
        0x88, 0x4e, 0x07, 0x98, 0xae, 0x68, 0xf9, 0xba, 0xb4, 0xe3, 0xb1, 0x1c, 0xe7, 0xe1, 0xaf,
        0xe0, 0xfc,
    ]);
    assert_eq!(digest(&bytes), expected);
    let first = hash(&echo.first);
    let returned = hash(&echo.returned);
    assert!(matches!(returned.kind(), ExprKind::Keccak { .. }));
    for (expression, expected) in [
        (first.clone(), expected),
        (returned.clone(), expected),
        (op(opcode::EQ, &[first, returned]), U256::ONE),
    ] {
        let mut budget = WorkBudget::new(100_000_000);
        assert_eq!(
            state.local_value_budgeted(&expression, &limits(), &mut budget),
            ValueQuery::Exact(expected)
        );
        assert!(budget.used() > 0);
        assert!(!budget.exhausted());
    }
    assert_eq!(state, before);
}

#[test]
fn residual_echo_without_its_own_guard_keeps_both_contents() {
    // The small case must complete the conservative fallback; 511 may refuse.
    for length in [32, 511] {
        let mut bytes = payload(length);
        bytes[0] = 0xa5;
        let echo = joined_echo(&bytes, exact(&bytes));
        let other = joined_echo(&bytes, exact(&bytes));
        for state in [RelationState::default(), full_f(&other.length)] {
            let before = state.clone();
            let answer = state.local_value(&hash(&echo.returned), &limits());
            match answer {
                ValueQuery::Bounds(bounds) => {
                    assert!(bounds.contains(digest(&bytes)));
                    assert!(bounds.contains(digest(&vec![0; length])));
                }
                ValueQuery::Undetermined(QueryReason::ExpressionLimit) if length == 511 => {}
                other => panic!("an unrelated F selected the echo: {other:?}"),
            }
            assert_eq!(state, before);
        }
    }
    // Unknown offsets still permit equality of a same-F selected immutable
    // view. A failed concrete extraction must not discard that identity.
    let selector = ExprId::fresh().unwrap();
    let offset = ExprId::fresh().unwrap();
    let literal = exact(&[1, 2]);
    let selected = exact(&[3, 4])
        .join_guarded(&literal, selector.clone(), limits().expressions().into())
        .unwrap();
    let left = hash(&ByteView::new(selected, offset.clone(), c(1)));
    let right = hash(&ByteView::new(literal, offset.clone(), c(1)));
    let equality = op(opcode::EQ, &[left, right]);
    assert!(matches!(
        RelationState::default().local_value(&equality, &limits()),
        ValueQuery::Bounds(_)
    ));
    let mut state = RelationState::default();
    assert_eq!(
        state.assume(&selector, false, &limits()),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        state.local_value(&equality, &limits()),
        ValueQuery::Exact(U256::ONE)
    );
    assert!(matches!(
        state.local_value(&offset, &limits()),
        ValueQuery::Bounds(_)
    ));
}

#[test]
fn residual_echo_changed_last_byte_and_source_do_not_reuse_the_first_hash() {
    let bytes = payload(511);
    let mut changed = bytes.clone();
    *changed.last_mut().unwrap() ^= 1;
    let echo = joined_echo(&bytes, exact(&changed));
    let state = full_f(&echo.length);
    assert_eq!(
        state.local_value(&hash(&echo.returned), &limits()),
        ValueQuery::Exact(digest(&changed))
    );
    assert_eq!(
        state.local_value(
            &op(opcode::EQ, &[hash(&echo.first), hash(&echo.returned)]),
            &limits()
        ),
        ValueQuery::Exact(U256::ZERO)
    );
    let token = ExprId::fresh().unwrap();
    for source in [
        SourceId::scoped(token.clone(), U256::ONE),
        SourceId::scoped(token, U256::from(2)),
        SourceId::fresh().unwrap(),
    ] {
        let echo = joined_echo(&bytes, ByteStateId::input(source, c(bytes.len())));
        let state = full_f(&echo.length);
        assert!(matches!(
            state.local_value(&hash(&echo.returned), &limits()),
            ValueQuery::Bounds(_)
        ));
    }
}

#[test]
fn residual_echo_partial_overlapping_and_unknown_writes_keep_the_byte_fallback() {
    let states = limits().expressions().into();
    let bytes = payload(64);
    let original = exact(&bytes);
    let partial = ByteStateId::zero()
        .copy(
            c(1024),
            ByteView::new(original.clone(), c(0), c(63)),
            c(63),
            states,
        )
        .unwrap();
    let mut partial_bytes = bytes.clone();
    partial_bytes[63] = 0;
    let overlapping = original
        .copy(
            c(16),
            ByteView::new(original.clone(), c(0), c(48)),
            c(48),
            states,
        )
        .unwrap();
    let mut overlapping_bytes = bytes.clone();
    overlapping_bytes[16..].copy_from_slice(&bytes[..48]);
    for (view, expected) in [
        (ByteView::new(partial, c(1024), c(64)), partial_bytes),
        (ByteView::new(overlapping, c(0), c(64)), overlapping_bytes),
    ] {
        let expected = digest(&expected);
        assert_ne!(expected, digest(&bytes));
        assert_eq!(
            RelationState::default().local_value(&hash(&view), &limits()),
            ValueQuery::Exact(expected)
        );
    }
    let unknown = original
        .write_byte(ExprId::fresh().unwrap(), c(0xa5), states)
        .unwrap();
    assert!(matches!(
        RelationState::default()
            .local_value(&hash(&ByteView::new(unknown, c(0), c(64))), &limits()),
        ValueQuery::Bounds(_)
    ));
}

#[test]
fn residual_echo_bulk_admission_is_paid_atomic_and_cancellable() {
    let limits = limits();
    let bytes = payload(511);
    let literal = ByteView::new(exact(&bytes), c(0), c(bytes.len()));
    let mut paid = WorkBudget::new(100_000_000);
    let mut work = Work {
        remaining: limits.max_nodes,
        cancellation: analysis_control::Cancellation::default(),
        ledger: Some(&mut paid),
    };
    assert_eq!(
        Engine::new(&mut work, &limits, Domain::default())
            .literal_hash(&literal)
            .unwrap(),
        Some(digest(&bytes))
    );
    let full_fee = paid.used();
    // Measure a successful envelope, then refuse one unit below it. No fee
    // coefficient is part of the test's expected result.
    for local in [1, limits.max_nodes] {
        let mut budget = WorkBudget::new(full_fee - 1);
        let mut work = Work {
            remaining: local,
            cancellation: analysis_control::Cancellation::default(),
            ledger: Some(&mut budget),
        };
        let mut engine = Engine::new(&mut work, &limits, Domain::default());
        assert!(matches!(
            engine.literal_hash(&literal),
            Err(Stop::Pending(QueryReason::ExpressionLimit))
        ));
        assert_eq!(engine.work.remaining, local - 1);
        assert_eq!(engine.work.ledger.as_ref().unwrap().used(), 1);
        assert!(!engine.work.ledger.as_ref().unwrap().exhausted());
    }
    let echo = joined_echo(&bytes, exact(&bytes));
    let state = full_f(&echo.length);
    let expression = hash(&echo.returned);
    for via_term in [false, true] {
        let mut work = Work {
            remaining: limits.max_nodes,
            cancellation: analysis_control::Cancellation::default(),
            ledger: None,
        };
        let mut engine = Engine::new(&mut work, &limits, Domain::default());
        for atom in &state.constraints {
            engine.atom(atom).unwrap();
            engine.validate(atom).unwrap();
        }
        engine.hash_view(&echo.returned).unwrap();
        let mut budget = WorkBudget::new(full_fee - 1);
        engine.work.ledger = Some(&mut budget);
        let answer = if via_term {
            engine.term(&expression).map(|term| term.as_constant())
        } else {
            engine.eval(&expression).map(|value| value.singleton())
        };
        assert!(matches!(
            answer,
            Err(Stop::Pending(QueryReason::ExpressionLimit))
        ));
        assert!(retained(&engine.memo, &expression).is_none());
        assert!(retained(&engine.terms, &expression).is_none());
        assert!(!engine.work.ledger.as_ref().unwrap().exhausted());
        engine.work.ledger = None;
        let answer = if via_term {
            engine.term(&expression).unwrap().as_constant()
        } else {
            engine.eval(&expression).unwrap().singleton()
        };
        assert_eq!(answer, Some(digest(&bytes)));
    }

    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    let mut budget = WorkBudget::new(100_000_000);
    let mut work = Work {
        remaining: limits.max_nodes,
        cancellation,
        ledger: Some(&mut budget),
    };
    assert!(matches!(
        Engine::new(&mut work, &limits, Domain::default()).literal_hash(&literal),
        Err(Stop::Pending(QueryReason::Cancelled))
    ));
    assert_eq!(work.remaining, limits.max_nodes);
    assert_eq!(budget.used(), 0);
}

#[test]
fn residual_echo_hash_alias_does_not_turn_a_union_find_cycle_into_evaluation() {
    let mut bytes = payload(64);
    bytes[0] = 0xa5;
    let echo = joined_echo(&bytes, exact(&bytes));
    let expression = hash(&echo.returned);
    let mut state = RelationState::default();
    assert_eq!(
        state.add_equal(&echo.selector, &expression, &limits()),
        CheckResult::MayBeFeasible
    );
    let before = state.clone();
    // The immutable definition refers to q, while F aliases q to that result.
    // Normalizing view operands must not recursively evaluate the UF parent.
    assert!(matches!(
        state.local_value(&expression, &limits()),
        ValueQuery::Bounds(_)
    ));
    assert_eq!(state, before);
}
