use super::*;
use crate::{
    BudgetedOutcome, Cancellation, QueryBudget, Unknown, check_candidates_budgeted,
    with_cancellation,
};

#[test]
fn mathematical_hits_skip_real_native_calls_after_the_same_original_permits() {
    let observe = crate::query_pair::tests::Observe::new(None, None, None);
    let mut cache = DecidedCache::default();
    let x = Context::default().fresh_bv("x", 8);
    assert_eq!(
        check_unique_cached(
            &[x.eq(Bv::from_u64(7, 8))],
            &x,
            Provider::Z3,
            DEFAULT_RLIMIT,
            scope(),
            &mut cache
        ),
        UniqueOutcome::Unique("#x07".into())
    );
    assert_eq!(
        observe.native_calls(),
        2,
        "model + independent exclusion proof"
    );
    let y = Context::default().fresh_bv("renamed", 8);
    assert_eq!(
        check_unique_cached(
            &[y.eq(Bv::from_u64(7, 8))],
            &y,
            Provider::Z3,
            DEFAULT_RLIMIT,
            scope(),
            &mut cache
        ),
        UniqueOutcome::Unique("#x07".into())
    );
    assert_eq!(
        observe.native_calls(),
        2,
        "a proved warm answer executes no native check"
    );
    assert_eq!(cache.stats().unique_hits, 1);
    assert_eq!(
        candidate_current(
            &[x.eq(Bv::from_u64(7, 8))],
            Provider::Z3,
            DEFAULT_RLIMIT,
            scope(),
            &mut cache
        ),
        Outcome::Sat(None)
    );
    assert_eq!(observe.native_calls(), 3);
    assert_eq!(
        candidate_current(
            &[y.eq(Bv::from_u64(7, 8))],
            Provider::Z3,
            DEFAULT_RLIMIT,
            scope(),
            &mut cache
        ),
        Outcome::Sat(None)
    );
    assert_eq!(observe.native_calls(), 3);
    assert_eq!(cache.stats().candidate_hits, 1);
}

#[test]
fn provider_rlimit_and_encoding_policy_are_separate_namespaces() {
    let x = Context::default().fresh_bv("x", 8);
    let base = [x.eq(Bv::from_u64(7, 8))];
    let mut cache = DecidedCache::default();
    for provider in [Provider::Z3, Provider::Bitwuzla, Provider::Cvc5] {
        assert!(matches!(
            check_unique_cached(&base, &x, provider, DEFAULT_RLIMIT, scope(), &mut cache),
            UniqueOutcome::Unique(_)
        ));
    }
    assert_eq!(cache.stats().unique_hits, 0);
    assert!(matches!(
        check_unique_cached(
            &base,
            &x,
            Provider::Z3,
            DEFAULT_RLIMIT + 1,
            scope(),
            &mut cache
        ),
        UniqueOutcome::Unique(_)
    ));
    assert!(matches!(
        check_unique_cached(
            &base,
            &x,
            Provider::Z3,
            DEFAULT_RLIMIT,
            CacheScope {
                depth: 257,
                ..scope()
            },
            &mut cache
        ),
        UniqueOutcome::Unique(_)
    ));
    assert_eq!(cache.stats().unique_hits, 0);
    assert_eq!(cache.stats().entries, 5);
}

#[test]
fn unknown_errors_and_zero_allowance_never_become_answers() {
    let x = Context::default().fresh_bv("x", 8);
    let base = [x.bvmul(&x).eq(Bv::from_u64(4, 8))];
    let mut cache = DecidedCache::default();
    for _ in 0..2 {
        assert_eq!(
            check_unique_cached(&base, &x, Provider::Z3, 1, scope(), &mut cache),
            UniqueOutcome::Unknown(Unknown::ResourceLimit)
        );
        assert_eq!(
            candidate_current(&base, Provider::Z3, 1, scope(), &mut cache),
            Outcome::Unknown(Unknown::ResourceLimit)
        );
        assert!(matches!(
            check_unique_cached(&base, &x, Provider::Z3, 0, scope(), &mut cache),
            UniqueOutcome::Error(_)
        ));
        assert!(matches!(
            candidate_current(&base, Provider::Z3, 0, scope(), &mut cache),
            Outcome::Error(_)
        ));
    }
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(cache.stats().unique_hits + cache.stats().candidate_hits, 0);
    assert_eq!(cache.stats().undecided, 4);
}

#[test]
fn oversize_key_or_graph_bypasses_to_the_original_oracle() {
    let x = Context::default().fresh_bv("x", 8);
    let base = [x.eq(Bv::from_u64(42, 8))];
    for limits in [
        Limits {
            key_bytes: 8,
            ..Limits::default()
        },
        Limits {
            nodes: 1,
            ..Limits::default()
        },
        Limits {
            edges: 1,
            ..Limits::default()
        },
    ] {
        let mut cache = DecidedCache {
            limits,
            ..DecidedCache::default()
        };
        assert_eq!(
            unique(&mut cache, &base, &x),
            UniqueOutcome::Unique("#x2a".into())
        );
        assert_eq!(cache.stats().bypasses, 1);
        assert_eq!(cache.stats().entries, 0);
    }
}

#[test]
fn lru_and_retention_admission_bound_entries_key_bytes_and_answer_bytes() {
    let x = Context::default().fresh_bv("x", 8);
    let mut cache = DecidedCache {
        limits: Limits {
            entries: 2,
            ..Limits::default()
        },
        ..DecidedCache::default()
    };
    let one = [x.eq(Bv::from_u64(1, 8))];
    let two = [x.eq(Bv::from_u64(2, 8))];
    let three = [x.eq(Bv::from_u64(3, 8))];
    unique(&mut cache, &one, &x);
    unique(&mut cache, &two, &x);
    unique(&mut cache, &one, &x);
    unique(&mut cache, &three, &x);
    assert_eq!(cache.stats().unique_hits, 1);
    assert_eq!(cache.stats().entries, 2);
    assert_eq!(cache.stats().evictions, 1);
    unique(&mut cache, &one, &x);
    unique(&mut cache, &two, &x);
    assert_eq!(cache.stats().unique_hits, 2);
    assert_eq!(cache.stats().evictions, 2);
    assert!(cache.stats().retained_bytes <= cache.limits.retained_bytes);
    assert_eq!(
        cache.stats().retained_bytes,
        cache.stats().payload_bytes + cache.stats().entries * super::super::ENTRY_METADATA
    );
    let mut too_small = DecidedCache {
        limits: Limits {
            retained_bytes: super::super::ENTRY_METADATA,
            ..Limits::default()
        },
        ..DecidedCache::default()
    };
    unique(&mut too_small, &one, &x);
    assert_eq!(too_small.stats().entries, 0);
    assert_eq!(too_small.stats().bypasses, 1);
    cache.clock = u64::MAX;
    unique(&mut cache, &one, &x);
    assert_eq!(cache.stats().entries, 2);
    assert_eq!(cache.stats().unique_hits, 2);
}

#[test]
fn cancellation_still_wins_before_a_warm_hit_and_leaves_other_analysis_independent() {
    let x = Context::default().fresh_bv("x", 8);
    let base = [x.eq(Bv::from_u64(42, 8))];
    let mut cache = DecidedCache::default();
    unique(&mut cache, &base, &x);
    candidate(&mut cache, &base);
    let token = Cancellation::new();
    token.cancel();
    with_cancellation(&token, || {
        assert_eq!(
            check_unique_cached(&base, &x, Provider::Z3, DEFAULT_RLIMIT, scope(), &mut cache),
            UniqueOutcome::Unknown(Unknown::Cancelled)
        );
        assert_eq!(
            candidate_current(&base, Provider::Z3, DEFAULT_RLIMIT, scope(), &mut cache),
            Outcome::Unknown(Unknown::Cancelled)
        );
        let mut used = 0;
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            crate::current_cancellation(),
            &mut cache,
            scope(),
        );
        assert!(
            super::super::key::query(
                &base,
                Some(&x),
                Provider::Z3,
                DEFAULT_RLIMIT,
                scope(),
                Limits::default(),
                &mut query.budget
            )
            .is_err()
        );
    });
    assert_eq!(cache.stats().unique_hits + cache.stats().candidate_hits, 0);
    unique(&mut cache, &base, &x);
    assert_eq!(cache.stats().unique_hits, 1);
    let mut next_analysis = DecidedCache::default();
    unique(&mut next_analysis, &base, &x);
    assert_eq!(next_analysis.stats().unique_hits, 0);
}

#[test]
fn warm_candidates_keep_original_fees_first_work_and_unknown_order() {
    let x = Context::default().fresh_bv("x", 8);
    let candidates = [Bv::from_u64(1, 8), Bv::from_u64(2, 8)];
    let mut cache = DecidedCache::default();
    let mut used = 0;
    let mut exhausted = false;
    for _ in 0..2 {
        let mut budget = QueryBudget::new(
            1_000_000_000,
            &mut used,
            &mut exhausted,
            Cancellation::default(),
            1_000_000,
        )
        .with_cache(&mut cache, scope());
        assert_eq!(
            check_candidates_budgeted(
                &[],
                &x,
                &candidates,
                &[],
                Provider::Z3,
                DEFAULT_RLIMIT,
                &mut budget
            ),
            [
                BudgetedOutcome::Solver(Outcome::Sat(None)),
                BudgetedOutcome::Solver(Outcome::Sat(None))
            ]
        );
    }
    assert_eq!(used, 4_000_000 + cache.stats().maintenance_work as usize);
    assert_eq!(cache.stats().candidate_hits, 2);
    let before = used;
    let mut budget = QueryBudget::new(
        before + 999_999,
        &mut used,
        &mut exhausted,
        Cancellation::default(),
        1_000_000,
    )
    .with_cache(&mut cache, scope());
    assert_eq!(
        check_candidates_budgeted(
            &[],
            &x,
            &candidates,
            &[],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut budget
        ),
        [BudgetedOutcome::Work]
    );
    assert!(exhausted);
    assert_eq!(used, before);
    assert_eq!(cache.stats().candidate_hits, 2);
    let mut used = 0;
    let mut exhausted = false;
    let previous_maintenance = cache.stats().maintenance_work;
    let mut budget = QueryBudget::new(
        1_000_000_000,
        &mut used,
        &mut exhausted,
        Cancellation::default(),
        1_000_000,
    )
    .with_cache(&mut cache, scope());
    assert_eq!(
        check_candidates_budgeted(
            &[],
            &x.bvmul(&x),
            &candidates,
            &[],
            Provider::Z3,
            1,
            &mut budget
        ),
        [BudgetedOutcome::Solver(Outcome::Unknown(
            Unknown::ResourceLimit
        ))]
    );
    assert_eq!(
        used,
        1_000_000 + (cache.stats().maintenance_work - previous_maintenance) as usize
    );
    assert_eq!(cache.stats().candidate_hits, 2);
}

#[test]
fn refused_optional_maintenance_runs_the_oracle_without_exhausting_or_refunding() {
    let x = Context::default().fresh_bv("x", 8);
    let one = Bv::from_u64(1, 8);
    let mut cache = DecidedCache::default();
    candidate(&mut cache, &[x.eq(&one)]);
    let entries = cache.stats().entries;
    let maintenance_before = cache.stats().maintenance_work;
    let mut used = 0;
    let mut exhausted = false;
    let mut budget = QueryBudget::new(10, &mut used, &mut exhausted, Cancellation::default(), 10)
        .with_cache(&mut cache, scope());
    assert_eq!(
        check_candidates_budgeted(
            &[],
            &x,
            std::slice::from_ref(&one),
            &[],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut budget
        ),
        [BudgetedOutcome::Solver(Outcome::Sat(None))]
    );
    assert_eq!(used, 10);
    assert!(!exhausted);
    assert_eq!(cache.stats().candidate_hits, 0);
    assert_eq!(cache.stats().entries, entries);
    assert_eq!(cache.stats().maintenance_work, maintenance_before);
    let mut used = 0;
    let mut exhausted = false;
    let mut budget = QueryBudget::new(20, &mut used, &mut exhausted, Cancellation::default(), 10)
        .with_cache(&mut cache, scope());
    assert_eq!(
        check_candidates_budgeted(
            &[],
            &x,
            &[one, Bv::from_u64(2, 8)],
            &[],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut budget
        ),
        [
            BudgetedOutcome::Solver(Outcome::Sat(None)),
            BudgetedOutcome::Solver(Outcome::Sat(None))
        ]
    );
    // The two-unit allowance cannot pay the 128-unit hash-state setup, so
    // optional maintenance is refused without charging or exhausting the ledger.
    assert_eq!(used, 20);
    assert!(!exhausted);
    assert_eq!(cache.stats().candidate_hits, 0);
    assert_eq!(cache.stats().entries, entries);
    assert_eq!(cache.stats().maintenance_work, maintenance_before);

    // Admit exactly the setup charge, then refuse the first key byte. Paid
    // maintenance is retained, leaving too little for the second original fee.
    let fee = 4 * super::super::budget::hash_work(2);
    let mut used = 0;
    let mut exhausted = false;
    let mut budget = QueryBudget::new(
        2 * fee,
        &mut used,
        &mut exhausted,
        Cancellation::default(),
        fee,
    )
    .with_cache(&mut cache, scope());
    assert_eq!(
        check_candidates_budgeted(
            &[],
            &x,
            &[Bv::from_u64(1, 8), Bv::from_u64(2, 8)],
            &[],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut budget
        ),
        [
            BudgetedOutcome::Solver(Outcome::Sat(None)),
            BudgetedOutcome::Work
        ]
    );
    assert!(exhausted);
    assert_eq!(used, fee + super::super::budget::hash_work(2));
    assert_eq!(cache.stats().candidate_hits, 0);
    assert_eq!(cache.stats().entries, entries);
    assert_eq!(
        cache.stats().maintenance_work,
        maintenance_before + super::super::budget::hash_work(2) as u64
    );
}

#[test]
fn refused_hit_upkeep_cannot_reinsert_a_duplicate_key_or_ghost_lru_age() {
    let value = Bv::from_u64(7, 8);
    let mut cache = DecidedCache {
        limits: Limits {
            entries: 1,
            ..Limits::default()
        },
        ..DecidedCache::default()
    };
    unique(&mut cache, &[], &value);
    let payload = cache.stats().payload_bytes;
    let mut used = 0;
    let key = {
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            crate::current_cancellation(),
            &mut cache,
            scope(),
        );
        query
            .cache
            .key(
                &[],
                Some(&value),
                Provider::Z3,
                DEFAULT_RLIMIT,
                scope(),
                &mut query.budget,
            )
            .unwrap()
    };
    let construction = used;
    let lookup = super::super::budget::tree_work(1, super::super::index::INDEX_BYTES)
        + super::super::index::equality_work(1, key.bytes.len());
    let mut used = 0;
    let mut query = CacheQuery::new(
        construction + lookup,
        &mut used,
        crate::current_cancellation(),
        &mut cache,
        scope(),
    );
    assert_eq!(
        super::super::check_unique_cached(&[], &value, Provider::Z3, DEFAULT_RLIMIT, &mut query),
        UniqueOutcome::Unique("#x07".into())
    );
    drop(query);
    assert_eq!(used, construction + lookup);
    assert_eq!(cache.stats().entries, 1);
    assert_eq!(cache.stats().payload_bytes, payload);
    assert_eq!(cache.ages.len(), 1);
    unique(&mut cache, &[], &Bv::from_u64(8, 8));
    assert_eq!(cache.stats().entries, 1);
    assert_eq!(cache.ages.len(), 1);
    assert_eq!(cache.stats().evictions, 1);
}
