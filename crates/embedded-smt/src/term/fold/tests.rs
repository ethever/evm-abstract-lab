use super::super::{Bool, Bv, Context, FunctionArg, FunctionSort, Op, Sort, Term};
use crate::{DEFAULT_RLIMIT, Outcome, Provider, check};
use num_bigint::BigUint;
use num_traits::One;

const PROVIDERS: [Provider; 3] = [Provider::Z3, Provider::Bitwuzla, Provider::Cvc5];

// Build the oracle directly: none of these operation nodes enters a constructor
// that can eagerly fold. The same explicit native allowance applies both ways.
fn raw_bv(op: Op, width: u32, args: &[&Bv]) -> Bv {
    Bv(Term::new(
        op,
        Sort::Bv(width),
        args.iter().map(|v| v.0.clone()).collect(),
    ))
}

fn raw_bool(op: Op, args: Vec<Term>) -> Bool {
    Bool(Term::new(op, Sort::Bool, args))
}

fn equivalent(raw: &Bv, folded: &Bv) {
    for provider in PROVIDERS {
        assert_eq!(
            check(&[raw.eq(folded).not()], None, provider, DEFAULT_RLIMIT),
            Outcome::Unsat,
            "{provider}: raw={raw:?}, folded={folded:?}"
        );
    }
}

fn bool_equivalent(raw: &Bool, folded: &Bool) {
    let different = raw_bool(Op::Eq, vec![raw.0.clone(), folded.0.clone()]).not();
    for provider in PROVIDERS {
        assert_eq!(
            check(
                std::slice::from_ref(&different),
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat,
            "{provider}: raw={raw:?}, folded={folded:?}"
        );
    }
}

#[test]
fn modular_literals_match_unfurled_native_oracle_at_width_edges() {
    for width in [1, 8, 256, 512] {
        let max = (BigUint::one() << width) - BigUint::one();
        let max = Bv::from_str(width, &max.to_str_radix(10)).unwrap();
        let one = Bv::from_u64(1, width);
        let zero = Bv::from_u64(0, width);
        for (op, a, b, folded) in [
            (Op::Add, &max, &one, max.bvadd(&one)),
            (Op::Sub, &zero, &one, zero.bvsub(&one)),
            (Op::Mul, &max, &max, max.bvmul(&max)),
            (Op::Sub, &max, &max, max.bvsub(&max)),
        ] {
            assert!(folded.literal_hex().is_some());
            equivalent(&raw_bv(op, width, &[a, b]), &folded);
        }
    }
}

#[test]
fn literal_shifts_match_native_width_and_sign_edges() {
    for width in [1, 8, 256, 512] {
        let high = BigUint::one() << (width - 1);
        let high = Bv::from_str(width, &high.to_str_radix(10)).unwrap();
        let one = Bv::from_u64(1, width);
        let mut shifts = vec![0, width, width + 1];
        if width == 1 {
            // A one-bit shift operand cannot encode 2; 1 is already >= width.
            shifts = vec![0, 1];
        } else {
            shifts.push(width - 1);
        }
        for value in [&high, &one] {
            for shift in &shifts {
                let shift = Bv::from_u64(u64::from(*shift), width);
                for (op, folded) in [
                    (Op::Shl, value.bvshl(&shift)),
                    (Op::Lshr, value.bvlshr(&shift)),
                    (Op::Ashr, value.bvashr(&shift)),
                ] {
                    assert!(folded.literal_hex().is_some());
                    equivalent(&raw_bv(op, width, &[value, &shift]), &folded);
                }
            }
            // A bounded-width operand can denote an enormous numeric shift.
            // Saturation must happen before any BigUint shift allocation.
            let huge = (BigUint::one() << width) - BigUint::one();
            let huge = Bv::from_str(width, &huge.to_str_radix(10)).unwrap();
            for (op, folded) in [
                (Op::Shl, value.bvshl(&huge)),
                (Op::Lshr, value.bvlshr(&huge)),
                (Op::Ashr, value.bvashr(&huge)),
            ] {
                assert!(folded.literal_hex().is_some());
                equivalent(&raw_bv(op, width, &[value, &huge]), &folded);
            }
        }
    }
}

#[test]
fn unsigned_comparisons_and_boolean_literals_match_raw_oracle() {
    for width in [1, 8, 256, 512] {
        let max = (BigUint::one() << width) - BigUint::one();
        let values = [
            Bv::from_u64(0, width),
            Bv::from_u64(1, width),
            Bv::from_str(width, &max.to_str_radix(10)).unwrap(),
        ];
        for a in &values {
            for b in &values {
                for (op, folded) in [
                    (Op::Eq, a.eq(b)),
                    (Op::Ult, a.bvult(b)),
                    (Op::Ule, a.bvule(b)),
                    (Op::Ugt, a.bvugt(b)),
                    (Op::Uge, a.bvuge(b)),
                ] {
                    assert!(folded.as_bool().is_some());
                    bool_equivalent(&raw_bool(op, vec![a.0.clone(), b.0.clone()]), &folded);
                }
            }
        }
    }
    for a in [false, true] {
        let a = Bool::from_bool(a);
        bool_equivalent(&raw_bool(Op::Not, vec![a.0.clone()]), &a.not());
        for b in [false, true] {
            let b = Bool::from_bool(b);
            for (op, folded) in [
                (Op::And, Bool::and(&[a.clone(), b.clone()])),
                (Op::Or, Bool::or(&[a.clone(), b.clone()])),
            ] {
                bool_equivalent(&raw_bool(op, vec![a.0.clone(), b.0.clone()]), &folded);
            }
        }
    }
    assert_eq!(Bool::and(&[]).as_bool(), Some(true));
    assert_eq!(Bool::or(&[]).as_bool(), Some(false));
}

#[test]
fn literal_ite_extensions_and_extracts_match_raw_oracle() {
    for width in [1, 8, 256, 512] {
        let yes = Bv::from_u64(1, width);
        let no = Bv::from_u64(0, width);
        for guard in [false, true] {
            let guard = Bool::from_bool(guard);
            let raw = Bv(Term::new(
                Op::Ite,
                Sort::Bv(width),
                vec![guard.0.clone(), yes.0.clone(), no.0.clone()],
            ));
            equivalent(&raw, &guard.ite(&yes, &no));
        }
        let extended = yes.zero_ext(512 - width);
        assert_eq!(extended.literal_hex(), Some("1"));
        equivalent(
            &raw_bv(Op::ZeroExtend(512 - width), 512, &[&yes]),
            &extended,
        );
        for (high, low) in [(width - 1, 0), (width - 1, width - 1), (0, 0)] {
            let extracted = yes.extract(high, low);
            assert!(extracted.literal_hex().is_some());
            equivalent(
                &raw_bv(Op::Extract(high, low), high - low + 1, &[&yes]),
                &extracted,
            );
        }
    }
}

#[test]
fn cap_and_unsupported_operations_retain_the_original_node() {
    for width in [513, u32::MAX] {
        let zero = Bv::from_u64(0, width);
        let one = Bv::from_u64(1, width);
        assert!(matches!(one.bvadd(&one).0.0.op, Op::Add));
        assert!(matches!(one.bvsub(&one).0.0.op, Op::Sub));
        assert!(matches!(one.bvmul(&one).0.0.op, Op::Mul));
        assert!(matches!(one.eq(&one).0.0.op, Op::Eq));
        assert!(matches!(one.bvuge(&one).0.0.op, Op::Uge));
        assert!(matches!(one.bvshl(&zero).0.0.op, Op::Shl));
        assert!(matches!(one.bvashr(&one).0.0.op, Op::Ashr));
        assert!(matches!(one.extract(0, 0).0.0.op, Op::Extract(_, _)));
        assert!(matches!(
            Bool::from_bool(false).ite(&one, &zero).0.0.op,
            Op::Ite
        ));
    }
    let one = Bv::from_u64(1, 512);
    assert!(matches!(one.zero_ext(1).0.0.op, Op::ZeroExtend(1)));
    assert!(matches!(one.bvand(&one).0.0.op, Op::BvAnd));
}

#[test]
#[should_panic(expected = "conditional branches must have equal widths")]
fn false_ite_checks_branch_width_before_folding() {
    Bool::from_bool(false).ite(&Bv::from_u64(1, 8), &Bv::from_u64(0, 9));
}

#[test]
#[should_panic(expected = "bit-vector widths must match")]
fn zero_shift_checks_operand_width_before_identity() {
    Bv::from_u64(1, 8).bvshl(Bv::from_u64(0, 9));
}

#[test]
#[should_panic(expected = "bit-vector width overflow")]
fn extension_checks_overflow_before_literal_folding() {
    Bv::from_u64(0, u32::MAX).zero_ext(1);
}

#[test]
#[should_panic(expected = "valid bit extraction range")]
fn extract_checks_range_before_literal_folding() {
    Bv::from_u64(0, 8).extract(8, 0);
}

#[test]
#[should_panic(expected = "function argument width must be positive")]
fn invalid_function_in_unselected_branch_still_panics() {
    let no = Bv::from_u64(0, 8);
    let function = Context::default().fresh_function("bad", &[FunctionSort::Bv(0)], 8);
    Bool::from_bool(false).ite(&function.apply(&[]), &no);
}

#[test]
fn masked_symbols_and_invalid_function_names_still_reach_backend_errors() {
    let context = Context::default();
    let zero = Bv::from_u64(0, 8);
    let bad_symbol = context.fresh_bv("bad\0symbol", 8);
    let bad_function = context.fresh_function("bad\0function", &[], 8).apply(&[]);
    for bad in [bad_symbol, bad_function] {
        let bad_bool = bad.eq(&zero);
        let guarded = [
            Bool::and(&[Bool::from_bool(false), bad_bool.clone()]),
            Bool::or(&[Bool::from_bool(true), bad_bool.clone()]),
            Bool::and(&[bad_bool.clone(), Bool::from_bool(false)]),
            Bool::or(&[bad_bool, Bool::from_bool(true)]),
        ];
        for assertion in guarded {
            assert_eq!(assertion.as_bool(), None);
            for provider in PROVIDERS {
                assert!(
                    matches!(
                        check(
                            std::slice::from_ref(&assertion),
                            None,
                            provider,
                            DEFAULT_RLIMIT
                        ),
                        Outcome::Error(_)
                    ),
                    "{provider}"
                );
            }
        }
        for projection in [
            Bool::from_bool(false).ite(&bad, &zero),
            Bool::from_bool(true).ite(&zero, &bad),
            bad.bvshl(&zero),
            bad.bvlshr(&zero),
            bad.bvashr(&zero),
        ] {
            for provider in PROVIDERS {
                assert!(
                    matches!(
                        check(&[], Some(&projection), provider, DEFAULT_RLIMIT),
                        Outcome::Error(_)
                    ),
                    "{provider}"
                );
            }
        }
    }
    let good = context.fresh_array("array", 8, 8);
    let bad = context.fresh_array("bad\0array", 8, 8);
    let selected = Bool::from_bool(false).ite_array(&bad, &good).select(&zero);
    for provider in PROVIDERS {
        assert!(
            matches!(
                check(&[], Some(&selected), provider, DEFAULT_RLIMIT),
                Outcome::Error(_)
            ),
            "{provider}"
        );
    }
}

#[test]
fn symbolic_zero_shifts_preserve_aliases_models_functions_and_arrays() {
    let context = Context::default();
    let x = context.fresh_bv("x", 8);
    let array = context.fresh_array("array", 8, 8);
    let function = context.fresh_function("f", &[FunctionSort::Bv(8)], 8);
    let zero = Bv::from_u64(0, 8);
    for value in [
        &x,
        &array.select(&x),
        &function.apply(&[FunctionArg::Bv(&x)]),
    ] {
        for shifted in [value.bvshl(&zero), value.bvlshr(&zero), value.bvashr(&zero)] {
            assert_eq!(shifted.0.id(), value.0.id());
            equivalent(value, &shifted);
        }
    }
    let shifted = x.bvshl(&zero);
    let stored = array.store(&shifted, &Bv::from_u64(7, 8));
    let fx = function.apply(&[FunctionArg::Bv(&x)]);
    let alias = function.clone().apply(&[FunctionArg::Bv(&shifted)]);
    let other = context
        .fresh_function("f", &[FunctionSort::Bv(8)], 8)
        .apply(&[FunctionArg::Bv(&shifted)]);
    for provider in PROVIDERS {
        assert_eq!(
            check(&[fx.eq(&alias).not()], None, provider, DEFAULT_RLIMIT),
            Outcome::Unsat
        );
        assert_eq!(
            check(
                &[stored.select(&x).eq(Bv::from_u64(7, 8)).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat
        );
        assert_eq!(
            check(&[fx.eq(&other).not()], None, provider, DEFAULT_RLIMIT),
            Outcome::Sat(None)
        );
        assert!(matches!(
            check(
                &[x.eq(Bv::from_u64(42, 8))],
                Some(&shifted),
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Sat(Some(_))
        ));
        assert!(matches!(x.eq(&x).0.0.op, Op::Eq));
    }
}
