//! Captured call-word history, read boundaries and conservative alias regressions.

fn product_algebra() -> crate::domain::ValueAlgebra<crate::domain::ProductAlgebra> {
    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
}
use super::super::{CheckResult, EvmValueQuery, QueryReason, RelationLimits, RelationState};
use crate::domain::symbolic::{ByteStateId, ByteView, ExprId};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}
fn policy() -> RelationLimits {
    RelationLimits {
        max_nodes: 8192,
        max_depth: 128,
        max_constraints: 256,
        ..RelationLimits::default()
    }
}
fn read(state: ByteStateId, at: ExprId, limits: RelationLimits) -> ExprId {
    ExprId::word_read(ByteView::new(state, at, c(32)), c(0), limits.expressions()).unwrap()
}
fn equality(left: ExprId, right: ExprId, limits: RelationLimits) -> ExprId {
    ExprId::operation(opcode::EQ, &[left, right], limits.expressions()).unwrap()
}
fn copy_word(memory: &ByteStateId, at: u64, value: ExprId, limits: RelationLimits) -> ByteStateId {
    let captured = ByteStateId::zero()
        .write_word(c(0), value, limits.expressions().into())
        .unwrap();
    memory
        .copy(
            c(at),
            ByteView::new(captured, c(0), c(32)),
            c(32),
            limits.expressions().into(),
        )
        .unwrap()
}

#[test]
fn captured_copy_history_survives_disjoint_status_words_and_freshening() {
    {
        let limits = policy();
        let x = ExprId::fresh().unwrap();
        let y = ExprId::fresh().unwrap();
        let first_guard = ExprId::fresh().unwrap();
        let second_guard = ExprId::fresh().unwrap();
        let zero = ByteStateId::zero();
        let first = zero
            .join_guarded(
                &copy_word(&zero, 0x80, x, limits),
                first_guard.clone(),
                limits.expressions().into(),
            )
            .unwrap()
            .write_word(
                c(0x100),
                ExprId::ite(first_guard.clone(), c(0), c(1), limits.expressions()).unwrap(),
                limits.expressions().into(),
            )
            .unwrap();
        let snapshot = first.clone();
        let historical = read(snapshot.clone(), c(0x80), limits);
        let latest = first
            .join_guarded(
                &copy_word(&first, 0xc0, y, limits),
                second_guard.clone(),
                limits.expressions().into(),
            )
            .unwrap()
            .write_word(
                c(0x120),
                ExprId::ite(second_guard.clone(), c(0), c(1), limits.expressions()).unwrap(),
                limits.expressions().into(),
            )
            .unwrap();
        let current = read(latest.clone(), c(0x80), limits);
        let second = read(latest.clone(), c(0xc0), limits);
        assert_eq!(first, snapshot, "captured state identity survives writes");
        assert_eq!(
            RelationState::default().local_value(
                &equality(current, historical.clone(), limits),
                &limits,
                product_algebra().borrowed()
            ),
            EvmValueQuery::Exact(U256::from(1)),
            "local history across both call alternatives"
        );
        let mut successful = RelationState::default();
        assert_eq!(
            successful.assume(&first_guard, false, &limits, product_algebra().borrowed()),
            CheckResult::MayBeFeasible
        );
        assert_eq!(
            successful.assume(&second_guard, false, &limits, product_algebra().borrowed()),
            CheckResult::MayBeFeasible
        );
        let equal_observations = equality(historical.clone(), second, limits);
        for value in [U256::ZERO, U256::from(1)] {
            assert_eq!(
                successful.can_equal(
                    &equal_observations,
                    value,
                    &limits,
                    product_algebra().borrowed()
                ),
                CheckResult::MayBeFeasible,
                "local: independent GAS observations may agree or differ"
            );
        }
        let map = latest
            .fresh_leaves()
            .into_iter()
            .map(|leaf| (leaf, ExprId::fresh().unwrap()))
            .collect();
        let renamed = latest.rename_fresh(&map, limits.expressions()).unwrap();
        assert_ne!(latest, renamed);
        let renamed_history = historical.rename_fresh(&map, limits.expressions()).unwrap();
        assert_eq!(
            RelationState::default().local_value(
                &equality(read(renamed, c(0x80), limits), renamed_history, limits),
                &limits,
                product_algebra().borrowed()
            ),
            EvmValueQuery::Exact(U256::from(1)),
            "local: replay keeps shared historical dependencies"
        );
    }
}

#[test]
fn later_word_aliases_and_overflow_do_not_inherit_disjoint_history() {
    {
        let limits = policy();
        for (at, written, symbolic) in [
            (U256::from(128), U256::from(159), false),
            (U256::from(128), U256::from(160), false),
            (U256::from(128), U256::from(128), true),
            (U256::from(128), U256::from(159), true),
            (U256::from(128), U256::from(256), true),
            (U256::from(128), U256::MAX, false),
            (U256::MAX, U256::ZERO, false),
            (U256::MAX - U256::from(31), U256::ZERO, false),
        ] {
            let x = ExprId::fresh().unwrap();
            let y = ExprId::fresh().unwrap();
            let key = if symbolic {
                ExprId::fresh().unwrap()
            } else {
                ExprId::constant(written)
            };
            let before = ByteStateId::zero()
                .write_word(ExprId::constant(at), x.clone(), limits.expressions().into())
                .unwrap();
            let after = before
                .write_word(key.clone(), y.clone(), limits.expressions().into())
                .unwrap();
            let mut path = RelationState::default();
            for (leaf, value) in [
                (x, U256::from_be_bytes([0x11; 32])),
                (y, U256::from_be_bytes([0xaa; 32])),
            ] {
                assert_eq!(
                    path.add_equal(
                        &leaf,
                        &ExprId::constant(value),
                        &limits,
                        product_algebra().borrowed()
                    ),
                    CheckResult::MayBeFeasible
                );
            }
            if symbolic {
                assert_eq!(
                    path.add_equal(
                        &key,
                        &ExprId::constant(written),
                        &limits,
                        product_algebra().borrowed()
                    ),
                    CheckResult::MayBeFeasible
                );
            }
            let expected: [u8; 32] = std::array::from_fn(|j| {
                let Some(index) = at.checked_add(U256::from(j)) else {
                    return 0;
                };
                if index >= written && index - written < U256::from(32) {
                    0xaa
                } else {
                    0x11
                }
            });
            assert_eq!(
                path.local_value(
                    &read(after, ExprId::constant(at), limits),
                    &limits,
                    product_algebra().borrowed()
                ),
                EvmValueQuery::Exact(U256::from_be_bytes(expected)),
                "local: read={at}, written={written}, symbolic={symbolic}"
            );
        }
    }
}

#[test]
fn copied_word_history_respects_short_views_overlap_and_mathematical_padding() {
    {
        let limits = policy();
        for (at, destination, copied, symbolic) in [
            (U256::ZERO, U256::ZERO, U256::from(1), false),
            (U256::ZERO, U256::ZERO, U256::from(31), false),
            (U256::ZERO, U256::ZERO, U256::from(32), false),
            (U256::ZERO, U256::ZERO, U256::MAX, false),
            (U256::ZERO, U256::from(31), U256::from(32), false),
            (U256::ZERO, U256::from(32), U256::from(32), false),
            (U256::ZERO, U256::ZERO, U256::from(32), true),
            (U256::ZERO, U256::MAX, U256::from(32), false),
            (U256::MAX, U256::MAX, U256::from(32), false),
        ] {
            let old = ExprId::fresh().unwrap();
            let data = ExprId::fresh().unwrap();
            let address = if symbolic {
                ExprId::fresh().unwrap()
            } else {
                ExprId::constant(destination)
            };
            let source = ByteStateId::zero()
                .write_word(c(0), data.clone(), limits.expressions().into())
                .unwrap();
            let memory = ByteStateId::zero()
                .write_word(
                    ExprId::constant(at),
                    old.clone(),
                    limits.expressions().into(),
                )
                .unwrap()
                .copy(
                    address.clone(),
                    ByteView::new(source, c(3), c(7)),
                    ExprId::constant(copied),
                    limits.expressions().into(),
                )
                .unwrap();
            let mut path = RelationState::default();
            assert_eq!(
                path.add_equal(
                    &old,
                    &ExprId::constant(U256::from_be_bytes([0x11; 32])),
                    &limits,
                    product_algebra().borrowed()
                ),
                CheckResult::MayBeFeasible
            );
            let source_bytes: [u8; 32] = std::array::from_fn(|j| j as u8);
            assert_eq!(
                path.add_equal(
                    &data,
                    &ExprId::constant(U256::from_be_bytes(source_bytes)),
                    &limits,
                    product_algebra().borrowed()
                ),
                CheckResult::MayBeFeasible
            );
            if symbolic {
                assert_eq!(
                    path.add_equal(
                        &address,
                        &ExprId::constant(destination),
                        &limits,
                        product_algebra().borrowed()
                    ),
                    CheckResult::MayBeFeasible
                );
            }
            let expected: [u8; 32] = std::array::from_fn(|j| {
                let Some(index) = at.checked_add(U256::from(j)) else {
                    return 0;
                };
                if index >= destination && index - destination < copied {
                    let relative = index - destination;
                    if relative < U256::from(7) {
                        source_bytes[3 + usize::try_from(relative).unwrap()]
                    } else {
                        0
                    }
                } else {
                    0x11
                }
            });
            assert_eq!(
                path.local_value(
                    &read(memory, ExprId::constant(at), limits),
                    &limits,
                    product_algebra().borrowed()
                ),
                EvmValueQuery::Exact(U256::from_be_bytes(expected)),
                "local: read={at}, copy={destination}, length={copied}, symbolic={symbolic}"
            );
        }
    }
}

#[test]
fn guarded_word_views_keep_boundaries_and_typed_query_denials() {
    {
        let limits = policy();
        let guard = ExprId::fresh().unwrap();
        let yes = ByteStateId::zero()
            .write_word(
                c(128),
                ExprId::constant(U256::from_be_bytes([0xaa; 32])),
                limits.expressions().into(),
            )
            .unwrap();
        let no = ByteStateId::zero()
            .write_word(
                c(192),
                ExprId::constant(U256::from_be_bytes([0xbb; 32])),
                limits.expressions().into(),
            )
            .unwrap();
        let state = yes
            .join_guarded(&no, guard.clone(), limits.expressions().into())
            .unwrap();
        let view = ByteView::new(
            state,
            ExprId::ite(guard.clone(), c(128), c(192), limits.expressions()).unwrap(),
            ExprId::ite(guard.clone(), c(7), c(32), limits.expressions()).unwrap(),
        );
        let value = ExprId::word_read(view, c(0), limits.expressions()).unwrap();
        for (nonzero, expected) in [
            (
                true,
                U256::from_be_bytes(std::array::from_fn::<_, 32, _>(|j| {
                    if j < 7 { 0xaa } else { 0 }
                })),
            ),
            (false, U256::from_be_bytes([0xbb; 32])),
        ] {
            let mut path = RelationState::default();
            assert_eq!(
                path.assume(&guard, nonzero, &limits, product_algebra().borrowed()),
                CheckResult::MayBeFeasible
            );
            assert_eq!(
                path.local_value(&value, &limits, product_algebra().borrowed()),
                EvmValueQuery::Exact(expected),
                "local, guard={nonzero}"
            );
        }
        let denied = RelationLimits {
            max_nodes: value.nodes() - 1,
            ..limits
        };
        assert_eq!(
            RelationState::default().local_value(&value, &denied, product_algebra().borrowed()),
            EvmValueQuery::Undetermined(QueryReason::ExpressionLimit)
        );
        let denied = RelationLimits {
            max_depth: value.depth() - 1,
            ..limits
        };
        assert_eq!(
            RelationState::default().local_value(&value, &denied, product_algebra().borrowed()),
            EvmValueQuery::Undetermined(QueryReason::ExpressionLimit)
        );
        let token = analysis_control::Cancellation::new();
        token.cancel();
        analysis_control::with_cancellation(&token, || {
            assert_eq!(
                RelationState::default().local_value(&value, &limits, product_algebra().borrowed()),
                EvmValueQuery::Undetermined(QueryReason::Cancelled)
            );
        });
    }
}
