//! Ordinary feasibility keeps its independent native session on every miss.

use super::{CacheQuery, DecidedCache, candidate_current, scope};
use crate::{
    Bool, Bv, Cancellation, Context, DEFAULT_RLIMIT, Outcome, Provider, Unknown, check,
    check_cached, with_cancellation,
};

fn cached(base: &[Bool], provider: Provider, rlimit: u32, cache: &mut DecidedCache) -> Outcome {
    let mut used = 0;
    let mut query = CacheQuery::new(
        usize::MAX,
        &mut used,
        crate::current_cancellation(),
        cache,
        scope(),
    );
    check_cached(base, provider, rlimit, &mut query)
}

#[test]
fn public_feasibility_reuses_alpha_renamed_sat_and_unsat_for_each_provider() {
    for provider in [Provider::Z3, Provider::Bitwuzla, Provider::Cvc5] {
        let mut cache = DecidedCache::default();
        for repeat in 0..2 {
            let x = Context::default().fresh_bv("renamed", 8);
            for (base, expected) in [
                (vec![x.eq(Bv::from_u64(7, 8))], Outcome::Sat(None)),
                (
                    vec![x.eq(Bv::from_u64(7, 8)), x.eq(Bv::from_u64(8, 8))],
                    Outcome::Unsat,
                ),
            ] {
                assert_eq!(check(&base, None, provider, DEFAULT_RLIMIT), expected);
                assert_eq!(
                    cached(&base, provider, DEFAULT_RLIMIT, &mut cache),
                    expected
                );
            }
            assert_eq!(cache.stats().candidate_hits, repeat * 2);
        }
        assert_eq!(cache.stats().entries, 2);
        assert_eq!(cache.stats().unique_hits, 0);
    }
}

#[test]
fn miss_and_maintenance_bypass_use_fresh_context_while_candidate_miss_keeps_current() {
    ::z3::with_z3_config(&::z3::Config::new(), || {
        let caller = ::z3::Context::thread_local();
        let caller_id = caller.get_z3_context().as_ptr() as usize;
        let observe = crate::query_pair::tests::Observe::new(None, None, None);
        let mut cache = DecidedCache::default();
        let x = Context::default().fresh_bv("x", 8);
        let base = [x.eq(Bv::from_u64(7, 8))];
        assert_eq!(
            cached(&base, Provider::Z3, DEFAULT_RLIMIT, &mut cache),
            Outcome::Sat(None)
        );
        assert_ne!(observe.native_contexts()[0], caller_id);
        assert_eq!(
            cached(&base, Provider::Z3, DEFAULT_RLIMIT, &mut cache),
            Outcome::Sat(None)
        );
        assert_eq!(
            observe.native_calls(),
            1,
            "warm feasibility skips native work"
        );

        let mut used = 0;
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            Cancellation::new(),
            &mut cache,
            scope(),
        )
        .with_allowance(0);
        assert_eq!(
            check_cached(&base, Provider::Z3, DEFAULT_RLIMIT, &mut query),
            Outcome::Sat(None)
        );
        drop(query);
        assert_ne!(observe.native_contexts()[1], caller_id);
        assert_eq!(used, 0);

        let different = [x.eq(Bv::from_u64(8, 8))];
        assert_eq!(
            candidate_current(
                &different,
                Provider::Z3,
                DEFAULT_RLIMIT,
                scope(),
                &mut cache
            ),
            Outcome::Sat(None)
        );
        assert_eq!(observe.native_contexts()[2], caller_id);
        assert_eq!(
            ::z3::Context::thread_local().get_z3_context().as_ptr() as usize,
            caller_id,
            "private feasibility sessions restore the caller context"
        );
    });
}

#[test]
fn complete_feasibility_keys_share_candidate_answers_without_merging_guards_or_aliases() {
    let mut cache = DecidedCache::default();
    let x = Context::default().fresh_bv("x", 8);
    let y = Context::default().fresh_bv("y", 8);
    let same = [x.eq(Bv::from_u64(0, 8)), x.eq(Bv::from_u64(1, 8))];
    let distinct = [x.eq(Bv::from_u64(0, 8)), y.eq(Bv::from_u64(1, 8))];
    assert_eq!(
        cached(&same, Provider::Z3, DEFAULT_RLIMIT, &mut cache),
        Outcome::Unsat
    );
    assert_eq!(
        candidate_current(&same, Provider::Z3, DEFAULT_RLIMIT, scope(), &mut cache),
        Outcome::Unsat
    );
    assert_eq!(
        candidate_current(&distinct, Provider::Z3, DEFAULT_RLIMIT, scope(), &mut cache),
        Outcome::Sat(None)
    );
    assert_eq!(
        cached(&distinct, Provider::Z3, DEFAULT_RLIMIT, &mut cache),
        Outcome::Sat(None)
    );
    let changed = [same[0].clone(), same[1].not()];
    assert_eq!(
        check(&changed, None, Provider::Z3, DEFAULT_RLIMIT),
        Outcome::Sat(None)
    );
    assert_eq!(
        cached(&changed, Provider::Z3, DEFAULT_RLIMIT, &mut cache),
        Outcome::Sat(None)
    );
    assert_eq!(cache.stats().candidate_hits, 2);
    assert_eq!(cache.stats().entries, 3);
}

#[test]
fn native_unknown_and_error_are_not_retained_and_zero_rlimit_keeps_its_type() {
    let mut cache = DecidedCache::default();
    let x = Context::default().fresh_bv("x", 8);
    let hard = [x.bvmul(&x).eq(Bv::from_u64(4, 8))];
    let invalid = [Context::default()
        .fresh_bv("invalid\0name", 8)
        .eq(Bv::from_u64(7, 8))];
    for _ in 0..2 {
        assert_eq!(
            check(&hard, None, Provider::Z3, 1),
            Outcome::Unknown(Unknown::ResourceLimit)
        );
        assert_eq!(
            cached(&hard, Provider::Z3, 1, &mut cache),
            Outcome::Unknown(Unknown::ResourceLimit)
        );
        let error = check(&invalid, None, Provider::Z3, DEFAULT_RLIMIT);
        assert!(matches!(error, Outcome::Error(_)));
        assert_eq!(
            cached(&invalid, Provider::Z3, DEFAULT_RLIMIT, &mut cache),
            error
        );
    }
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(cache.stats().undecided, 4);
    let warm = [x.eq(Bv::from_u64(7, 8))];
    assert_eq!(
        cached(&warm, Provider::Z3, DEFAULT_RLIMIT, &mut cache),
        Outcome::Sat(None)
    );
    let before = cache.stats();
    let error = check(&warm, None, Provider::Z3, 0);
    assert!(matches!(error, Outcome::Error(_)));
    assert_eq!(cached(&warm, Provider::Z3, 0, &mut cache), error);
    assert_eq!(cache.stats(), before);
}

#[test]
fn current_cancellation_wins_on_warm_hits_and_before_zero_rlimit_validation() {
    let mut cache = DecidedCache::default();
    let base = [Context::default().fresh_bv("x", 8).eq(Bv::from_u64(7, 8))];
    assert_eq!(
        cached(&base, Provider::Z3, DEFAULT_RLIMIT, &mut cache),
        Outcome::Sat(None)
    );
    let before = cache.stats();
    let current = Cancellation::new();
    current.cancel();
    with_cancellation(&current, || {
        for rlimit in [0, DEFAULT_RLIMIT] {
            let mut used = 0;
            let mut query = CacheQuery::new(
                usize::MAX,
                &mut used,
                Cancellation::new(),
                &mut cache,
                scope(),
            );
            assert_eq!(
                check_cached(&base, Provider::Z3, rlimit, &mut query),
                Outcome::Unknown(Unknown::Cancelled)
            );
            drop(query);
            assert_eq!(used, 0);
        }
    });
    assert_eq!(cache.stats(), before);

    // A different, captured maintenance token only refuses optional work. The
    // ordinary oracle still observes the current token, as in candidate checks.
    let captured = Cancellation::new();
    captured.cancel();
    let mut used = 0;
    let mut query = CacheQuery::new(usize::MAX, &mut used, captured, &mut cache, scope());
    assert_eq!(
        check_cached(&base, Provider::Z3, DEFAULT_RLIMIT, &mut query),
        Outcome::Sat(None)
    );
    drop(query);
    assert_eq!(used, 0);
    assert_eq!(cache.stats().candidate_hits, 0);
}

#[test]
fn zero_or_insufficient_optional_allowance_falls_back_to_the_exact_oracle() {
    let observe = crate::query_pair::tests::Observe::new(None, None, None);
    let mut cache = DecidedCache::default();
    let x = Context::default().fresh_bv("x", 8);
    for base in [
        vec![x.eq(Bv::from_u64(7, 8))],
        vec![x.eq(Bv::from_u64(7, 8)), x.eq(Bv::from_u64(8, 8))],
    ] {
        let expected = check(&base, None, Provider::Z3, DEFAULT_RLIMIT);
        assert_eq!(
            cached(&base, Provider::Z3, DEFAULT_RLIMIT, &mut cache),
            expected
        );
        for (maximum, allowance) in [
            (usize::MAX, 0),
            (usize::MAX, 1),
            (usize::MAX, 64),
            (73, usize::MAX),
        ] {
            let calls = observe.native_calls();
            let before = cache.stats();
            let mut used = 73;
            let mut query =
                CacheQuery::new(maximum, &mut used, Cancellation::new(), &mut cache, scope())
                    .with_allowance(allowance);
            assert_eq!(
                check_cached(&base, Provider::Z3, DEFAULT_RLIMIT, &mut query),
                expected
            );
            drop(query);
            assert!(used - 73 <= allowance);
            assert!(used <= maximum);
            assert_eq!(observe.native_calls(), calls + 1);
            assert_eq!(cache.stats().candidate_hits, before.candidate_hits);
            assert_eq!(cache.stats().entries, before.entries);
            assert_eq!(
                cache.stats().maintenance_work - before.maintenance_work,
                (used - 73) as u64
            );
        }
    }
}
