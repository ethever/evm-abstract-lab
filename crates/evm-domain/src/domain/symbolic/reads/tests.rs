use super::super::{ExprError, ExprId, ExprKind, ExprLimits, SlotStateId, SlotStateKind, SourceId};
use crate::domain::identity::Symbol;
use alloy_primitives::U256;

fn c(value: usize) -> ExprId {
    ExprId::constant(U256::from(value))
}

#[test]
fn slot_read_handles_4096_literal_writes_with_finite_limits() {
    const WRITES: usize = 4096;
    // Match the checked world's admitted history, later writes and join/read.
    let limits = ExprLimits {
        max_nodes: 6 * WRITES + 3 * 8 + 6,
        max_depth: WRITES + 8 + 3,
    };
    let source = SourceId::scoped(ExprId::input(73, Symbol::Caller), U256::from(0x1234));
    for initial in [SlotStateId::zero(), SlotStateId::input(source.clone())] {
        let mut state = initial.clone();
        for index in 0..WRITES {
            state = state.store(c(index), c(index + 1), limits.into()).unwrap();
        }
        assert_eq!(state.nodes(), initial.nodes() + 3 * WRITES);
        assert_eq!(state.depth(), initial.depth() + WRITES);
        for (key, expected) in [(0, 1), (WRITES - 1, WRITES)] {
            assert_eq!(
                ExprId::slot_read(state.clone(), c(key), limits).unwrap(),
                c(expected)
            );
        }
        let unlisted = ExprId::slot_read(state, c(WRITES), limits).unwrap();
        match initial.kind() {
            SlotStateKind::Zero => assert_eq!(unlisted, c(0)),
            SlotStateKind::Input { .. } => {
                let ExprKind::SlotRead { state, key } = unlisted.kind() else {
                    panic!("an unobserved source cell must retain its original snapshot");
                };
                assert_eq!(state, &initial);
                assert_eq!(key, &c(WRITES));
                assert!(
                    matches!(state.kind(), SlotStateKind::Input { source: retained, .. }
                    if retained == &source && retained.namespace() == U256::from(0x1234))
                );
                assert_eq!((unlisted.nodes(), unlisted.depth()), (4, 3));
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn slot_read_keeps_may_alias_writes_and_the_retained_cursor() {
    let limits = ExprLimits::default();
    let source = SourceId::scoped(ExprId::input(74, Symbol::Caller), U256::from(0x5678));
    let initial = SlotStateId::input(source);
    let symbolic_key = ExprId::input(74, Symbol::CallValue);
    let alias = initial
        .store(symbolic_key.clone(), c(17), limits.into())
        .unwrap();
    let state = alias.store(c(9), c(23), limits.into()).unwrap();
    let read = ExprId::slot_read(state.clone(), c(0), limits).unwrap();
    assert!(matches!(read.kind(), ExprKind::SlotRead { state, key }
        if state == &alias && key == &c(0)));
    assert_eq!(
        (read.nodes(), read.depth()),
        (alias.nodes() + 2, alias.depth() + 1)
    );
    // An identical latest symbolic key still folds. A symbolic query may
    // alias the later literal write, so that write must remain in its cursor.
    assert_eq!(
        ExprId::slot_read(alias, symbolic_key.clone(), limits).unwrap(),
        c(17)
    );
    let symbolic_read = ExprId::slot_read(state.clone(), symbolic_key.clone(), limits).unwrap();
    assert!(
        matches!(symbolic_read.kind(), ExprKind::SlotRead { state: retained, key }
        if retained == &state && key == &symbolic_key)
    );
}

#[test]
fn slot_read_evaluates_nested_ites_in_branch_order_and_keeps_symbolic_queries() {
    let limits = ExprLimits::default();
    let guard = ExprId::input(75, Symbol::Caller);
    let inner_guard = ExprId::input(75, Symbol::CallValue);
    let zero = SlotStateId::zero();
    let yes = zero.store(c(0), c(11), limits.into()).unwrap();
    let no = zero.store(c(0), c(22), limits.into()).unwrap();
    let inner = yes
        .join_guarded(&no, inner_guard.clone(), limits.into())
        .unwrap();
    let state = inner
        .join_guarded(&zero, guard.clone(), limits.into())
        .unwrap();
    let expected = ExprId::ite(
        guard,
        ExprId::ite(inner_guard, c(11), c(22), limits).unwrap(),
        c(0),
        limits,
    )
    .unwrap();
    assert_eq!(
        ExprId::slot_read(state.clone(), c(0), limits).unwrap(),
        expected
    );
    assert_eq!(
        ExprId::slot_read(state.clone(), c(7), limits).unwrap(),
        c(0)
    );
    let key = ExprId::input(76, Symbol::Caller);
    let read = ExprId::slot_read(state.clone(), key.clone(), limits).unwrap();
    assert!(
        matches!(read.kind(), ExprKind::SlotRead { state: retained, key: retained_key }
        if retained == &state && retained_key == &key)
    );
}

#[test]
fn slot_read_checks_root_and_retained_ite_limits_with_cancellation_precedence() {
    let limits = ExprLimits::default();
    let state = SlotStateId::zero()
        .store(c(0), c(7), limits.into())
        .unwrap();
    let exact = ExprLimits {
        max_nodes: 6,
        max_depth: 3,
    };
    for key in [c(0), c(1)] {
        assert!(ExprId::slot_read(state.clone(), key.clone(), exact).is_ok());
        for denied in [
            ExprLimits {
                max_nodes: 5,
                ..exact
            },
            ExprLimits {
                max_depth: 2,
                ..exact
            },
        ] {
            assert_eq!(
                ExprId::slot_read(state.clone(), key.clone(), denied),
                Err(ExprError::Limit { nodes: 6, depth: 3 })
            );
        }
    }

    let token = ExprId::input(77, Symbol::Caller);
    let yes = SlotStateId::input(SourceId::scoped(token.clone(), U256::from(1)));
    let no = SlotStateId::input(SourceId::scoped(token, U256::from(2)));
    let guard = ExprId::input(77, Symbol::CallValue);
    let selected = yes.join_guarded(&no, guard, limits.into()).unwrap();
    let exact_ite = ExprLimits {
        max_nodes: 10,
        max_depth: 4,
    };
    let read = ExprId::slot_read(selected.clone(), c(0), exact_ite).unwrap();
    assert_eq!((read.nodes(), read.depth()), (10, 4));
    assert_eq!(
        ExprId::slot_read(
            selected,
            c(0),
            ExprLimits {
                max_nodes: 9,
                ..exact_ite
            }
        ),
        Err(ExprError::Limit {
            nodes: 10,
            depth: 4
        })
    );

    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            ExprId::slot_read(
                state,
                c(0),
                ExprLimits {
                    max_nodes: 0,
                    max_depth: 0
                }
            ),
            Err(ExprError::Cancelled)
        );
    });
}
