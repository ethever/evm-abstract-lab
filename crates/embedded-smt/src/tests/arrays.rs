use super::{DEFAULT_RLIMIT, PROVIDERS, number};
use crate::{Array, Bool, Bv, Context, Error, Outcome, Provider, Unknown, check};

#[test]
fn symbolic_aliases_preserve_write_order_and_historical_reads() {
    let context = Context::default();
    let k = context.fresh_bv("k", 256);
    let q = context.fresh_bv("q", 256);
    let initial = context.fresh_array("storage", 256, 256);
    let seven = Bv::from_u64(7, 256);
    let nine = Bv::from_u64(9, 256);
    let first = initial.store(&k, &seven);
    let historical = first.select(&k);
    let second = first.store(&q, &nine);
    let latest = second.select(&k);
    let expected = k.eq(&q).ite(&nine, &seven);
    for provider in PROVIDERS {
        assert_eq!(
            check(
                &[latest.eq(&expected).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat,
            "{provider}"
        );
        assert_eq!(
            check(
                &[historical.eq(&seven).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat,
            "{provider}"
        );
        assert_eq!(
            number(check(&[k.eq(&q)], Some(&latest), provider, DEFAULT_RLIMIT)),
            9,
            "{provider}"
        );
        assert_eq!(
            number(check(
                &[k.eq(&q).not()],
                Some(&latest),
                provider,
                DEFAULT_RLIMIT
            )),
            7,
            "{provider}"
        );
        for (alias, value) in [(true, &nine), (false, &seven)] {
            let equality = k.eq(&q);
            let guard = if alias { equality } else { equality.not() };
            assert_eq!(
                check(&[guard, latest.eq(value)], None, provider, DEFAULT_RLIMIT),
                Outcome::Sat(None),
                "{provider}"
            );
        }
        let overwritten = second.store(&k, &Bv::from_u64(11, 256)).select(&k);
        assert_eq!(
            number(check(&[], Some(&overwritten), provider, DEFAULT_RLIMIT)),
            11,
            "{provider}"
        );
    }
}

#[test]
fn mixed_concrete_and_symbolic_stores_preserve_other_addresses() {
    let context = Context::default();
    let p = context.fresh_bv("p", 256);
    let q = context.fresh_bv("q", 256);
    let concrete = Bv::from_u64(128, 256);
    let zero = Bv::from_u64(0, 8);
    let seven = Bv::from_u64(7, 8);
    let nine = Bv::from_u64(9, 8);
    let array = Array::constant(256, &zero)
        .store(&concrete, &seven)
        .store(&p, &nine);
    let original_at_concrete = p.eq(&concrete).ite(&nine, &seven);
    let at_q = p.eq(&q).ite(&nine, &q.eq(&concrete).ite(&seven, &zero));
    for provider in PROVIDERS {
        let violation = Bool::or(&[
            array
                .select(&concrete)
                .eq(original_at_concrete.clone())
                .not(),
            array.select(&q).eq(&at_q).not(),
        ]);
        assert_eq!(
            check(&[violation], None, provider, DEFAULT_RLIMIT),
            Outcome::Unsat,
            "{provider}"
        );
        assert_eq!(
            number(check(
                &[q.eq(&p).not(), q.eq(&concrete).not()],
                Some(&array.select(&q)),
                provider,
                DEFAULT_RLIMIT
            )),
            0,
            "{provider}"
        );
    }
}

#[test]
fn fresh_arrays_do_not_invent_zero_or_merge_independent_origins() {
    let first = Context::default().fresh_array("same", 256, 8);
    let second = Context::default().fresh_array("same", 256, 8);
    let index = Bv::from_u64(5, 256);
    for provider in PROVIDERS {
        assert_eq!(
            check(
                &[
                    first.select(&index).eq(Bv::from_u64(13, 8)),
                    second.select(&index).eq(Bv::from_u64(42, 8))
                ],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Sat(None),
            "{provider}"
        );
        assert_eq!(
            check(
                &[first.eq(first.clone()).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat,
            "{provider}"
        );
        // Distinct origins may nevertheless have identical contents.
        assert_eq!(
            check(&[first.eq(&second)], None, provider, DEFAULT_RLIMIT),
            Outcome::Sat(None),
            "{provider}"
        );
    }
}

#[test]
fn array_ite_keeps_addresses_paired_with_their_branch_contents() {
    let context = Context::default();
    let flag = context.fresh_bv("flag", 1);
    let condition = flag.eq(Bv::from_u64(1, 1));
    let p = Bv::from_u64(128, 256);
    let q = Bv::from_u64(192, 256);
    let initial = Array::constant(256, &Bv::from_u64(0, 256));
    let value = Bv::from_u64(7, 256);
    let yes = initial.store(&p, &value);
    let no = initial.store(&q, &value);
    let joined = condition.ite_array(&yes, &no);
    let address = condition.ite(&p, &q);
    for provider in PROVIDERS {
        assert_eq!(
            check(
                &[joined.select(&address).eq(&value).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat,
            "{provider}"
        );
        for guard in [condition.clone(), condition.not()] {
            assert_eq!(
                number(check(
                    &[guard],
                    Some(&joined.select(&address)),
                    provider,
                    DEFAULT_RLIMIT
                )),
                7,
                "{provider}"
            );
        }
        assert_eq!(
            check(
                &[condition.ite_array(&yes, &yes).eq(&yes).not()],
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
fn array_equality_is_extensional_and_store_order_commutes_for_distinct_indices() {
    let context = Context::default();
    let a = context.fresh_array("a", 8, 13);
    let b = context.fresh_array("b", 8, 13);
    let p = context.fresh_bv("p", 8);
    let q = context.fresh_bv("q", 8);
    let first = a
        .store(&p, &Bv::from_u64(7, 13))
        .store(&q, &Bv::from_u64(9, 13));
    let second = a
        .store(&q, &Bv::from_u64(9, 13))
        .store(&p, &Bv::from_u64(7, 13));
    for provider in PROVIDERS {
        assert_eq!(
            check(
                &[a.eq(&b), a.select(&p).eq(b.select(&p)).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat,
            "{provider}"
        );
        assert_eq!(
            check(
                &[p.eq(&q).not(), first.eq(&second).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat,
            "{provider}"
        );
        assert_eq!(
            check(
                &[p.eq(&q), first.eq(&second)],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat,
            "{provider}"
        );
        let rewritten = a.store(&p, &a.select(&p));
        assert_eq!(
            check(&[rewritten.eq(&a).not()], None, provider, DEFAULT_RLIMIT),
            Outcome::Unsat,
            "{provider}"
        );
    }
}

#[test]
fn symbolic_constant_defaults_and_non_hex_widths_project_bv_models() {
    let context = Context::default();
    let default = context.fresh_bv("default", 13);
    let p = context.fresh_bv("p", 257);
    let array = Array::constant(257, &default);
    assert_eq!(array.index_width(), 257);
    assert_eq!(array.element_width(), 13);
    assert_eq!(array.select(&p).width(), 13);
    for provider in PROVIDERS {
        assert_eq!(
            number(check(
                &[default.eq(Bv::from_u64(8191, 13))],
                Some(&array.select(&p)),
                provider,
                DEFAULT_RLIMIT
            )),
            8191,
            "{provider}"
        );
        let Outcome::Sat(Some(model)) = check(
            &[default.eq(Bv::from_u64(1, 13))],
            Some(&array.select(&p)),
            provider,
            DEFAULT_RLIMIT,
        ) else {
            panic!("{provider}: expected a BV model projection");
        };
        if let Some(bits) = model.strip_prefix("#b") {
            assert_eq!(bits.len(), 13, "{provider}");
        }
    }
}

#[test]
fn symbolic_defaults_mix_with_fresh_arrays_stores_equality_and_conditionals() {
    for element_width in [1, 13, 256] {
        let context = Context::default();
        let default = context.fresh_bv("default", element_width);
        let p = context.fresh_bv("p", 256);
        let q = context.fresh_bv("q", 256);
        let a = Array::constant(256, &default);
        let b = context.fresh_array("b", 256, element_width);
        let condition = p.eq(&q);
        let zero = Bv::from_u64(0, element_width);
        let one = Bv::from_u64(1, element_width);
        let updated = a.store(&p, &one);
        let joined = condition.ite_array(&updated, &b);
        let expected = condition.ite(&p.eq(&q).ite(&one, &default), &b.select(&q));
        for provider in PROVIDERS {
            // Exact symbolic-default decomposition takes extra native fuel;
            // still require a proof for the original store/ITE relation.
            let select_allowance = match (provider, element_width) {
                (Provider::Cvc5, 13) => 1_000_000,
                (Provider::Cvc5, 256) => 10_000_000,
                _ => DEFAULT_RLIMIT,
            };
            assert_eq!(
                check(
                    &[joined.select(&q).eq(&expected).not()],
                    None,
                    provider,
                    select_allowance
                ),
                Outcome::Unsat,
                "{provider}: width={element_width}"
            );
            let read_allowance = if provider == Provider::Cvc5 && element_width == 13 {
                10_000_000
            } else {
                DEFAULT_RLIMIT
            };
            let equality_outcome = if provider == Provider::Cvc5 && element_width == 256 {
                Outcome::Unknown(Unknown::ResourceLimit)
            } else {
                Outcome::Unsat
            };
            assert_eq!(
                check(
                    &[a.eq(&b), b.select(&q).eq(&default).not()],
                    None,
                    provider,
                    read_allowance
                ),
                equality_outcome,
                "{provider}: width={element_width}"
            );
            let different_defaults = check(
                &[default.eq(&one), a.eq(Array::constant(256, &zero))],
                None,
                provider,
                DEFAULT_RLIMIT,
            );
            if provider == Provider::Bitwuzla {
                assert_bitwuzla_constant_array_unknown(different_defaults);
            } else {
                assert_eq!(
                    different_defaults,
                    Outcome::Unsat,
                    "{provider}: width={element_width}"
                );
            }
            let model = check(
                &[default.eq(&one), a.eq(&b)],
                Some(&b.select(&q)),
                provider,
                DEFAULT_RLIMIT,
            );
            if provider == Provider::Cvc5 && element_width == 256 {
                assert_eq!(model, Outcome::Unknown(Unknown::ResourceLimit));
                // The same query must be solved with a larger explicit native
                // allowance, rather than treating exhaustion as a proof.
                assert_eq!(
                    number(check(
                        &[default.eq(&one), a.eq(&b)],
                        Some(&b.select(&q)),
                        provider,
                        1_000_000,
                    )),
                    1
                );
            } else {
                assert_eq!(number(model), 1, "{provider}: width={element_width}");
            }
        }
    }
}

fn assert_bitwuzla_constant_array_unknown(outcome: Outcome) {
    let Outcome::Unknown(Unknown::Solver(reason)) = outcome else {
        panic!("expected Bitwuzla's native constant-array UNKNOWN, got {outcome:?}")
    };
    assert!(reason.starts_with("Bitwuzla returned unknown after "));
    assert!(reason.ends_with(" termination checks"));
}

#[test]
fn finite_index_complete_overwrite_preserves_reads_and_reports_native_equality_boundaries() {
    let zero_index = Bv::from_u64(0, 1);
    let one_index = Bv::from_u64(1, 1);
    let seven = Bv::from_u64(7, 13);
    let nine = Bv::from_u64(9, 13);
    for symbolic_default in [false, true] {
        let default = if symbolic_default {
            Context::default().fresh_bv("default", 13)
        } else {
            Bv::from_u64(0, 13)
        };
        let a = Array::constant(1, &default)
            .store(&zero_index, &seven)
            .store(&one_index, &nine);
        let b = Array::constant(1, &nine).store(&zero_index, &seven);
        for provider in PROVIDERS {
            // All providers must still prove the original read/write semantics
            // for both arrays; UNKNOWN is permitted only for the known native
            // constant-array equality cases below.
            let wrong_read = Bool::or(&[
                a.select(&zero_index).eq(&seven).not(),
                a.select(&one_index).eq(&nine).not(),
                b.select(&zero_index).eq(&seven).not(),
                b.select(&one_index).eq(&nine).not(),
            ]);
            assert_eq!(
                check(&[wrong_read], None, provider, DEFAULT_RLIMIT),
                Outcome::Unsat,
                "{provider}: symbolic_default={symbolic_default}"
            );
            for negate in [false, true] {
                let equality = a.eq(&b);
                let equality = if negate { equality.not() } else { equality };
                let outcome = check(&[equality], None, provider, DEFAULT_RLIMIT);
                match (provider, symbolic_default, negate) {
                    (Provider::Bitwuzla, _, false) => assert_bitwuzla_constant_array_unknown(outcome),
                    (Provider::Cvc5, true, _) => assert_eq!(outcome, Outcome::Error(Error {
                        provider: Provider::Cvc5,
                        message: "Array theory solver does not yet support write-chains connecting two different constant arrays".into(),
                    })),
                    (_, _, true) => assert_eq!(outcome, Outcome::Unsat, "{provider}: symbolic_default={symbolic_default}"),
                    (_, _, false) => assert_eq!(outcome, Outcome::Sat(None), "{provider}: symbolic_default={symbolic_default}"),
                }
            }
        }
    }
}

#[test]
#[should_panic(expected = "array index width must be positive")]
fn fresh_array_rejects_zero_index_width() {
    Context::default().fresh_array("bad", 0, 8);
}

#[test]
#[should_panic(expected = "array element width must be positive")]
fn fresh_array_rejects_zero_element_width() {
    Context::default().fresh_array("bad", 8, 0);
}

#[test]
#[should_panic(expected = "array index width must be positive")]
fn constant_array_rejects_zero_index_width() {
    Array::constant(0, &Bv::from_u64(0, 8));
}

#[test]
#[should_panic(expected = "array index widths must match")]
fn array_select_rejects_mismatched_index_width() {
    let array = Array::constant(8, &Bv::from_u64(0, 13));
    array.select(&Bv::from_u64(0, 9));
}

#[test]
#[should_panic(expected = "array element widths must match")]
fn array_store_rejects_mismatched_element_width() {
    let array = Array::constant(8, &Bv::from_u64(0, 13));
    array.store(&Bv::from_u64(0, 8), &Bv::from_u64(0, 8));
}

#[test]
#[should_panic(expected = "array sorts must match")]
fn array_equality_rejects_mismatched_index_width() {
    let array = Array::constant(8, &Bv::from_u64(0, 13));
    let other_index = Array::constant(9, &Bv::from_u64(0, 13));
    array.eq(&other_index);
}

#[test]
#[should_panic(expected = "array sorts must match")]
fn array_equality_rejects_mismatched_element_width() {
    let array = Array::constant(8, &Bv::from_u64(0, 13));
    let other_element = Array::constant(8, &Bv::from_u64(0, 8));
    array.eq(&other_element);
}

#[test]
#[should_panic(expected = "conditional array sorts must match")]
fn array_ite_rejects_mismatched_index_width() {
    let array = Array::constant(8, &Bv::from_u64(0, 13));
    let other_index = Array::constant(9, &Bv::from_u64(0, 13));
    Bool::from_bool(true).ite_array(&array, &other_index);
}

#[test]
#[should_panic(expected = "conditional array sorts must match")]
fn array_ite_rejects_mismatched_element_width() {
    let array = Array::constant(8, &Bv::from_u64(0, 13));
    let other_element = Array::constant(8, &Bv::from_u64(0, 8));
    Bool::from_bool(true).ite_array(&array, &other_element);
}
