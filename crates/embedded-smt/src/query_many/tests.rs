use super::{
    BudgetedUniqueOutcome, MAX_UNIQUE_PACKED_WIDTH, MAX_UNIQUE_TARGETS, QueryManyError,
    check_unique_many, check_unique_many_budgeted,
};
use crate::{
    Bool, Bv, Context, DEFAULT_RLIMIT, FunctionArg, FunctionSort, Provider, UniqueOutcome,
    check_unique, query_pair::tests::Observe,
};
use num_bigint::BigUint;

mod models;
mod resources;

fn number(printed: &str) -> BigUint {
    if let Some(hex) = printed.strip_prefix("#x") {
        BigUint::parse_bytes(hex.as_bytes(), 16).unwrap()
    } else {
        BigUint::parse_bytes(printed.strip_prefix("#b").unwrap().as_bytes(), 2).unwrap()
    }
}

fn oracle(base: &[Bool], targets: &[Bv], provider: Provider) -> Vec<UniqueOutcome> {
    targets
        .iter()
        .map(|target| check_unique(base, target, provider, DEFAULT_RLIMIT))
        .collect()
}

fn assert_same(actual: &[UniqueOutcome], expected: &[UniqueOutcome]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        match (actual, expected) {
            (UniqueOutcome::Unique(actual), UniqueOutcome::Unique(expected)) => {
                // Providers may choose binary instead of hexadecimal printing.
                assert_eq!(number(actual), number(expected));
            }
            _ => assert_eq!(actual, expected),
        }
    }
}

#[test]
fn sixteen_unique_256_bit_targets_use_three_native_checks_instead_of_thirty_two() {
    let context = Context::default();
    let targets: Vec<_> = (0..MAX_UNIQUE_TARGETS)
        .map(|_| context.fresh_bv("value", 256))
        .collect();
    let base: Vec<_> = targets
        .iter()
        .enumerate()
        .map(|(index, target)| target.eq(Bv::from_u64(index as u64 + 1, 256)))
        .collect();
    assert_eq!(
        targets.iter().map(Bv::width).sum::<u32>(),
        MAX_UNIQUE_PACKED_WIDTH
    );
    let expected = {
        let observe = Observe::new(None, None, None);
        let expected = oracle(&base, &targets, Provider::Z3);
        assert!(
            expected
                .iter()
                .all(|outcome| matches!(outcome, UniqueOutcome::Unique(_)))
        );
        assert_eq!(observe.native_calls(), 2 * targets.len());
        expected
    };
    let caller = ::z3::Context::thread_local();
    let caller_id = caller.get_z3_context().as_ptr() as usize;
    let observe = Observe::new(None, None, None);
    let actual = check_unique_many(&base, &targets, Provider::Z3, DEFAULT_RLIMIT).unwrap();
    assert_same(&actual, &expected);
    assert_eq!(observe.native_calls(), 3);
    let contexts = observe.native_contexts();
    assert_ne!(contexts[0], caller_id);
    assert_eq!(contexts[0], contexts[1]);
    assert_eq!(::z3::Context::thread_local(), caller);
}

#[test]
fn aliases_and_distinct_same_prefix_symbols_match_the_single_value_oracle() {
    for provider in [Provider::Z3, Provider::Bitwuzla, Provider::Cvc5] {
        let context = Context::default();
        let x = context.fresh_bv("same", 8);
        let y = context.fresh_bv("same", 8);
        let targets = [x.clone(), x.clone(), y];
        let expected = oracle(&[], &targets, provider);
        assert_eq!(expected, vec![UniqueOutcome::Multiple; 3]);
        {
            let observe = Observe::new(None, None, None);
            assert_same(
                &check_unique_many(&[], &targets, provider, DEFAULT_RLIMIT).unwrap(),
                &expected,
            );
            if provider == Provider::Z3 {
                assert!((2..=targets.len() + 2).contains(&observe.native_calls()));
            }
        }
        for value in [42, 43] {
            let base = [x.eq(Bv::from_u64(value, 8))];
            let expected = oracle(&base, &targets, provider);
            assert!(matches!(
                &expected[..],
                [
                    UniqueOutcome::Unique(_),
                    UniqueOutcome::Unique(_),
                    UniqueOutcome::Multiple
                ]
            ));
            let observe = Observe::new(None, None, None);
            let actual = check_unique_many(&base, &targets, provider, DEFAULT_RLIMIT).unwrap();
            assert_same(&actual, &expected);
            if provider == Provider::Z3 {
                // AND is unproved; OR changes y, then proves x and its alias unique.
                assert_eq!(observe.native_calls(), 4);
            }
        }
    }
}

#[test]
fn packed_models_split_at_the_real_widths_including_values_above_u64() {
    for provider in [Provider::Z3, Provider::Bitwuzla, Provider::Cvc5] {
        let context = Context::default();
        let values = [
            Bv::from_u64(1, 1),
            Bv::from_u64(5, 3),
            Bv::from_u64(0, 8),
            Bv::from_u64(257, 9),
            Bv::from_str(65, "18446744073709551617").unwrap(),
        ];
        let targets: Vec<_> = values
            .iter()
            .map(|value| context.fresh_bv("width", value.width()))
            .collect();
        let base: Vec<_> = targets
            .iter()
            .zip(&values)
            .map(|(target, value)| target.eq(value))
            .collect();
        let expected = oracle(&base, &targets, provider);
        assert!(
            expected
                .iter()
                .all(|outcome| matches!(outcome, UniqueOutcome::Unique(_)))
        );
        let actual = check_unique_many(&base, &targets, provider, DEFAULT_RLIMIT).unwrap();
        assert_same(&actual, &expected);
        for ((outcome, target), value) in actual.iter().zip(&targets).zip(&values) {
            let UniqueOutcome::Unique(printed) = outcome else {
                unreachable!()
            };
            let reconstructed =
                Bv::from_str(target.width(), &number(printed).to_str_radix(10)).unwrap();
            assert_eq!(reconstructed.literal_hex(), value.literal_hex());
        }
    }
}

#[test]
fn complete_array_aliases_and_function_declaration_identities_survive_every_check() {
    for provider in [Provider::Z3, Provider::Bitwuzla, Provider::Cvc5] {
        let context = Context::default();
        let a = context.fresh_array("array", 2, 8);
        let b = context.fresh_array("array", 2, 8);
        let i = context.fresh_bv("index", 2);
        let j = context.fresh_bv("index", 2);
        let sort = FunctionSort::Array {
            index_width: 2,
            element_width: 8,
        };
        let f = context.fresh_function("digest", &[sort], 8);
        let g = context.fresh_function("digest", &[sort], 8);
        let fa = f.apply(&[FunctionArg::Array(&a)]);
        let fb = f.clone().apply(&[FunctionArg::Array(&b)]);
        let gb = g.apply(&[FunctionArg::Array(&b)]);
        let selected = b.select(&j);
        let mut base = vec![
            a.eq(&b),
            i.eq(&j),
            a.select(&i).eq(Bv::from_u64(7, 8)),
            fa.eq(Bv::from_u64(42, 8)),
        ];
        let targets = [selected.clone(), fb, gb];
        let expected = oracle(&base, &targets, provider);
        assert!(matches!(
            &expected[..],
            [
                UniqueOutcome::Unique(_),
                UniqueOutcome::Unique(_),
                UniqueOutcome::Multiple
            ]
        ));
        assert_same(
            &check_unique_many(&base, &targets, provider, DEFAULT_RLIMIT).unwrap(),
            &expected,
        );
        base.push(selected.eq(Bv::from_u64(8, 8)));
        let expected = oracle(&base, &targets, provider);
        assert_eq!(expected, vec![UniqueOutcome::Infeasible; targets.len()]);
        let observe = Observe::new(None, None, None);
        assert_eq!(
            check_unique_many(&base, &targets, provider, DEFAULT_RLIMIT).unwrap(),
            expected
        );
        if provider == Provider::Z3 {
            assert_eq!(observe.native_calls(), 1);
        }
    }
}

#[test]
fn empty_and_oversize_queries_never_reach_a_native_solver_or_overflow_packing() {
    let context = Context::default();
    let observe = Observe::new(None, None, None);
    assert_eq!(
        check_unique_many(&[Bool::from_bool(false)], &[], Provider::Z3, 0),
        Ok(vec![])
    );
    let targets = vec![context.fresh_bv("too_many", 8); MAX_UNIQUE_TARGETS + 1];
    assert_eq!(
        check_unique_many(&[], &targets, Provider::Z3, DEFAULT_RLIMIT),
        Err(QueryManyError::TooManyTargets { actual: 17 }),
    );
    let targets = [context.fresh_bv("wide", 4096), context.fresh_bv("wide", 1)];
    assert_eq!(
        check_unique_many(&[], &targets, Provider::Z3, DEFAULT_RLIMIT),
        Err(QueryManyError::PackedWidthLimit { actual: 4097 }),
    );
    let targets = [
        context.fresh_bv("overflow", u32::MAX),
        context.fresh_bv("overflow", 1),
    ];
    assert_eq!(
        check_unique_many(&[], &targets, Provider::Z3, DEFAULT_RLIMIT),
        Err(QueryManyError::PackedWidthOverflow),
    );
    assert_eq!(observe.native_calls(), 0);
}
