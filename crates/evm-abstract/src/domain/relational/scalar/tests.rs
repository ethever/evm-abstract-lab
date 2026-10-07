use super::{ScalarQuery, failed};
use crate::domain::{
    Domain, NumericValue,
    facts::{FactError, Symbol},
    identity,
    symbolic::{ExprId, ExprLimits},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::num::NonZeroUsize;

#[test]
fn malformed_or_metadata_facts_never_claim_numerical_bottom() {
    for error in [
        FactError::InvalidBitIndex(256),
        FactError::InvalidModulus,
        FactError::UnsupportedOperation(0xff),
        FactError::InvalidOperation {
            opcode: opcode::EQ,
            expected: 2,
            actual: 1,
        },
        FactError::OriginContradiction {
            subject: Symbol::THIS,
        },
    ] {
        assert!(matches!(failed(error), ScalarQuery::Unknown(_)));
    }
    for error in [
        FactError::ConflictingBits,
        FactError::InvalidBounds,
        FactError::EmptyFiniteSet,
        FactError::Contradiction {
            subject: Symbol::THIS,
        },
    ] {
        assert_eq!(failed(error), ScalarQuery::Infeasible);
    }
}
#[test]
fn scalar_projection_preserves_the_constants_only_numeric_profile() {
    let variable = ExprId::input(1, identity::Symbol::CallValue);
    let condition = ExprId::operation(
        opcode::LT,
        &[variable.clone(), ExprId::constant(U256::from(10))],
        ExprLimits::default(),
    )
    .unwrap();
    let state = super::super::RelationState {
        constraints: [super::super::Constraint::Truth {
            expression: condition,
            nonzero: true,
        }]
        .into_iter()
        .collect(),
        bottom: false,
    };
    let result = state.refine_numeric(
        &variable,
        &NumericValue::top(),
        Domain::new(NonZeroUsize::new(8).unwrap()),
        &super::super::RelationLimits::default(),
    );
    if let ScalarQuery::Refined { numeric, .. } = result {
        assert!(numeric.is_top());
    } else {
        panic!("expected sound coarse constants-only projection: {result:?}");
    }
}
