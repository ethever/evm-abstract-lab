use crate::{
    domain::{
        AbstractValue, Domain, DomainSpec, EvmNumericAlgebra, FiniteConstantSet, NumericValue,
        Profile, ReductionStatus, ValueAlgebra, WordElement, WordObservation,
        congruence::Congruence,
        cooperation::{self, BoundsToMask, MaskToBounds, ProductAlgebra, test_support::Count},
        interval::Interval,
        known_bits::KnownBits,
        relational::{CheckResult, EvmScalarQuery, RelationLimits, RelationState},
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::{cell::Cell, num::NonZeroUsize, rc::Rc};

fn domain(capacity: usize, rounds: usize) -> Domain {
    Domain::from_spec(DomainSpec::new(
        Profile::Product,
        NonZeroUsize::new(capacity).unwrap(),
        NonZeroUsize::new(rounds).unwrap(),
        NonZeroUsize::new(4096).unwrap(),
    ))
}

fn cycle() -> NumericValue {
    NumericValue {
        bits: KnownBits::new(U256::ZERO, U256::ONE).unwrap(),
        interval: Interval::new_unsigned(U256::from(2), U256::from(8)).unwrap(),
        ..NumericValue::top()
    }
}

fn finite(size: usize) -> NumericValue {
    NumericValue {
        finite: FiniteConstantSet::try_from_values((0..size).map(U256::from).collect()).unwrap(),
        ..NumericValue::top()
    }
}

#[test]
fn transfer_fact_admission_matches_the_fact_table_and_preserves_refusal_statuses() {
    use crate::domain::{
        NumericReduction,
        facts::{Fact, FactLattice, OperationRelation, RelationalFact, Symbol, Term},
    };
    for (op, count) in [(opcode::NOT, 1), (opcode::ADD, 2), (opcode::ADDMOD, 3)] {
        let args = vec![NumericValue::top(); count];
        let relation = OperationRelation::new(
            op,
            Symbol::THIS,
            (1..=count)
                .map(|i| Term::Symbol(Symbol::new(i as u32)))
                .collect(),
        )
        .unwrap();
        for capacity in 1..=4 {
            let domain = Domain::from_spec(DomainSpec::new(
                Profile::Product,
                NonZeroUsize::new(8).unwrap(),
                NonZeroUsize::new(4).unwrap(),
                NonZeroUsize::new(capacity).unwrap(),
            ));
            let admitted = FactLattice::new(capacity)
                .insert(Fact::Relation(RelationalFact::Operation(relation.clone())))
                .is_ok();
            for status in [
                ReductionStatus::Stable,
                ReductionStatus::RoundLimit,
                ReductionStatus::Empty,
                ReductionStatus::OriginConflict,
                ReductionStatus::SymbolicLimit,
                ReductionStatus::WorkLimit,
                ReductionStatus::Cancelled,
                ReductionStatus::KernelFailure,
            ] {
                let result = domain.numeric_apply_with(op, args.iter(), |value| NumericReduction {
                    value,
                    status,
                    rounds: 7,
                    strengthened: 11,
                });
                let expected = if admitted
                    || matches!(
                        status,
                        ReductionStatus::WorkLimit
                            | ReductionStatus::Cancelled
                            | ReductionStatus::KernelFailure
                    ) {
                    status
                } else {
                    ReductionStatus::FactLimit
                };
                assert_eq!(result.status, expected, "op={op:#x}, capacity={capacity}");
                assert_eq!((result.rounds, result.strengthened), (7, 11));
            }
        }
    }
}

#[test]
fn closure_quote_funds_rule_and_graph_reservations_across_capacity_and_step_limits() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for capacity in [1, 8, 32, 512] {
        for rounds in [1, 2, 4, 8] {
            let domain = domain(capacity, rounds);
            for input in [
                NumericValue::top(),
                NumericValue::constant(U256::from(7)),
                cycle(),
                NumericValue::unsigned_range(U256::from(2), U256::from(100)).unwrap(),
                finite(capacity),
                finite(capacity + 1),
                finite(1024),
            ] {
                let masks = Cell::new(0);
                let bounds = Cell::new(0);
                let algebra = ProductAlgebra::new(
                    domain.spec(),
                    Count {
                        rule: BoundsToMask,
                        calls: &masks,
                    },
                    Count {
                        rule: MaskToBounds,
                        calls: &bounds,
                    },
                );
                let result = algebra.reduce_numeric(&input);
                let (used, dependencies, steps) = cooperation::PROTOCOL_WORK.get();
                let quote = domain.numeric_refinement_work(&input, 0);
                assert!(
                    used <= quote,
                    "capacity={capacity} rounds={rounds}: {used}>{quote}"
                );
                assert_eq!(dependencies, 10);
                assert!(steps <= 1 + 3 * rounds);
                assert!(result.rounds <= bounds.get());
                let semantic = masks.get() * 256
                    + bounds.get() * 4096
                    + result.rounds * (4096 + 256 * capacity + 32 * (capacity + 32));
                assert!(used > semantic, "graph/meet reservations must remain paid");
                if input.constants().is_some() && rounds >= 2 {
                    assert_eq!(result.status, ReductionStatus::Stable);
                    assert!(result.rounds <= 2 && masks.get() <= 2 && bounds.get() <= 2);
                }
            }
        }
    }
}

#[test]
fn stable_inputs_pay_one_cycle_and_finite_inputs_pay_at_most_two() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for capacity in [1, 8, 512] {
        let one = domain(capacity, 1);
        let two = domain(capacity, 2);
        let eight = domain(capacity, 8);
        for value in [NumericValue::top(), NumericValue::constant(U256::from(7))] {
            assert_eq!(
                one.numeric_refinement_work(&value, 0),
                eight.numeric_refinement_work(&value, 0)
            );
            let result = eight.reduce_numeric(&value);
            assert_eq!(result.status, ReductionStatus::Stable);
            assert_eq!(result.value, value);
            assert_eq!(result.rounds, 1);
            assert_eq!(cooperation::PROTOCOL_WORK.get().2, 4);
        }
        let value = finite(128);
        assert_eq!(
            two.numeric_refinement_work(&value, 1),
            eight.numeric_refinement_work(&value, 1)
        );
        assert!(
            eight.numeric_refinement_work(&NumericValue::top(), 1)
                > one.numeric_refinement_work(&NumericValue::top(), 1),
            "new facts can turn Top into a product requiring feedback"
        );
        let literal = NumericValue::constant(U256::from(7));
        assert_eq!(
            one.numeric_refinement_work(&literal, 1),
            eight.numeric_refinement_work(&literal, 1)
        );
        let algebra = ValueAlgebra::new(eight);
        let refined = algebra
            .constrain_numeric(&literal, &WordObservation::nonzero_word())
            .unwrap();
        assert_eq!(refined.value, literal);
        assert_eq!(refined.status, ReductionStatus::Stable);
        assert!(cooperation::PROTOCOL_WORK.get().0 <= eight.numeric_refinement_work(&literal, 1));
    }
}

#[test]
fn closure_coupon_itself_funds_stable_inputs_without_import_allowance() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for capacity in [1, 8, 512] {
        let domain = domain(capacity, 8);
        for value in [NumericValue::top(), NumericValue::constant(U256::from(7))] {
            // This is the closure tariff alone. The surrounding import/fact
            // envelope must not conceal underpriced kernel work or reporting.
            let coupon = domain.numeric_closure_work(value.finite.work_size(), Some(1));
            cooperation::PROTOCOL_WORK.set((0, 0, 0));
            let result = cooperation::close_with_work(
                domain,
                value.clone(),
                BoundsToMask,
                MaskToBounds,
                coupon,
            );
            assert_eq!(
                result.status,
                ReductionStatus::Stable,
                "capacity={capacity}"
            );
            assert_eq!(result.value, value);
            assert_eq!(result.rounds, 1);
            let (used, dependencies, steps) = cooperation::PROTOCOL_WORK.get();
            assert!(used <= coupon);
            assert_eq!((dependencies, steps), (10, 4));
        }
    }
}

#[test]
fn filtered_finite_covers_stabilize_without_capacity_truncation() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for capacity in [1, 8, 32] {
        for size in [4, 9, 128] {
            let domain = domain(capacity, 8);
            let value = NumericValue {
                bits: KnownBits::new(U256::ZERO, U256::ONE).unwrap(),
                interval: Interval::new_unsigned(U256::from(2), U256::from(size - 1)).unwrap(),
                ..finite(size)
            };
            let result = domain.reduce_numeric(&value);
            assert_eq!(result.status, ReductionStatus::Stable);
            assert!(result.rounds <= 2);
            assert_eq!(
                result.value.constants().unwrap(),
                &(3..size).step_by(2).map(U256::from).collect()
            );
            assert!(
                cooperation::PROTOCOL_WORK.get().0 <= domain.numeric_refinement_work(&value, 0)
            );
        }
    }
}

#[test]
fn arithmetic_reservation_is_atomic_and_covers_the_actual_cooperative_schedule() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    let _copies = crate::domain::algebra::OPERAND_COPIES.scope();
    for capacity in [1, 8, 512] {
        for rounds in [1, 2, 4, 8] {
            let domain = ValueAlgebra::new(domain(capacity, rounds));
            for (op, arity) in (0x01..=0x0b).chain(0x10..=0x1e).map(|op| {
                let arity = match op {
                    opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
                    opcode::ADDMOD | opcode::MULMOD => 3,
                    _ => 2,
                };
                (op, arity)
            }) {
                for input in [NumericValue::top(), cycle(), finite(capacity + 1)] {
                    let mut args = vec![AbstractValue::from_numeric(input)];
                    args.resize(arity, AbstractValue::constant(U256::from(7)));
                    let quote = domain.operation_work(&args);
                    cooperation::PROTOCOL_WORK.set((0, 0, 0));
                    let mut denied = WorkBudget::new(quote - 1);
                    assert!(domain.apply_budgeted(op, &args, &mut denied).is_err());
                    assert_eq!(denied.used(), 0);
                    assert!(denied.exhausted());
                    assert_eq!(cooperation::PROTOCOL_WORK.get(), (0, 0, 0));
                    assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 0);
                    let mut paid = WorkBudget::new(quote);
                    let actual = domain.apply_budgeted(op, &args, &mut paid).unwrap();
                    assert!(
                        !matches!(
                            actual.status,
                            ReductionStatus::WorkLimit
                                | ReductionStatus::Cancelled
                                | ReductionStatus::KernelFailure
                        ),
                        "the paid quote must fund the schedule, op={op:02x} rounds={rounds}"
                    );
                    assert_eq!(paid.used(), quote);
                    assert!(!paid.exhausted());
                    assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 0);
                    let (used, dependencies, steps) = cooperation::PROTOCOL_WORK.get();
                    assert!(used <= quote);
                    assert!(
                        dependencies <= 32,
                        "one scalar generation has only three facts and four jobs"
                    );
                    assert!(steps <= 1 + 3 * rounds);
                    let expected = domain.apply_detailed(op, &args);
                    assert_eq!(actual.value, expected.value);
                    assert_eq!(actual.status, expected.status);
                    assert_eq!(actual.rounds, expected.rounds);
                }
            }
        }
    }
}

#[test]
fn literal_and_top_binary_transfers_need_at_most_two_complete_cycles() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for capacity in [1, 2, 8, 255, 256, 512] {
        for rounds in [1, 2, 4, 8] {
            let domain = domain(capacity, rounds);
            for (literal, reverse) in [
                U256::ZERO,
                U256::ONE,
                U256::from(2),
                U256::from(3),
                U256::from(7),
                U256::from(255),
                U256::from(256),
                (U256::ONE << 255) - U256::ONE,
                U256::ONE << 255,
                (U256::ONE << 255) + U256::ONE,
                U256::MAX - U256::ONE,
                U256::MAX,
            ]
            .into_iter()
            .flat_map(|literal| [false, true].map(|reverse| (literal, reverse)))
            {
                let mut args = [NumericValue::constant(literal), NumericValue::top()];
                if reverse {
                    args.reverse();
                }
                for op in (0x01..=0x0b).chain(0x10..=0x1e).filter(|op| {
                    !matches!(
                        *op,
                        opcode::ISZERO
                            | opcode::NOT
                            | opcode::CLZ
                            | opcode::ADDMOD
                            | opcode::MULMOD
                    )
                }) {
                    let masks = Cell::new(0);
                    let bounds = Cell::new(0);
                    let algebra = ProductAlgebra::new(
                        domain.spec(),
                        Count {
                            rule: BoundsToMask,
                            calls: &masks,
                        },
                        Count {
                            rule: MaskToBounds,
                            calls: &bounds,
                        },
                    );
                    cooperation::PROTOCOL_WORK.set((0, 0, 0));
                    let result = algebra.apply_numeric(op, &args);
                    let quote = domain.numeric_operation_work(args.iter());
                    assert!(cooperation::PROTOCOL_WORK.get().0 <= quote);
                    assert!(
                        result.rounds <= 2 && masks.get() <= 2 && bounds.get() <= 2,
                        "op={op:02x} literal={literal} reverse={reverse} capacity={capacity}"
                    );
                    if rounds >= 2 {
                        assert_eq!(result.status, ReductionStatus::Stable, "op={op:02x}");
                    }
                }
            }
        }
    }
    let args = [NumericValue::constant(U256::ZERO), NumericValue::top()];
    assert_eq!(
        domain(8, 2).numeric_operation_work(args.iter()),
        domain(8, 8).numeric_operation_work(args.iter())
    );
}

#[test]
fn dyadic_fact_normalization_can_require_a_third_scalar_pass() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    let input = NumericValue {
        bits: KnownBits::new(U256::from(2), U256::from(69)).unwrap(),
        interval: Interval::new_unsigned(U256::from(93), U256::from(180)).unwrap(),
        congruence: Congruence::new(U256::from(2), U256::ONE).unwrap(),
        ..NumericValue::top()
    };
    let limited = domain(8, 2).reduce_numeric(&input);
    assert_eq!(limited.status, ReductionStatus::RoundLimit);
    assert_eq!(limited.rounds, 2);
    let domain = domain(8, 3);
    let result = domain.reduce_numeric(&input);
    assert_eq!(result.status, ReductionStatus::Stable);
    assert_eq!(result.rounds, 3);
    assert_eq!(
        result.value.constants().unwrap(),
        &[93, 101, 109, 117, 125]
            .into_iter()
            .map(U256::from)
            .collect()
    );
    assert!(cooperation::PROTOCOL_WORK.get().0 <= domain.numeric_refinement_work(&input, 0));
    let args = [input, NumericValue::constant(U256::ZERO)];
    let quote = domain.numeric_operation_work(args.iter());
    let result = domain.apply_numeric(opcode::ADD, &args);
    assert_eq!(result.status, ReductionStatus::Stable);
    assert_eq!(result.rounds, 3);
    assert!(cooperation::PROTOCOL_WORK.get().0 <= quote);
}

#[test]
fn quotes_saturate_and_cancellation_precedes_all_cooperative_work() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for (capacity, rounds) in [(usize::MAX, 1), (8, usize::MAX)] {
        let domain = domain(capacity, rounds);
        assert_eq!(
            domain.numeric_operation_work([&NumericValue::top()].into_iter()),
            usize::MAX
        );
        assert_eq!(
            domain.numeric_refinement_work(&NumericValue::top(), usize::MAX),
            usize::MAX
        );
        let mut maximum = WorkBudget::new(usize::MAX);
        let args = [AbstractValue::top(), AbstractValue::constant(U256::from(7))];
        assert!(
            domain
                .apply_budgeted(opcode::ADD, &args, &mut maximum)
                .is_err()
        );
        assert_eq!(maximum.used(), 0);
        assert_eq!(cooperation::PROTOCOL_WORK.get(), (0, 0, 0));
    }
    let domain = Domain::default();
    let args = [AbstractValue::top(), AbstractValue::constant(U256::from(7))];
    let token = analysis_control::Cancellation::new();
    let mut paid = analysis_control::with_cancellation(&token, || {
        WorkBudget::new(domain.operation_work(&args))
    });
    token.cancel();
    assert!(
        domain
            .apply_budgeted(opcode::ADD, &args, &mut paid)
            .is_err()
    );
    assert_eq!(paid.used(), 0);
    assert!(paid.exhausted());
    assert_eq!(cooperation::PROTOCOL_WORK.get(), (0, 0, 0));
}

#[test]
fn borrowed_default_operations_remove_actual_materialization_and_only_its_fees() {
    let _copies = crate::domain::algebra::OPERAND_COPIES.scope();
    for profile in [Profile::ConstantsOnly, Profile::Product] {
        let policy = Domain::from_spec(DomainSpec::new(
            profile,
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        ));
        let algebra = ValueAlgebra::new(policy);
        let domain = algebra.borrowed().without_relations();
        let args = [
            AbstractValue::constant(U256::ONE),
            AbstractValue::constant(U256::from(2)),
        ];
        let assembly = ProductAlgebra::new(policy.spec(), BoundsToMask, MaskToBounds);
        assert_eq!(
            assembly.operand_work(args.iter()),
            2,
            "both profiles retain assembly metadata"
        );
        let quote = domain.operation_work(&args);
        if profile == Profile::ConstantsOnly {
            assert_eq!(quote, 1);
        }
        let mut denied = WorkBudget::new(quote - 1);
        assert!(
            domain
                .apply_budgeted(opcode::ADD, &args, &mut denied)
                .is_err()
        );
        assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 0);
        let mut paid = WorkBudget::new(quote);
        let result = domain
            .apply_budgeted(opcode::ADD, &args, &mut paid)
            .unwrap();
        assert_eq!(result.value.singleton(), Some(U256::from(3)));
        assert_eq!(paid.used(), quote);
        assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 0);
        assert_eq!(
            result.value.provenance(),
            &crate::domain::provenance::Provenance::transfer(
                &args
                    .iter()
                    .map(|value| value.provenance().clone())
                    .collect::<Vec<_>>()
            )
        );
        assert_eq!(
            domain.observe_byte(opcode::BYTE, &args).singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 0);
    }
    let policy = Domain::default();
    let args = [
        AbstractValue::from_numeric(cycle()),
        AbstractValue::constant(U256::from(7)),
    ];
    let quote = policy.operation_work(&args);
    let metadata = args
        .iter()
        .map(|v| v.work_size() - v.numeric().work_size())
        .sum::<usize>();
    assert_eq!(
        quote,
        policy.numeric_operation_work(args.iter().map(AbstractValue::numeric)) + 3 * 256 + metadata
    );
    let mut paid = WorkBudget::new(quote);
    let result = policy
        .apply_budgeted(opcode::ADD, &args, &mut paid)
        .unwrap();
    assert_eq!(paid.used(), quote);
    for word in [3, 5, 7] {
        assert!(result.value.contains(U256::from(word + 7)));
    }
    assert_eq!(
        crate::domain::algebra::OPERAND_COPIES.get(),
        0,
        "nonfinite transfer also borrows its original operands"
    );
}

#[test]
fn opcode_quotes_pay_real_closed_jobs_and_decline_at_byte_capacity_boundaries() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for capacity in [1, 2, 8, 255, 256, 512] {
        for rounds in [1, 2, 4, 8] {
            let policy = domain(capacity, rounds);
            let algebra = ValueAlgebra::new(policy);
            for (op, args) in [
                (
                    opcode::BYTE,
                    vec![AbstractValue::constant(U256::ZERO), AbstractValue::top()],
                ),
                (
                    opcode::BYTE,
                    vec![
                        AbstractValue::constant(U256::from(31)),
                        AbstractValue::top(),
                    ],
                ),
                (
                    opcode::AND,
                    vec![
                        AbstractValue::top(),
                        AbstractValue::constant(U256::from(255)),
                    ],
                ),
                (opcode::NOT, vec![AbstractValue::top()]),
                (opcode::ISZERO, vec![AbstractValue::top()]),
                (
                    opcode::ADD,
                    vec![AbstractValue::constant(U256::from(7)), AbstractValue::top()],
                ),
                (
                    opcode::SUB,
                    vec![AbstractValue::top(), AbstractValue::constant(U256::from(7))],
                ),
            ] {
                let quote = algebra.operation_work_for(op, &args);
                assert!(quote <= algebra.operation_work(&args));
                let mut budget = WorkBudget::new(quote);
                assert!(budget.charge(quote));
                cooperation::PROTOCOL_WORK.set((0, 0, 0));
                let actual = algebra.apply_detailed(op, &args);
                assert!(cooperation::PROTOCOL_WORK.get().0 <= quote);
                if capacity < 256 || !matches!(op, opcode::BYTE | opcode::AND) {
                    assert_eq!(actual.status, ReductionStatus::Stable);
                    assert_eq!(actual.rounds, 1);
                    assert_eq!(cooperation::PROTOCOL_WORK.get().2, 4);
                }
            }
        }
    }
    let policy = ValueAlgebra::new(Domain::default());
    let args = [AbstractValue::constant(U256::ZERO), AbstractValue::top()];
    assert!(
        policy.operation_work_for(opcode::BYTE, &args)
            < policy.operation_work_for(opcode::MUL, &args)
    );
    let overflow = [
        AbstractValue::constant(U256::from(32)),
        AbstractValue::top(),
    ];
    assert_eq!(
        policy.operation_work_for(opcode::BYTE, &overflow),
        policy.operation_work(&overflow)
    );
}

#[test]
fn address_cube_eq_literal_quote_keeps_four_real_jobs_and_declines_other_shapes() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    let max = U256::MAX >> 96usize;
    for capacity in [1, 2, 8, 512] {
        for rounds in [1, 4, 8] {
            let algebra = ValueAlgebra::new(domain(capacity, rounds));
            for literal in [U256::ZERO, U256::from(3), max] {
                for reverse in [false, true] {
                    let mut args = [
                        AbstractValue::unknown_address(),
                        AbstractValue::constant(literal),
                    ];
                    if reverse {
                        args.reverse();
                    }
                    let quote = algebra.operation_work_for(opcode::EQ, &args);
                    assert!(quote <= algebra.operation_work(&args));
                    cooperation::PROTOCOL_WORK.set((0, 0, 0));
                    let result = algebra.apply_detailed(opcode::EQ, &args);
                    assert_eq!(result.status, ReductionStatus::Stable);
                    assert_eq!(result.rounds, 1);
                    assert_eq!(cooperation::PROTOCOL_WORK.get().2, 4);
                    assert!(cooperation::PROTOCOL_WORK.get().0 <= quote);
                    assert!(result.value.contains(U256::ZERO) && result.value.contains(U256::ONE));
                }
            }
        }
    }
    let algebra = ValueAlgebra::new(Domain::default());
    for cube in [
        NumericValue::unknown_byte(),
        NumericValue {
            nonzero: true,
            ..NumericValue::unknown_address()
        },
    ] {
        let args = [
            AbstractValue::from_numeric(cube),
            AbstractValue::constant(U256::from(3)),
        ];
        assert_eq!(
            algebra.operation_work_for(opcode::EQ, &args),
            algebra.operation_work(&args)
        );
    }
    let outside = [
        AbstractValue::unknown_address(),
        AbstractValue::constant(max + U256::ONE),
    ];
    assert_eq!(
        algebra.operation_work_for(opcode::EQ, &outside),
        algebra.operation_work(&outside)
    );
}

#[test]
fn borrowed_numeric_entry_preserves_owned_transfer_results_and_limits() {
    for profile in [Profile::ConstantsOnly, Profile::Product] {
        for capacity in [1, 8, 512] {
            let policy = Domain::from_spec(DomainSpec::new(
                profile,
                NonZeroUsize::new(capacity).unwrap(),
                NonZeroUsize::new(4).unwrap(),
                NonZeroUsize::new(256).unwrap(),
            ));
            for value in [
                NumericValue::constant(U256::from(7)),
                finite(2),
                NumericValue {
                    nonzero: true,
                    ..finite(3)
                },
                NumericValue {
                    finite: FiniteConstantSet::try_from_values(
                        [U256::ONE, U256::from(2)].into_iter().collect(),
                    )
                    .unwrap(),
                    ..NumericValue::constant(U256::from(2))
                },
                cycle(),
                NumericValue::top(),
            ] {
                for op in (0x01..=0x0b).chain(0x10..=0x1e) {
                    let arity = match op {
                        opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
                        opcode::ADDMOD | opcode::MULMOD => 3,
                        _ => 2,
                    };
                    let mut args = vec![value.clone()];
                    args.resize(arity, NumericValue::constant(U256::from(3)));
                    let owned = policy.apply_numeric(op, &args);
                    let borrowed = policy.apply_numeric_borrowed(op, args.iter());
                    assert_eq!(
                        borrowed.value, owned.value,
                        "op={op:02x} profile={profile:?}"
                    );
                    assert_eq!(borrowed.status, owned.status);
                    assert_eq!(borrowed.rounds, owned.rounds);
                    assert_eq!(borrowed.strengthened, owned.strengthened);
                    if matches!(op, opcode::BYTE | opcode::AND) {
                        assert_eq!(
                            policy.observe_byte_numeric(op, &args),
                            policy.observe_byte_numeric_borrowed(op, args.iter()),
                        );
                    }
                }
            }
            for (op, count) in [(opcode::NOT, 0), (opcode::ADDMOD, 4)] {
                let args = vec![NumericValue::top(); count];
                assert!(policy.apply_numeric(op, &args).value.is_top());
                assert!(
                    policy
                        .apply_numeric_borrowed(op, args.iter())
                        .value
                        .is_top()
                );
            }
        }
    }
}

#[test]
fn borrowed_finite_transfer_preserves_stricter_singletons_and_source_metadata() {
    let _copies = crate::domain::algebra::OPERAND_COPIES.scope();
    let input = NumericValue {
        finite: FiniteConstantSet::try_from_values(
            [U256::ONE, U256::from(2)].into_iter().collect(),
        )
        .unwrap(),
        ..NumericValue::constant(U256::from(2))
    };
    let domain = ValueAlgebra::new(Domain::default());
    let args = [
        AbstractValue::from_numeric(input),
        AbstractValue::constant(U256::from(3)),
    ];
    let result = domain.apply(opcode::ADD, &args);
    assert_eq!(result.singleton(), Some(U256::from(5)));
    assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 0);
    assert!(result.provenance().is_top());
}

#[test]
fn borrowed_iszero_uses_nonzero_guarantees_beyond_the_raw_finite_cover() {
    let _copies = crate::domain::algebra::OPERAND_COPIES.scope();
    let policy = Domain::default();
    let input = NumericValue {
        nonzero: true,
        ..finite(3)
    };
    assert!(input.constants().unwrap().contains(&U256::ZERO));
    assert!(!input.contains(U256::ZERO));
    assert_eq!(input.singleton(), None);
    let owned = policy.apply_numeric(opcode::ISZERO, std::slice::from_ref(&input));
    let borrowed = policy.apply_numeric_borrowed(opcode::ISZERO, std::iter::once(&input));
    assert_eq!(owned.value, borrowed.value);
    assert_eq!(owned.status, borrowed.status);
    assert_eq!(borrowed.value.singleton(), Some(U256::ZERO));
    for word in [U256::ONE, U256::from(2)] {
        assert!(input.contains(word));
        assert!(borrowed.value.contains(crate::domain::evaluate(
            opcode::ISZERO,
            word,
            U256::ZERO,
            U256::ZERO
        )));
    }
    assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 0);
}

#[derive(Debug, serde::Serialize)]
struct DeepWord {
    numeric: NumericValue,
    payload: Vec<u8>,
    #[serde(skip)]
    copies: Rc<Cell<usize>>,
}
impl Clone for DeepWord {
    fn clone(&self) -> Self {
        self.copies.set(self.copies.get() + 1);
        Self {
            numeric: self.numeric.clone(),
            payload: self.payload.clone(),
            copies: Rc::clone(&self.copies),
        }
    }
}
impl PartialEq for DeepWord {
    fn eq(&self, other: &Self) -> bool {
        self.numeric == other.numeric
    }
}
impl Eq for DeepWord {}
impl std::fmt::Display for DeepWord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.numeric.fmt(f)
    }
}
impl WordElement for DeepWord {
    fn top() -> Self {
        Self {
            numeric: NumericValue::top(),
            payload: vec![],
            copies: Rc::new(Cell::new(0)),
        }
    }
    fn constant(value: U256) -> Self {
        Self {
            numeric: NumericValue::constant(value),
            ..Self::top()
        }
    }
    fn from_observation(value: &WordObservation) -> Self {
        Self {
            numeric: NumericValue::from_observation(value),
            ..Self::top()
        }
    }
    fn observation(&self) -> WordObservation {
        self.numeric.observation()
    }
    fn singleton(&self) -> Option<U256> {
        self.numeric.singleton()
    }
    fn is_top(&self) -> bool {
        self.numeric.is_top()
    }
    fn work_size(&self) -> usize {
        64 + self.payload.len() + self.numeric.work_size()
    }
}

#[derive(Debug)]
struct DeepPolicy {
    calls: Cell<usize>,
}
impl EvmNumericAlgebra for DeepPolicy {
    type Element = DeepWord;
    fn spec(&self) -> DomainSpec {
        DomainSpec::new(
            Profile::ConstantsOnly,
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        )
    }
    fn project_numeric(&self, value: &DeepWord) -> DeepWord {
        value.clone()
    }
    fn join_numeric(&self, _: &DeepWord, _: &DeepWord) -> DeepWord {
        DeepWord::top()
    }
    fn widen_numeric(&self, left: &DeepWord, right: &DeepWord) -> DeepWord {
        self.join_numeric(left, right)
    }
    fn widen_procedure_numeric(&self, left: &DeepWord, right: &DeepWord) -> DeepWord {
        self.join_numeric(left, right)
    }
    fn reduce_numeric(&self, value: &DeepWord) -> crate::domain::NumericReduction<DeepWord> {
        crate::domain::NumericReduction::stable(value.clone())
    }
    fn apply_numeric(
        &self,
        op: u8,
        args: &[DeepWord],
    ) -> crate::domain::NumericReduction<DeepWord> {
        self.calls.set(self.calls.get() + 1);
        let value = if op == opcode::ADD && args.len() == 2 {
            args[0]
                .singleton()
                .zip(args[1].singleton())
                .map(|(a, b)| a.wrapping_add(b))
                .map_or_else(DeepWord::top, DeepWord::constant)
        } else {
            DeepWord::top()
        };
        crate::domain::NumericReduction::stable(value)
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a DeepWord>) -> usize {
        args.fold(1024usize, |work, value| {
            work.saturating_add(value.work_size())
        })
    }
    fn projection_work(&self, _: &DeepWord) -> usize {
        0
    }
    fn refinement_work(&self, value: &DeepWord, _: usize) -> usize {
        value.work_size().saturating_mul(16).saturating_add(1024)
    }
}

#[test]
fn backward_compatible_custom_materialization_remains_paid_before_deep_copies() {
    let _copies = crate::domain::algebra::OPERAND_COPIES.scope();
    let copies = Rc::new(Cell::new(0));
    let args = [4096, 2048].map(|size| {
        crate::domain::EvmValue::from_numeric(DeepWord {
            numeric: NumericValue::constant(U256::from(7)),
            payload: vec![9; size],
            copies: Rc::clone(&copies),
        })
    });
    let domain = ValueAlgebra::new(DeepPolicy {
        calls: Cell::new(0),
    });
    let view = domain.borrowed().without_relations();
    let quote = view.operation_work(&args);
    assert_eq!(
        view.operation_work_for_refs(opcode::ADD, &[&args[0], &args[1]]),
        quote
    );
    assert!(quote >= 2 * (4096 + 2048));
    assert_eq!(copies.get(), 0);
    let mut denied = WorkBudget::new(quote - 1);
    assert!(
        view.apply_budgeted(opcode::ADD, &args, &mut denied)
            .is_err()
    );
    assert_eq!(denied.used(), 0);
    assert_eq!(copies.get(), 0);
    assert_eq!(domain.numeric_algebra().calls.get(), 0);
    let mut paid = WorkBudget::new(quote);
    let result = view.apply_budgeted(opcode::ADD, &args, &mut paid).unwrap();
    assert_eq!(result.value.singleton(), Some(U256::from(14)));
    assert_eq!(paid.used(), quote);
    assert_eq!(copies.get(), 2);
    assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 2);
    assert_eq!(domain.numeric_algebra().calls.get(), 1);
}

#[test]
fn staged_scalar_refusal_keeps_paid_prefix_without_running_required_rules() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    let algebra = ValueAlgebra::new(Domain::default());
    let domain = algebra.borrowed();
    let limits = RelationLimits::default();
    let x = ExprId::fresh().unwrap();
    let mut state = RelationState::default();
    assert_eq!(
        state.assume_checked(&x, true, &limits).unwrap(),
        CheckResult::MayBeFeasible
    );
    let input = AbstractValue::top().with_expression(x.clone());
    let before = input.clone();
    let imported = NumericValue {
        nonzero: true,
        ..NumericValue::top()
    };
    let closure = domain.numeric_reduction_work(&imported);
    let mut full = WorkBudget::new(20_000_000);
    let actual = state
        .refine_numeric_budgeted_with(&x, &input, domain, &limits, &mut full)
        .unwrap();
    assert_eq!(
        actual,
        state.refine_numeric_with(&x, input.numeric(), domain, &limits)
    );
    assert_eq!(cooperation::PROTOCOL_WORK.get().2, 4);
    let import = full.used().checked_sub(closure).unwrap();
    assert!(import > 0 && full.used() < domain.refinement_work(&input, state.len()));
    for (budget_size, paid_prefix) in [(import - 1, 0), (import + closure - 1, import)] {
        cooperation::PROTOCOL_WORK.set((0, 0, 0));
        let mut denied = WorkBudget::new(budget_size);
        assert!(
            state
                .refine_numeric_budgeted_with(&x, &input, domain, &limits, &mut denied)
                .is_err()
        );
        assert_eq!(denied.used(), paid_prefix);
        assert!(denied.exhausted());
        assert_eq!(cooperation::PROTOCOL_WORK.get(), (0, 0, 0));
        assert_eq!(input, before);
    }
    let mut exact = WorkBudget::new(import + closure);
    assert_eq!(
        state
            .refine_numeric_budgeted_with(&x, &input, domain, &limits, &mut exact)
            .unwrap(),
        actual
    );
    assert_eq!(exact.used(), import + closure);
    assert!(!exact.exhausted());
    let token = analysis_control::Cancellation::new();
    let mut cancelled =
        analysis_control::with_cancellation(&token, || WorkBudget::new(import + closure));
    token.cancel();
    cooperation::PROTOCOL_WORK.set((0, 0, 0));
    assert!(
        state
            .refine_numeric_budgeted_with(&x, &input, domain, &limits, &mut cancelled)
            .is_err()
    );
    assert_eq!(cancelled.used(), 0);
    assert_eq!(cooperation::PROTOCOL_WORK.get(), (0, 0, 0));
    let saturated = ValueAlgebra::new(self::domain(usize::MAX, usize::MAX));
    let mut maximum = WorkBudget::new(usize::MAX);
    assert!(
        state
            .refine_numeric_budgeted_with(&x, &input, saturated.borrowed(), &limits, &mut maximum)
            .is_err()
    );
    assert_eq!(maximum.used(), 0);
    assert_eq!(cooperation::PROTOCOL_WORK.get(), (0, 0, 0));
}

#[test]
fn staged_fact_payload_quotes_distinguish_equal_counts_and_skip_unperformed_closure() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    let algebra = ValueAlgebra::new(domain(8, 4));
    let domain = algebra.borrowed();
    let limits = RelationLimits::default();
    let x = ExprId::fresh().unwrap();
    let input = AbstractValue::top().with_expression(x.clone());
    let mut fees = Vec::new();
    for size in [2, 128] {
        let mut state = RelationState::default();
        state.observe(&x, &finite(size), &limits).unwrap();
        assert_eq!(state.len(), 1);
        let mut paid = WorkBudget::new(20_000_000);
        let result = state
            .refine_numeric_budgeted_with(&x, &input, domain, &limits, &mut paid)
            .unwrap();
        assert_eq!(
            result,
            state.refine_numeric_with(&x, input.numeric(), domain, &limits)
        );
        assert!(
            matches!(result,EvmScalarQuery::Refined{ref numeric,..} if (0..size).all(|word|numeric.contains(U256::from(word))))
        );
        fees.push(paid.used());
    }
    assert!(
        fees[1] > fees[0],
        "one Members atom must pay its entire actual payload"
    );
    let y = ExprId::fresh().unwrap();
    let mut equal = RelationState::default();
    assert_eq!(equal.add_equal(&x, &y, &limits), CheckResult::MayBeFeasible);
    cooperation::PROTOCOL_WORK.set((0, 0, 0));
    let mut paid = WorkBudget::new(20_000_000);
    assert_eq!(
        equal
            .refine_numeric_budgeted_with(&x, &input, domain, &limits, &mut paid)
            .unwrap(),
        EvmScalarQuery::Unchanged
    );
    assert!(paid.used() < domain.refinement_work(&input, equal.len()));
    assert_eq!(cooperation::PROTOCOL_WORK.get(), (0, 0, 0));
}

#[test]
fn custom_scalar_backend_keeps_one_whole_quote_and_never_clones_while_quoting() {
    let copies = Rc::new(Cell::new(0));
    let input = crate::domain::EvmValue::from_numeric(DeepWord {
        numeric: NumericValue::constant(U256::from(7)),
        payload: vec![9; 4096],
        copies: Rc::clone(&copies),
    });
    let x = ExprId::fresh().unwrap();
    let limits = RelationLimits::default();
    let mut state = RelationState::default();
    state.assume_checked(&x, true, &limits).unwrap();
    let policy = ValueAlgebra::new(DeepPolicy {
        calls: Cell::new(0),
    });
    let domain = policy.borrowed();
    let whole = domain.refinement_work(&input, state.len());
    assert_eq!(copies.get(), 0);
    let mut denied = WorkBudget::new(whole - 1);
    assert!(
        state
            .refine_numeric_budgeted_with(&x, &input, domain, &limits, &mut denied)
            .is_err()
    );
    assert_eq!(copies.get(), 0);
    assert_eq!(denied.used(), 0);
    let mut paid = WorkBudget::new(whole);
    let result = state
        .refine_numeric_budgeted_with(&x, &input, domain, &limits, &mut paid)
        .unwrap();
    assert!(
        matches!(result,EvmScalarQuery::Refined{ref numeric,..} if numeric.singleton()==Some(U256::from(7)))
    );
    assert_eq!(
        paid.used(),
        whole,
        "custom whole quote must not acquire a second closure charge"
    );
    assert!(copies.get() > 0);
}
