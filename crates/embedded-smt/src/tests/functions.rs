use super::{DEFAULT_RLIMIT, PROVIDERS, number};
use crate::{
    Array, Bv, Cancellation, Context, FunctionArg, FunctionSort, Outcome, Unknown, check,
    with_cancellation,
};

#[test]
fn equal_symbolic_parameters_imply_equal_results_including_array_contents() {
    let context = Context::default();
    let function = context.fresh_function(
        "hash",
        &[
            FunctionSort::Array {
                index_width: 256,
                element_width: 8,
            },
            FunctionSort::Bv(256),
        ],
        256,
    );
    let a = context.fresh_array("bytes_a", 256, 8);
    let b = context.fresh_array("bytes_b", 256, 8);
    let length_a = context.fresh_bv("length_a", 256);
    let length_b = context.fresh_bv("length_b", 256);
    let hash_a = function.apply(&[FunctionArg::Array(&a), FunctionArg::Bv(&length_a)]);
    let hash_b = function
        .clone()
        .apply(&[FunctionArg::Array(&b), FunctionArg::Bv(&length_b)]);
    let storage = context
        .fresh_array("storage", 256, 256)
        .store(&hash_a, &Bv::from_u64(7, 256));
    for provider in PROVIDERS {
        let mut guards = vec![a.eq(&b), length_a.eq(&length_b)];
        guards.push(hash_a.eq(&hash_b).not());
        assert_eq!(
            check(&guards, None, provider, DEFAULT_RLIMIT),
            Outcome::Unsat,
            "{provider}"
        );
        guards.pop();
        guards.push(storage.select(&hash_b).eq(Bv::from_u64(7, 256)).not());
        assert_eq!(
            check(&guards, None, provider, DEFAULT_RLIMIT),
            Outcome::Unsat,
            "{provider}"
        );
        // The projection may itself introduce a previously unseen application.
        let equal_to_known = function.apply(&[FunctionArg::Array(&b), FunctionArg::Bv(&length_b)]);
        assert_eq!(
            number(check(
                &[
                    a.eq(&b),
                    length_a.eq(&length_b),
                    hash_a.eq(Bv::from_u64(42, 256))
                ],
                Some(&equal_to_known),
                provider,
                DEFAULT_RLIMIT
            )),
            42,
            "{provider}"
        );
    }
}

#[test]
fn extensionally_equal_arrays_have_equal_function_results_without_explicit_array_guard() {
    let context = Context::default();
    let function = context.fresh_function(
        "digest",
        &[FunctionSort::Array {
            index_width: 256,
            element_width: 8,
        }],
        256,
    );
    let initial = Array::constant(256, &Bv::from_u64(0, 8));
    let p = context.fresh_bv("p", 256);
    let q = context.fresh_bv("q", 256);
    let seven = Bv::from_u64(7, 8);
    let nine = Bv::from_u64(9, 8);
    let a = initial.store(&p, &seven).store(&q, &nine);
    let b = initial.store(&q, &nine).store(&p, &seven);
    let hash_a = function.apply(&[FunctionArg::Array(&a)]);
    let hash_b = function.apply(&[FunctionArg::Array(&b)]);
    for provider in PROVIDERS {
        assert_eq!(
            check(
                &[p.eq(&q).not(), hash_a.eq(&hash_b).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat,
            "{provider}"
        );
    }
}

#[test]
fn functions_compare_symbolic_default_arrays_extensionally() {
    let context = Context::default();
    let function = context.fresh_function(
        "digest",
        &[FunctionSort::Array {
            index_width: 256,
            element_width: 13,
        }],
        256,
    );
    let x = context.fresh_bv("x", 13);
    let y = context.fresh_bv("y", 13);
    let a = Array::constant(256, &x);
    let b = Array::constant(256, &y);
    let ha = function.apply(&[FunctionArg::Array(&a)]);
    let hb = function.apply(&[FunctionArg::Array(&b)]);
    for provider in PROVIDERS {
        assert_eq!(
            check(
                &[x.eq(&y), ha.eq(&hb).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat,
            "{provider}"
        );
        assert_eq!(
            check(
                &[x.eq(&y).not(), ha.eq(&hb)],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Sat(None),
            "{provider}"
        );
        assert_eq!(
            check(
                &[x.eq(&y).not(), ha.eq(&hb).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Sat(None),
            "{provider}"
        );
    }
}

#[test]
fn array_and_function_symbol_names_with_nuls_return_errors() {
    let context = Context::default();
    let array = context.fresh_array("bad\0array", 256, 13);
    let function = context.fresh_function("bad\0function", &[], 13);
    let result = function.apply(&[]);
    let p = Bv::from_u64(0, 256);
    let symbolic_default = Array::constant(256, &context.fresh_bv("default", 13));
    for provider in PROVIDERS {
        assert!(
            matches!(check(&[], Some(&array.select(&p)), provider, DEFAULT_RLIMIT), Outcome::Error(error) if error.provider == provider),
            "{provider}"
        );
        assert!(
            matches!(check(&[array.eq(&symbolic_default)], None, provider, DEFAULT_RLIMIT), Outcome::Error(error) if error.provider == provider),
            "{provider}"
        );
        assert!(
            matches!(check(&[], Some(&result), provider, DEFAULT_RLIMIT), Outcome::Error(error) if error.provider == provider),
            "{provider}"
        );
    }
}

#[test]
fn unknown_functions_allow_collisions_and_distinct_declarations_are_independent() {
    let context = Context::default();
    let function = context.fresh_function("same", &[FunctionSort::Bv(8)], 13);
    let other = context.fresh_function("same", &[FunctionSort::Bv(8)], 13);
    let independent = Context::default().fresh_function("same", &[FunctionSort::Bv(8)], 13);
    let x = context.fresh_bv("x", 8);
    let y = context.fresh_bv("y", 8);
    let fx = function.apply(&[FunctionArg::Bv(&x)]);
    let fy = function.apply(&[FunctionArg::Bv(&y)]);
    for provider in PROVIDERS {
        for equality in [fx.eq(&fy), fx.eq(&fy).not()] {
            assert_eq!(
                check(&[x.eq(&y).not(), equality], None, provider, DEFAULT_RLIMIT),
                Outcome::Sat(None),
                "{provider}"
            );
        }
        for other in [&other, &independent] {
            let other_result = other.apply(&[FunctionArg::Bv(&x)]);
            assert_eq!(
                check(
                    &[fx.eq(&other_result).not()],
                    None,
                    provider,
                    DEFAULT_RLIMIT
                ),
                Outcome::Sat(None),
                "{provider}"
            );
        }
        let same_result = function.clone().apply(&[FunctionArg::Bv(&x)]);
        assert_eq!(
            check(&[fx.eq(&same_result).not()], None, provider, DEFAULT_RLIMIT),
            Outcome::Unsat,
            "{provider}"
        );
        // Distinct lengths remain separate parameters even for equal content.
        let with_length = context.fresh_function(
            "length_sensitive",
            &[FunctionSort::Bv(8), FunctionSort::Bv(8)],
            8,
        );
        let first = with_length.apply(&[FunctionArg::Bv(&x), FunctionArg::Bv(&Bv::from_u64(1, 8))]);
        let second =
            with_length.apply(&[FunctionArg::Bv(&x), FunctionArg::Bv(&Bv::from_u64(2, 8))]);
        assert_eq!(
            check(&[first.eq(&second).not()], None, provider, DEFAULT_RLIMIT),
            Outcome::Sat(None),
            "{provider}"
        );
    }
}

#[test]
fn empty_function_domain_is_one_consistent_unknown_constant() {
    let function = Context::default().fresh_function("constant", &[], 257);
    let first = function.apply(&[]);
    let second = function.clone().apply(&[]);
    for provider in PROVIDERS {
        assert_eq!(
            check(&[first.eq(&second).not()], None, provider, DEFAULT_RLIMIT),
            Outcome::Unsat,
            "{provider}"
        );
        assert_eq!(
            number(check(
                &[first.eq(Bv::from_u64(99, 257))],
                Some(&second),
                provider,
                DEFAULT_RLIMIT
            )),
            99,
            "{provider}"
        );
    }
}

#[test]
fn congruence_budget_and_cancellation_return_unknown_and_do_not_poison_next_query() {
    let context = Context::default();
    let function = context.fresh_function("table", &[FunctionSort::Bv(8)], 8);
    let assertions: Vec<_> = (0..8)
        .map(|index| {
            let input = Bv::from_u64(index, 8);
            function
                .apply(&[FunctionArg::Bv(&input)])
                .eq(Bv::from_u64(index, 8))
        })
        .collect();
    let cancelled = Cancellation::new();
    cancelled.cancel();
    for provider in PROVIDERS {
        assert_eq!(
            check(&assertions, None, provider, 8),
            Outcome::Unknown(Unknown::ResourceLimit),
            "{provider}"
        );
        assert_eq!(
            with_cancellation(&cancelled, || check(
                &assertions,
                None,
                provider,
                DEFAULT_RLIMIT
            )),
            Outcome::Unknown(Unknown::Cancelled),
            "{provider}"
        );
        assert_eq!(
            check(&assertions, None, provider, DEFAULT_RLIMIT),
            Outcome::Sat(None),
            "{provider}"
        );
    }
}

#[test]
fn array_queries_keep_native_resource_unknown_separate_from_unsat() {
    let context = Context::default();
    let array = context.fresh_array("a", 256, 8);
    let index = context.fresh_bv("index", 256);
    let value = array.select(&index);
    let assertion = value.bvmul(&value).eq(Bv::from_u64(4, 8));
    for provider in PROVIDERS {
        assert_eq!(
            check(std::slice::from_ref(&assertion), None, provider, 1),
            Outcome::Unknown(Unknown::ResourceLimit),
            "{provider}"
        );
        assert!(
            matches!(
                check(
                    std::slice::from_ref(&assertion),
                    Some(&value),
                    provider,
                    DEFAULT_RLIMIT
                ),
                Outcome::Sat(Some(_))
            ),
            "{provider}"
        );
    }
}

#[test]
fn typed_function_construction_checks_all_widths_arity_and_argument_sorts() {
    assert!(
        std::panic::catch_unwind(|| Context::default().fresh_function(
            "bad",
            &[FunctionSort::Bv(0)],
            8
        ))
        .is_err()
    );
    assert!(std::panic::catch_unwind(|| Context::default().fresh_function("bad", &[], 0)).is_err());
    for (index_width, element_width) in [(0, 8), (8, 0)] {
        assert!(
            std::panic::catch_unwind(|| Context::default().fresh_function(
                "bad",
                &[FunctionSort::Array {
                    index_width,
                    element_width
                }],
                8
            ))
            .is_err()
        );
    }
    let function = Context::default().fresh_function(
        "f",
        &[
            FunctionSort::Bv(8),
            FunctionSort::Array {
                index_width: 8,
                element_width: 13,
            },
        ],
        13,
    );
    let bv = Bv::from_u64(0, 8);
    let array = Array::constant(8, &Bv::from_u64(0, 13));
    assert!(std::panic::catch_unwind(|| function.apply(&[])).is_err());
    assert!(
        std::panic::catch_unwind(|| function.apply(&[FunctionArg::Bv(&bv), FunctionArg::Bv(&bv)]))
            .is_err()
    );
    assert!(
        std::panic::catch_unwind(|| function.apply(&[
            FunctionArg::Bv(&Bv::from_u64(0, 9)),
            FunctionArg::Array(&array)
        ]))
        .is_err()
    );
    let wrong_array = Array::constant(9, &Bv::from_u64(0, 13));
    assert!(
        std::panic::catch_unwind(
            || function.apply(&[FunctionArg::Bv(&bv), FunctionArg::Array(&wrong_array)])
        )
        .is_err()
    );
}
