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
fn masked_target_equality_refines_only_the_proved_bits() {
    let limits = super::super::RelationLimits::default();
    let variable = ExprId::fresh().unwrap();
    let mask = (U256::ONE << 160usize) - U256::ONE;
    let address = U256::from(0x101);
    for swapped in [false, true] {
        let args = if swapped {
            [ExprId::constant(mask), variable.clone()]
        } else {
            [variable.clone(), ExprId::constant(mask)]
        };
        let projected = ExprId::operation(opcode::AND, &args, limits.expressions()).unwrap();
        let equality = ExprId::operation(
            opcode::EQ,
            &[projected.clone(), ExprId::constant(address)],
            limits.expressions(),
        )
        .unwrap();
        for constraint in [
            super::super::Constraint::Truth {
                expression: equality.clone(),
                nonzero: true,
            },
            super::super::Constraint::Equal {
                left: ExprId::constant(address),
                right: projected,
            },
        ] {
            let relations = super::super::RelationState {
                constraints: [constraint].into_iter().collect(),
                bottom: false,
                path: Default::default(),
            };
            let ScalarQuery::Refined { numeric, .. } = relations.refine_numeric(
                &variable,
                &NumericValue::top(),
                Domain::default(),
                &limits,
            ) else {
                panic!("masked equality must translate into known bits");
            };
            assert!(numeric.contains(address));
            assert!(numeric.contains((U256::MAX & !mask) | address));
            assert!(!numeric.contains(address + U256::ONE));
            assert!(
                numeric.singleton().is_none(),
                "unknown high bits must remain unknown"
            );
        }
        let excluded = super::super::RelationState {
            constraints: [super::super::Constraint::Truth {
                expression: equality,
                nonzero: false,
            }]
            .into_iter()
            .collect(),
            bottom: false,
            path: Default::default(),
        };
        assert_eq!(
            excluded.refine_numeric(&variable, &NumericValue::top(), Domain::default(), &limits),
            ScalarQuery::Unchanged
        );
    }
}

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
        assert_eq!(
            failed(error.clone()),
            ScalarQuery::Unknown(super::QueryReason::ScalarFactError(error))
        );
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
        path: Default::default(),
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

#[test]
fn malformed_operation_and_origin_subject_survive_serialized_query_boundaries() {
    let errors = [
        FactError::InvalidOperation {
            opcode: opcode::ADD,
            expected: 2,
            actual: 1,
        },
        FactError::OriginContradiction {
            subject: Symbol::new(17),
        },
    ];
    let reports: Vec<_> = errors
        .into_iter()
        .map(|error| serde_json::to_value(failed(error)).unwrap())
        .collect();
    let operation = &reports[0]["Unknown"]["ScalarFactError"]["InvalidOperation"];
    assert_eq!(operation["opcode"], opcode::ADD);
    assert_eq!(operation["expected"], 2);
    assert_eq!(operation["actual"], 1);
    assert_eq!(
        reports[1]["Unknown"]["ScalarFactError"]["OriginContradiction"]["subject"],
        17
    );
}
