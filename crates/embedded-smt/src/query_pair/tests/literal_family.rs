use super::*;
use crate::{CacheQuery, CacheScope, DecidedCache, check_cached};

#[test]
fn literal_families_keep_individual_answers_with_one_preparation_per_chunk() {
    let x = Context::default().fresh_bv("literal", 8);
    {
        let observe = Observe::new(None, None, None);
        assert_eq!(
            check_candidates(
                &[x.bvule(Bv::from_u64(7, 8))],
                &x,
                &[1, 9, 5, 8].map(|n| Bv::from_u64(n, 8)),
                Provider::Z3,
                DEFAULT_RLIMIT,
            ),
            [
                Outcome::Sat(None),
                Outcome::Unsat,
                Outcome::Sat(None),
                Outcome::Unsat
            ]
        );
        assert_eq!(observe.trace().preparations.len(), 1);
        assert_eq!(observe.native_calls(), 4);
    }
    let literals = (0..64).map(|n| Bv::from_u64(n, 8)).collect::<Vec<_>>();
    let expected = vec![Outcome::Sat(None); 64];
    {
        let observe = Observe::new(None, None, None);
        assert_eq!(
            fresh_candidates(&[], &x, &literals, Provider::Z3, DEFAULT_RLIMIT),
            expected
        );
        assert_eq!(observe.trace().preparations.len(), 64);
        assert_eq!(observe.native_calls(), 64);
    }
    let observe = Observe::new(None, None, None);
    assert_eq!(
        check_candidates(&[], &x, &literals, Provider::Z3, DEFAULT_RLIMIT),
        expected
    );
    assert_eq!(observe.trace().contexts.len(), 2);
    assert_eq!(observe.trace().preparations.len(), 2);
    assert_eq!(observe.native_calls(), 64);
}

#[test]
fn literal_checks_reset_rlimit_after_cumulative_context_work_exceeds_it() {
    let rlimit = 1000;
    let x = Context::default().fresh_bv("reset", 8);
    let literals = (0..32).map(|n| Bv::from_u64(n, 8)).collect::<Vec<_>>();
    let observe = Observe::new(None, None, None);
    assert_eq!(
        check_candidates(&[], &x, &literals, Provider::Z3, rlimit),
        vec![Outcome::Sat(None); 32]
    );
    let trace = observe.trace();
    assert_eq!(trace.preparations.len(), 1);
    assert_eq!(trace.resources.len(), 32);
    let used = trace
        .resources
        .iter()
        .map(|(limit, before, after)| {
            assert_eq!(*limit, rlimit);
            let delta = after
                .expect("native resource counter")
                .checked_sub(before.expect("native resource counter"))
                .expect("monotonic context resource counter");
            assert!(delta < rlimit);
            u64::from(delta)
        })
        .sum::<u64>();
    assert!(
        used > u64::from(rlimit),
        "fixture must exercise cumulative work beyond one check's limit: {used}"
    );
}

#[test]
fn literal_family_pins_uf_congruence_and_array_definitions_before_pop() {
    let context = Context::default();
    let x = context.fresh_bv("x", 8);
    let y = context.fresh_bv("y", 8);
    let f = context.fresh_function("f", &[FunctionSort::Bv(8)], 8);
    let g = context.fresh_function("g", &[FunctionSort::Bv(8)], 8);
    let base = [f.apply(&[FunctionArg::Bv(&x)]).eq(Bv::from_u64(0, 8))];
    let after = [x.eq(&y)];
    let literals = [0, 1, 2, 0].map(|n| Bv::from_u64(n, 8));
    for (target, expected) in [
        (
            f.apply(&[FunctionArg::Bv(&y)]),
            vec![
                Outcome::Sat(None),
                Outcome::Unsat,
                Outcome::Unsat,
                Outcome::Sat(None),
            ],
        ),
        (g.apply(&[FunctionArg::Bv(&y)]), vec![Outcome::Sat(None); 4]),
    ] {
        let (mut used, mut exhausted) = (0, false);
        let mut budget =
            QueryBudget::new(28, &mut used, &mut exhausted, Cancellation::default(), 7);
        assert_eq!(
            check_candidates_budgeted(
                &base,
                &target,
                &literals,
                &after,
                Provider::Z3,
                DEFAULT_RLIMIT,
                &mut budget
            ),
            expected
                .into_iter()
                .map(BudgetedOutcome::Solver)
                .collect::<Vec<_>>()
        );
        assert_eq!(used, 28);
        assert!(!exhausted);
    }

    let a = context.fresh_array("a", 8, 8);
    let b = context.fresh_array("b", 8, 8);
    let target = b.select(&x);
    let h = context.fresh_function("stored", &[FunctionSort::Bv(8)], 8);
    let base = [h.apply(&[FunctionArg::Bv(&x)]).eq(Bv::from_u64(7, 8))];
    let after = [
        y.eq(&x),
        a.select(&y).eq(Bv::from_u64(11, 8)),
        b.eq(a.store(&y, &h.apply(&[FunctionArg::Bv(&y)]))),
    ];
    let (mut used, mut exhausted) = (0, false);
    let mut budget = QueryBudget::new(28, &mut used, &mut exhausted, Cancellation::default(), 7);
    assert_eq!(
        check_candidates_budgeted(
            &base,
            &target,
            &[7, 11, 8, 7].map(|n| Bv::from_u64(n, 8)),
            &after,
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut budget
        ),
        [
            Outcome::Sat(None),
            Outcome::Unsat,
            Outcome::Unsat,
            Outcome::Sat(None)
        ]
        .map(BudgetedOutcome::Solver)
    );
    assert_eq!(used, 28);
    assert!(!exhausted);
}

#[test]
fn small_nonliteral_and_other_provider_families_keep_the_fresh_path() {
    let x = Context::default().fresh_bv("original", 8);
    for (candidates, expected) in [
        (vec![Bv::from_u64(1, 8)], vec![Outcome::Sat(None)]),
        (
            vec![Bv::from_u64(1, 8), Bv::from_u64(2, 8)],
            vec![Outcome::Sat(None); 2],
        ),
        (
            vec![
                Bv::from_u64(0, 8),
                x.bvadd(Bv::from_u64(1, 8)),
                Bv::from_u64(2, 8),
            ],
            vec![Outcome::Sat(None), Outcome::Unsat, Outcome::Sat(None)],
        ),
    ] {
        let observe = Observe::new(None, None, None);
        assert_eq!(
            check_candidates(&[], &x, &candidates, Provider::Z3, DEFAULT_RLIMIT),
            expected
        );
        assert_eq!(observe.trace().preparations.len(), candidates.len());
        assert_eq!(observe.native_calls(), candidates.len());
    }
    for provider in [Provider::Bitwuzla, Provider::Cvc5] {
        let observe = Observe::new(None, None, None);
        assert_eq!(
            check_candidates(
                &[],
                &x,
                &[0, 1, 2].map(|n| Bv::from_u64(n, 8)),
                provider,
                DEFAULT_RLIMIT
            ),
            vec![Outcome::Sat(None); 3]
        );
        assert!(observe.trace().preparations.is_empty());
        assert!(observe.trace().contexts.is_empty());
    }
    let observe = Observe::new(None, None, None);
    assert_eq!(
        check_unique(
            &[x.eq(Bv::from_u64(7, 8))],
            &x,
            Provider::Z3,
            DEFAULT_RLIMIT
        ),
        UniqueOutcome::Unique("#x07".into())
    );
    assert_eq!(observe.trace().preparations.len(), 2);
}

#[test]
fn individual_full_cache_keys_allow_lazy_misses_and_no_all_hit_preparation() {
    let x = Context::default().fresh_bv("cached", 8);
    let base = [x.bvule(Bv::from_u64(7, 8))];
    let after = [x.eq(Bv::from_u64(5, 8))];
    let literals = [4, 5, 6].map(|n| Bv::from_u64(n, 8));
    let expected = [Outcome::Unsat, Outcome::Sat(None), Outcome::Unsat];
    let scope = CacheScope {
        enabled: true,
        constraints: 2048,
        nodes: 16_384,
        depth: 256,
    };
    let mut cache = DecidedCache::default();
    let full = |candidate: &Bv| {
        let mut assertions = base.to_vec();
        assertions.push(x.eq(candidate));
        assertions.extend_from_slice(&after);
        assertions
    };
    let mut maintenance = 0;
    let mut query = CacheQuery::new(
        usize::MAX,
        &mut maintenance,
        Cancellation::default(),
        &mut cache,
        scope,
    );
    assert_eq!(
        check_cached(
            &full(&literals[0]),
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut query
        ),
        Outcome::Unsat
    );
    drop(query);
    for native in [2, 0] {
        let observe = Observe::new(None, None, None);
        let (mut used, mut exhausted) = (0, false);
        let mut budget = QueryBudget::new(
            10_000_000,
            &mut used,
            &mut exhausted,
            Cancellation::default(),
            1_000_000,
        )
        .with_cache(&mut cache, scope);
        assert_eq!(
            check_candidates_budgeted(
                &base,
                &x,
                &literals,
                &after,
                Provider::Z3,
                DEFAULT_RLIMIT,
                &mut budget
            ),
            expected.clone().map(BudgetedOutcome::Solver)
        );
        assert!(used >= 3_000_000);
        assert!(!exhausted);
        assert_eq!(observe.native_calls(), native);
        assert_eq!(observe.trace().preparations.len(), usize::from(native > 0));
    }
    let observe = Observe::new(None, None, None);
    let mut query = CacheQuery::new(
        usize::MAX,
        &mut maintenance,
        Cancellation::default(),
        &mut cache,
        scope,
    );
    for (literal, expected) in literals.iter().zip(expected) {
        assert_eq!(
            check_cached(&full(literal), Provider::Z3, DEFAULT_RLIMIT, &mut query),
            expected
        );
    }
    assert_eq!(observe.native_calls(), 0);
    // The same candidate without its definition suffix is a different Fi.
    assert_eq!(
        check_cached(
            &[base[0].clone(), x.eq(&literals[0])],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut query
        ),
        Outcome::Sat(None)
    );
    assert_eq!(observe.native_calls(), 1);
}

#[test]
fn literal_family_preserves_first_work_unknown_validation_and_cancellation() {
    let x = Context::default().fresh_bv("boundary", 8);
    let literals = [4, 9, 16].map(|n| Bv::from_u64(n, 8));
    {
        let observe = Observe::new(None, None, None);
        let (mut used, mut exhausted) = (0, false);
        let mut budget = QueryBudget::new(0, &mut used, &mut exhausted, Cancellation::default(), 7);
        assert_eq!(
            check_candidates_budgeted(
                &[],
                &x,
                &literals,
                &[],
                Provider::Z3,
                DEFAULT_RLIMIT,
                &mut budget
            ),
            [BudgetedOutcome::Work]
        );
        assert_eq!(used, 0);
        assert!(exhausted);
        assert!(observe.trace().preparations.is_empty());
        assert!(observe.trace().contexts.is_empty());
    }
    {
        let observe = Observe::new(None, None, None);
        assert!(matches!(
            check_candidates(&[], &x, &literals, Provider::Z3, 0).as_slice(),
            [Outcome::Error(_)]
        ));
        let token = Cancellation::new();
        token.cancel();
        assert_eq!(
            with_cancellation(&token, || check_candidates(
                &[],
                &x,
                &literals,
                Provider::Z3,
                DEFAULT_RLIMIT
            )),
            [Outcome::Unknown(Unknown::Cancelled)]
        );
        assert!(observe.trace().preparations.is_empty());
    }
    {
        let observe = Observe::new(None, None, None);
        let (mut used, mut exhausted) = (0, false);
        let mut budget =
            QueryBudget::new(21, &mut used, &mut exhausted, Cancellation::default(), 7);
        assert_eq!(
            check_candidates_budgeted(
                &[],
                &x.bvmul(&x),
                &literals,
                &[],
                Provider::Z3,
                1,
                &mut budget
            ),
            [BudgetedOutcome::Solver(Outcome::Unknown(
                Unknown::ResourceLimit
            ))]
        );
        assert_eq!(used, 7);
        assert!(!exhausted);
        assert_eq!(observe.native_calls(), 1);
        assert_eq!(observe.trace().preparations.len(), 1);
    }
    let token = Cancellation::new();
    let observe = Observe::new(None, None, Some((1, token.clone())));
    let (mut used, mut exhausted) = (0, false);
    let mut budget = QueryBudget::new(21, &mut used, &mut exhausted, token.clone(), 7);
    assert_eq!(
        with_cancellation(&token, || check_candidates_budgeted(
            &[],
            &x,
            &literals,
            &[],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut budget
        )),
        [
            BudgetedOutcome::Solver(Outcome::Sat(None)),
            BudgetedOutcome::Work
        ]
    );
    assert_eq!(used, 7);
    assert!(exhausted);
    assert_eq!(observe.native_calls(), 1);
    assert_eq!(observe.trace().preparations.len(), 1);
}

#[test]
fn common_congruence_uses_one_original_allowance_before_native_checks() {
    let context = Context::default();
    let x = context.fresh_bv("x", 8);
    let y = context.fresh_bv("y", 8);
    let f = context.fresh_function("bounded", &[FunctionSort::Bv(8)], 8);
    let base = [f.apply(&[FunctionArg::Bv(&x)]).eq(Bv::from_u64(0, 8))];
    let target = f.apply(&[FunctionArg::Bv(&y)]);
    let observe = Observe::new(None, None, None);
    let (mut used, mut exhausted) = (0, false);
    let mut budget = QueryBudget::new(21, &mut used, &mut exhausted, Cancellation::default(), 7);
    assert_eq!(
        check_candidates_budgeted(
            &base,
            &target,
            &[0, 1, 2].map(|n| Bv::from_u64(n, 8)),
            &[x.eq(&y)],
            Provider::Z3,
            5,
            &mut budget
        ),
        [BudgetedOutcome::Solver(Outcome::Unknown(
            Unknown::ResourceLimit
        ))]
    );
    assert_eq!(used, 7);
    assert!(!exhausted);
    assert_eq!(observe.native_calls(), 0);
    assert_eq!(observe.trace().preparations.len(), 1);
}
