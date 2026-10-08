use super::{CheckResult, RelationLimits, RelationState, ValueQuery};
use crate::domain::{
    identity::Symbol,
    symbolic::{ExprId, ExprLimits},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn op(opcode: u8, args: &[ExprId]) -> ExprId {
    ExprId::operation(
        opcode,
        args,
        ExprLimits {
            max_nodes: 4096,
            max_depth: 64,
        },
    )
    .unwrap()
}
fn limits() -> RelationLimits {
    RelationLimits {
        max_nodes: 4096,
        rlimit: 10_000_000,
        ..RelationLimits::default()
    }
}
fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}
#[test]
fn incompatible_branch_conditions_are_unsat_with_a_retained_input() {
    let x = ExprId::input(1, Symbol::CallValue);
    let mut state = RelationState::default();
    assert_eq!(
        state.assume(&op(opcode::EQ, &[x.clone(), c(1)]), true, &limits()),
        CheckResult::Sat
    );
    assert_eq!(
        state.assume(&op(opcode::EQ, &[x, c(2)]), true, &limits()),
        CheckResult::Unsat
    );
    assert!(state.is_bottom());
}
#[test]
fn distinct_input_scopes_remain_independent_not_disequal() {
    let left = ExprId::input(1, Symbol::Caller);
    let right = ExprId::input(2, Symbol::Caller);
    let equality = op(opcode::EQ, &[left, right]);
    assert_eq!(
        RelationState::default().assume(&equality, true, &limits()),
        CheckResult::Sat
    );
    assert_eq!(
        RelationState::default().assume(&equality, false, &limits()),
        CheckResult::Sat
    );
}
#[test]
fn join_covers_both_paths_instead_of_conjoining_their_conditions() {
    let x = ExprId::input(1, Symbol::CallValue);
    let mut a = RelationState::default();
    let mut b = a.clone();
    a.assume(&op(opcode::EQ, &[x.clone(), c(1)]), true, &limits());
    b.assume(&op(opcode::EQ, &[x.clone(), c(2)]), true, &limits());
    let joined = a.join(&b);
    assert!(!joined.is_bottom());
    assert_eq!(
        joined.can_equal(&x, U256::from(1), &limits()),
        CheckResult::Sat
    );
    assert_eq!(
        joined.can_equal(&x, U256::from(2), &limits()),
        CheckResult::Sat
    );
}
#[test]
fn budget_unknown_never_becomes_unsat_or_a_fabricated_constant() {
    let x = ExprId::input(1, Symbol::CallValue);
    let limits = RelationLimits {
        max_nodes: 1,
        ..limits()
    };
    assert!(matches!(
        RelationState::default().unique_value(&op(opcode::ADD, &[x, c(1)]), &limits),
        ValueQuery::Unknown(_)
    ));
}
#[test]
fn arithmetic_relation_refines_other_values_and_wraps_as_evm_words() {
    let x = ExprId::input(1, Symbol::CallValue);
    let next = op(opcode::ADD, &[x.clone(), c(1)]);
    let mut state = RelationState::default();
    state.assume(&op(opcode::EQ, &[next, c(0)]), true, &limits());
    assert_eq!(
        state.unique_value(&x, &limits()),
        ValueQuery::Unique(U256::MAX)
    );
}
#[test]
fn all_exact_encodings_match_the_shared_evm_oracle_at_boundaries() {
    let values = [
        U256::ZERO,
        U256::from(1),
        U256::from(255),
        U256::from(256),
        U256::from(1) << 255,
        U256::MAX,
    ];
    let ops = [
        opcode::ADD,
        opcode::SUB,
        opcode::MUL,
        opcode::DIV,
        opcode::SDIV,
        opcode::MOD,
        opcode::SMOD,
        opcode::ADDMOD,
        opcode::MULMOD,
        opcode::SIGNEXTEND,
        opcode::LT,
        opcode::GT,
        opcode::SLT,
        opcode::SGT,
        opcode::EQ,
        opcode::ISZERO,
        opcode::AND,
        opcode::OR,
        opcode::XOR,
        opcode::NOT,
        opcode::BYTE,
        opcode::SHL,
        opcode::SHR,
        opcode::SAR,
        opcode::CLZ,
    ];
    for provider in [
        super::SmtProvider::Z3,
        super::SmtProvider::Bitwuzla,
        super::SmtProvider::Cvc5,
    ] {
        let policy = RelationLimits {
            provider,
            ..limits()
        };
        for (index, opcode) in ops.into_iter().enumerate() {
            for (offset, a) in values.into_iter().enumerate() {
                let b = values[(index + offset + 1) % values.len()];
                let modulus = values[(offset + 2) % values.len()];
                let count = crate::domain::symbolic::arity(opcode).unwrap();
                let variables = [
                    ExprId::fresh().unwrap(),
                    ExprId::fresh().unwrap(),
                    ExprId::fresh().unwrap(),
                ];
                let actuals = [a, b, modulus];
                let mut state = RelationState::default();
                for (variable, value) in variables.iter().zip(actuals).take(count) {
                    assert_eq!(
                        state.add_unsigned_bounds(variable, value, value, &policy),
                        CheckResult::Sat
                    );
                }
                let expression = op(opcode, &variables[..count]);
                let expected = crate::domain::concrete::evaluate(opcode, a, b, modulus);
                assert_eq!(
                    state.unique_value(&expression, &policy),
                    ValueQuery::Unique(expected),
                    "provider={provider},opcode={opcode:x},a={a},b={b},c={modulus}"
                );
            }
        }
    }
}

#[test]
fn scalar_observation_imports_bounds_bits_and_constants_without_solver_roundtrips() {
    let x = ExprId::input(1, Symbol::CallValue);
    let mut state = RelationState::default();
    state
        .observe(&x, &crate::domain::NumericValue::unknown_byte(), &limits())
        .unwrap();
    assert_eq!(
        state.can_equal(&x, U256::from(256), &limits()),
        CheckResult::Unsat
    );
    assert_eq!(state.unique_value(&x, &limits()), ValueQuery::Multiple);
    state
        .observe(
            &x,
            &crate::domain::NumericValue::constant(U256::from(7)),
            &limits(),
        )
        .unwrap();
    assert_eq!(
        state.unique_value(&x, &limits()),
        ValueQuery::Unique(U256::from(7))
    );
}

#[test]
fn constraint_capacity_preserves_prior_guarantees_without_claiming_bottom() {
    let x = ExprId::input(1, Symbol::CallValue);
    let policy = RelationLimits {
        max_constraints: 1,
        ..limits()
    };
    let mut state = RelationState::default();
    assert_eq!(
        state.assume(&op(opcode::EQ, &[x.clone(), c(1)]), true, &policy),
        CheckResult::Sat
    );
    assert_eq!(
        state.assume(&op(opcode::EQ, &[x.clone(), c(2)]), true, &policy),
        CheckResult::Unknown(super::QueryReason::ConstraintLimit)
    );
    assert!(!state.is_bottom());
    assert_eq!(
        state.unique_value(&x, &policy),
        ValueQuery::Unique(U256::from(1))
    );
}

#[test]
fn default_fuel_handles_a_scoped_address_equality_without_redundant_bool_circuits() {
    let caller = ExprId::input(1, Symbol::Caller);
    let condition = op(opcode::EQ, &[caller.clone(), c(1)]);
    let numeric_condition = crate::domain::Domain::default().apply(
        opcode::EQ,
        &[
            crate::domain::AbstractValue::unknown_address(),
            crate::domain::AbstractValue::constant(U256::from(1)),
        ],
    );
    let policy = RelationLimits::default();
    let mut state = RelationState::default();
    state
        .observe(
            &caller,
            &crate::domain::NumericValue::unknown_address(),
            &policy,
        )
        .unwrap();
    state
        .observe(&condition, numeric_condition.numeric(), &policy)
        .unwrap();
    assert_eq!(
        state.len(),
        1,
        "address mask alone implies its unsigned bounds and comparison word has built-in boolean semantics"
    );
    assert_eq!(state.assume(&condition, true, &policy), CheckResult::Sat);
    assert_eq!(
        state.unique_value(&caller, &policy),
        ValueQuery::Unique(U256::from(1))
    );
}

#[test]
fn deterministic_resource_exhaustion_is_unknown_and_cannot_poison_a_fresh_query() {
    let x = ExprId::input(1, Symbol::CallValue);
    let condition = op(opcode::EQ, &[x.clone(), c(1)]);
    let mut state = RelationState::default();
    let tiny = RelationLimits {
        rlimit: 1,
        ..RelationLimits::default()
    };
    assert_eq!(
        state.assume(&condition, true, &tiny),
        CheckResult::Unknown(super::QueryReason::ResourceLimit)
    );
    assert!(!state.is_bottom());
    assert_eq!(
        state.unique_value(&x, &RelationLimits::default()),
        ValueQuery::Unique(U256::from(1))
    );
}

#[test]
fn direct_boolean_encoding_still_honors_the_expression_depth_limit() {
    let x = ExprId::input(1, Symbol::CallValue);
    let condition = op(opcode::EQ, &[x, c(1)]);
    let mut state = RelationState::default();
    assert_eq!(
        state.assume(&condition, true, &RelationLimits::default()),
        CheckResult::Sat
    );
    assert_eq!(
        state.check(&RelationLimits {
            max_depth: 1,
            ..RelationLimits::default()
        }),
        CheckResult::Unknown(super::QueryReason::ExpressionLimit)
    );
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(24))]
    #[test]
    fn native_word_operations_match_evm_for_arbitrary_full_width_operands(
        limbs_a in proptest::arbitrary::any::<[u64;4]>(),
        limbs_b in proptest::arbitrary::any::<[u64;4]>(),
        selected in 0usize..8,
    ) {
        let a = U256::from_limbs(limbs_a); let b = U256::from_limbs(limbs_b);
        let opcode = [opcode::ADD, opcode::SUB, opcode::MUL, opcode::XOR, opcode::SHL, opcode::SHR, opcode::SAR, opcode::SDIV][selected];
        let left = ExprId::fresh().unwrap(); let right = ExprId::fresh().unwrap();
        let mut state = RelationState::default();
        state.add_unsigned_bounds(&left,a,a,&limits()); state.add_unsigned_bounds(&right,b,b,&limits());
        let expression = op(opcode,&[left,right]);
        proptest::prop_assert_eq!(state.unique_value(&expression,&limits()), ValueQuery::Unique(crate::domain::concrete::evaluate(opcode,a,b,U256::ZERO)));
    }
}

#[test]
fn default_fuel_handles_false_address_guard_and_unrelated_value_projection() {
    let caller = ExprId::input(1, Symbol::Caller);
    let other = ExprId::input(1, Symbol::CallValue);
    let condition = op(opcode::EQ, &[caller.clone(), c(1)]);
    let mut state = RelationState::default();
    let policy = RelationLimits::default();
    state
        .observe(
            &caller,
            &crate::domain::NumericValue::unknown_address(),
            &policy,
        )
        .unwrap();
    assert_eq!(state.assume(&condition, false, &policy), CheckResult::Sat);
    assert!(state.relevant(&caller));
    assert!(!state.relevant(&other));
    assert_eq!(state.unique_value(&caller, &policy), ValueQuery::Multiple);
    assert_eq!(state.unique_value(&other, &policy), ValueQuery::Multiple);
}

#[test]
fn exact_exponentiation_handles_symbolic_base_and_literal_exponent_boundaries() {
    for provider in [
        super::SmtProvider::Z3,
        super::SmtProvider::Bitwuzla,
        super::SmtProvider::Cvc5,
    ] {
        let policy = RelationLimits {
            provider,
            ..RelationLimits::default()
        };
        for (base, exponent) in [
            (U256::from(3), U256::ZERO),
            (U256::from(3), U256::from(1)),
            (U256::from(3), U256::from(2)),
            (U256::from(3), U256::from(255)),
            (U256::from(1), U256::MAX),
            (U256::ZERO, U256::from(256)),
        ] {
            let variable = ExprId::input(1, Symbol::CallValue);
            let mut state = RelationState::default();
            state
                .observe(
                    &variable,
                    &crate::domain::NumericValue::constant(base),
                    &policy,
                )
                .unwrap();
            let expression = op(opcode::EXP, &[variable, ExprId::constant(exponent)]);
            assert_eq!(
                state.unique_value(&expression, &policy),
                ValueQuery::Unique(base.wrapping_pow(exponent)),
                "provider={provider}, base={base}, exponent={exponent}"
            );
        }
    }
}
#[test]
fn full_symbolic_exponentiation_respects_encoding_and_native_resource_limits() {
    let expression = op(
        opcode::EXP,
        &[
            ExprId::input(1, Symbol::CallValue),
            ExprId::input(1, Symbol::GasPrice),
        ],
    );
    assert_eq!(
        RelationState::default().unique_value(&expression, &RelationLimits::default()),
        ValueQuery::Unknown(super::QueryReason::ExpressionLimit)
    );
    for provider in [
        super::SmtProvider::Z3,
        super::SmtProvider::Bitwuzla,
        super::SmtProvider::Cvc5,
    ] {
        let native = RelationLimits {
            provider,
            max_nodes: 4096,
            rlimit: 1,
            ..RelationLimits::default()
        };
        assert_eq!(
            RelationState::default().unique_value(&expression, &native),
            ValueQuery::Unknown(super::QueryReason::ResourceLimit)
        );
    }
}

#[test]
fn symbolic_square_is_equal_to_modular_multiplication_without_nested_model_terms() {
    let base = ExprId::input(1, Symbol::CallValue);
    let square = op(opcode::EXP, &[base.clone(), c(2)]);
    let product = op(opcode::MUL, &[base.clone(), base]);
    let mut state = RelationState::default();
    assert_eq!(
        state.assume(
            &op(opcode::EQ, &[square, product]),
            false,
            &RelationLimits::default()
        ),
        CheckResult::Unsat
    );
}
#[test]
fn observed_constant_substitution_preserves_intrinsic_address_bounds() {
    let address = ExprId::input(1, Symbol::Caller);
    let mut state = RelationState::default();
    state
        .observe(
            &address,
            &crate::domain::NumericValue::constant(U256::MAX),
            &RelationLimits::default(),
        )
        .unwrap();
    assert_eq!(state.check(&RelationLimits::default()), CheckResult::Unsat);
}

fn direct_guard_state(expression: ExprId, truth: bool) -> RelationState {
    RelationState {
        constraints: [super::Constraint::Truth {
            expression,
            nonzero: truth,
        }]
        .into_iter()
        .collect(),
        bottom: false,
    }
}
#[test]
fn cheap_guard_projection_preserves_every_matching_word_at_unsigned_and_signed_boundaries() {
    let boundaries = [
        U256::ZERO,
        U256::from(1),
        (U256::from(1) << 255) - U256::from(1),
        U256::from(1) << 255,
        U256::MAX,
    ];
    for opcode in [opcode::EQ, opcode::LT, opcode::GT, opcode::SLT, opcode::SGT] {
        for constant in boundaries {
            for variable_left in [true, false] {
                for truth in [true, false] {
                    let variable = ExprId::input(1, Symbol::CallValue);
                    let arguments = if variable_left {
                        [variable.clone(), ExprId::constant(constant)]
                    } else {
                        [ExprId::constant(constant), variable.clone()]
                    };
                    let state = direct_guard_state(op(opcode, &arguments), truth);
                    let result = state.refine_numeric(
                        &variable,
                        &crate::domain::NumericValue::top(),
                        crate::domain::Domain::default(),
                        &RelationLimits::default(),
                    );
                    for candidate in boundaries {
                        let (a, b) = if variable_left {
                            (candidate, constant)
                        } else {
                            (constant, candidate)
                        };
                        let actual = crate::domain::concrete::evaluate(opcode, a, b, U256::ZERO)
                            != U256::ZERO;
                        if actual == truth {
                            match &result {
                                super::ScalarQuery::Refined { numeric, .. } => assert!(
                                    numeric.contains(candidate),
                                    "opcode={opcode:x},c={constant},candidate={candidate},truth={truth},left={variable_left}"
                                ),
                                super::ScalarQuery::Unchanged | super::ScalarQuery::Unknown(_) => {}
                                super::ScalarQuery::Infeasible => panic!(
                                    "pruned feasible guard: opcode={opcode:x},c={constant},v={candidate},truth={truth}"
                                ),
                            }
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn cheap_loop_guard_refines_the_derived_expression_to_nine_without_a_native_query() {
    let x = ExprId::fresh().unwrap();
    let next = op(opcode::ADD, &[x, c(1)]);
    let state = direct_guard_state(op(opcode::LT, &[next.clone(), c(10)]), true);
    match state.refine_numeric(
        &next,
        &crate::domain::NumericValue::top(),
        crate::domain::Domain::default(),
        &RelationLimits::default(),
    ) {
        super::ScalarQuery::Refined { numeric, complete } => {
            assert_eq!(
                numeric.interval().unsigned_bounds(),
                (U256::ZERO, U256::from(9))
            );
            assert!(complete);
        }
        other => panic!("expected cheap direct bound: {other:?}"),
    }
}
#[test]
fn cheap_fact_capacity_cannot_be_confused_with_contradiction() {
    let variable = ExprId::input(1, Symbol::CallValue);
    let state = direct_guard_state(op(opcode::LT, &[variable.clone(), c(10)]), true);
    let policy = crate::domain::DomainSpec::new(
        crate::domain::Profile::Product,
        std::num::NonZeroUsize::new(8).unwrap(),
        std::num::NonZeroUsize::new(4).unwrap(),
        std::num::NonZeroUsize::new(1).unwrap(),
    );
    assert_eq!(
        state.refine_numeric(
            &variable,
            &crate::domain::NumericValue::top(),
            crate::domain::Domain::from_spec(policy),
            &RelationLimits::default()
        ),
        super::ScalarQuery::Unknown(super::QueryReason::ScalarFactLimit)
    );
}

#[test]
fn default_fuel_handles_masked_derived_loop_guard_with_named_arithmetic() {
    for truth in [true, false] {
        let x = ExprId::fresh().unwrap();
        let next = op(opcode::ADD, &[x, c(1)]);
        let mut numeric =
            crate::domain::NumericValue::unsigned_range(U256::ZERO, U256::from(15)).unwrap();
        numeric.bits =
            crate::domain::known_bits::KnownBits::new(U256::MAX << 4usize, U256::ZERO).unwrap();
        let mut state = RelationState::default();
        let policy = RelationLimits::default();
        state.observe(&next, &numeric, &policy).unwrap();
        assert_eq!(
            state.assume(&op(opcode::LT, &[next, c(10)]), truth, &policy),
            CheckResult::Sat,
            "truth={truth}"
        );
    }
}

#[test]
fn every_provider_preserves_uniqueness_path_relations_and_evm_wrapping() {
    for provider in [
        super::SmtProvider::Z3,
        super::SmtProvider::Bitwuzla,
        super::SmtProvider::Cvc5,
    ] {
        let policy = RelationLimits {
            provider,
            ..limits()
        };
        let x = ExprId::input(1, Symbol::CallValue);
        let next = op(opcode::ADD, &[x.clone(), c(1)]);
        let mut state = RelationState::default();
        assert_eq!(
            state.assume(&op(opcode::EQ, &[next, c(0)]), true, &policy),
            CheckResult::Sat,
            "{provider}"
        );
        assert_eq!(
            state.unique_value(&x, &policy),
            ValueQuery::Unique(U256::MAX),
            "{provider}"
        );
        assert_eq!(
            state.can_equal(&x, U256::ZERO, &policy),
            CheckResult::Unsat,
            "{provider}"
        );
        let mut range = RelationState::default();
        assert_eq!(
            range.add_unsigned_bounds(&x, U256::from(1), U256::from(2), &policy),
            CheckResult::Sat
        );
        assert_eq!(
            range.unique_value(&x, &policy),
            ValueQuery::Multiple,
            "{provider}"
        );
        let same = op(opcode::EQ, &[x.clone(), x]);
        assert_eq!(
            RelationState::default().assume(&same, false, &policy),
            CheckResult::Unsat,
            "{provider}"
        );
    }
}
