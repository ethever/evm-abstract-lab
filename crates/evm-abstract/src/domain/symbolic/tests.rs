use super::{ExprId, ExprLimits};
use crate::domain::identity::Symbol;
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeMap;

#[test]
fn scope_and_runtime_freshness_do_not_confuse_input_identity() {
    assert_eq!(
        ExprId::input(1, Symbol::Caller),
        ExprId::input(1, Symbol::Caller)
    );
    assert_ne!(
        ExprId::input(1, Symbol::Caller),
        ExprId::input(2, Symbol::Caller)
    );
    assert_ne!(ExprId::fresh().unwrap(), ExprId::fresh().unwrap());
}
#[test]
fn limits_prevent_unbounded_expression_growth_before_allocation() {
    let x = ExprId::input(1, Symbol::Caller);
    assert!(
        ExprId::operation(
            opcode::ADD,
            &[x.clone(), x],
            ExprLimits {
                max_nodes: 2,
                max_depth: 2
            }
        )
        .is_err()
    );
}
#[test]
fn summary_rename_changes_internal_runtime_leaves_and_preserves_inputs() {
    let input = ExprId::input(2, Symbol::Caller);
    let local = ExprId::fresh().unwrap();
    let replacement = ExprId::fresh().unwrap();
    let expression =
        ExprId::operation(opcode::ADD, &[input, local.clone()], ExprLimits::default()).unwrap();
    let renamed = expression
        .rename_fresh(
            &BTreeMap::from([(local, replacement.clone())]),
            ExprLimits::default(),
        )
        .unwrap();
    assert_eq!(expression.inputs(), renamed.inputs());
    assert_eq!(renamed.fresh_leaves(), [replacement].into_iter().collect());
}
#[test]
fn constant_folding_preserves_evm_modular_and_division_rules() {
    let constant = |v| ExprId::constant(v);
    let addition = ExprId::operation(
        opcode::ADD,
        &[constant(U256::MAX), constant(U256::from(1))],
        ExprLimits::default(),
    )
    .unwrap();
    assert_eq!(addition.as_constant(), Some(U256::ZERO));
    let division = ExprId::operation(
        opcode::DIV,
        &[constant(U256::MAX), constant(U256::ZERO)],
        ExprLimits::default(),
    )
    .unwrap();
    assert_eq!(division.as_constant(), Some(U256::ZERO));
}

#[test]
fn canonicalization_preserves_scope_and_evm_shift_operand_order() {
    let x = ExprId::input(1, Symbol::CallValue);
    let zero = ExprId::constant(U256::ZERO);
    assert_eq!(
        ExprId::operation(
            opcode::ADD,
            &[x.clone(), zero.clone()],
            ExprLimits::default()
        )
        .unwrap(),
        x
    );
    assert_eq!(
        ExprId::operation(
            opcode::SHL,
            &[zero.clone(), x.clone()],
            ExprLimits::default()
        )
        .unwrap(),
        x
    );
    assert_eq!(
        ExprId::operation(opcode::SHL, &[x.clone(), zero], ExprLimits::default())
            .unwrap()
            .as_constant(),
        None
    );
    assert_eq!(
        ExprId::operation(opcode::XOR, &[x.clone(), x], ExprLimits::default())
            .unwrap()
            .as_constant(),
        Some(U256::ZERO)
    );
}

#[test]
fn expanded_tree_overflow_is_rejected_even_with_maximal_configured_limits() {
    let limits = ExprLimits {
        max_nodes: usize::MAX,
        max_depth: usize::MAX,
    };
    let mut expression = ExprId::input(1, Symbol::CallValue);
    let mut rejected = false;
    for _ in 0..usize::BITS {
        match ExprId::operation(
            opcode::ADD,
            &[expression.clone(), expression.clone()],
            limits,
        ) {
            Ok(next) => expression = next,
            Err(super::ExprError::Limit { .. }) => {
                rejected = true;
                break;
            }
            Err(error) => panic!("unexpected limit result: {error}"),
        }
    }
    assert!(rejected);
}

#[test]
fn complete_ordered_byte_extractions_recover_only_the_same_scoped_word() {
    let word = ExprId::input(1, Symbol::CallValue);
    let mut bytes: Vec<_> = (0..32)
        .map(|i| {
            ExprId::operation(
                opcode::BYTE,
                &[ExprId::constant(U256::from(i)), word.clone()],
                ExprLimits::default(),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(ExprId::reassemble_word(&bytes), Some(word));
    assert_eq!(ExprId::reassemble_word(&bytes[..31]), None);
    bytes.swap(0, 1);
    assert_eq!(ExprId::reassemble_word(&bytes), None);
    bytes.swap(0, 1);
    bytes[31] = ExprId::operation(
        opcode::BYTE,
        &[
            ExprId::constant(U256::from(31)),
            ExprId::input(2, Symbol::CallValue),
        ],
        ExprLimits::default(),
    )
    .unwrap();
    assert_eq!(ExprId::reassemble_word(&bytes), None);
}
