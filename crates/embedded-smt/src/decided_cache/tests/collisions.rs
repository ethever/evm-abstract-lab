use super::*;
use crate::{BudgetedOutcome, Cancellation, QueryBudget, Unknown, check_candidates_budgeted};

fn cache(entries: usize) -> DecidedCache {
    DecidedCache {
        force_collisions: true,
        limits: Limits {
            entries,
            ..Limits::default()
        },
        ..DecidedCache::default()
    }
}

fn consistent(cache: &DecidedCache) {
    let bucket_entries: usize = cache.entries.values().map(Vec::len).sum();
    assert_eq!(bucket_entries, cache.stats().entries);
    assert_eq!(cache.ages.len(), bucket_entries);
    let mut payload = 0;
    let mut retained = 0;
    for (digest, bucket) in &cache.entries {
        assert!(!bucket.is_empty() && bucket.len() <= super::super::index::BUCKET_LIMIT);
        for entry in bucket {
            assert_eq!(cache.ages.get(&entry.age), Some(digest));
            payload += entry.payload;
            retained += entry.charge;
        }
    }
    assert_eq!(payload, cache.stats().payload_bytes);
    assert_eq!(retained, cache.stats().retained_bytes);
    assert!(retained <= cache.limits.retained_bytes);
}

#[test]
fn collisions_need_full_eq_for_sat_unsat_unique_and_cross_root_aliases() {
    let observe = crate::query_pair::tests::Observe::new(None, None, None);
    let mut cache = cache(8192);
    for repeat in 0..2 {
        let symbols = Context::default();
        let x = symbols.fresh_bv("x", 8);
        let y = symbols.fresh_bv("y", 8);
        let same = [x.eq(Bv::from_u64(0, 8)), x.eq(Bv::from_u64(1, 8))];
        let different = [x.eq(Bv::from_u64(0, 8)), y.eq(Bv::from_u64(1, 8))];
        assert_eq!(
            candidate_current(&same, Provider::Z3, DEFAULT_RLIMIT, scope(), &mut cache),
            Outcome::Unsat
        );
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
        for value in [7, 8] {
            assert_eq!(
                check_unique_cached(
                    &[x.eq(Bv::from_u64(value, 8))],
                    &x,
                    Provider::Z3,
                    DEFAULT_RLIMIT,
                    scope(),
                    &mut cache
                ),
                UniqueOutcome::Unique(format!("#x{value:02x}"))
            );
        }
        assert_eq!(cache.stats().entries, 4);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.stats().candidate_hits, repeat * 2);
        assert_eq!(cache.stats().unique_hits, repeat * 2);
        assert_eq!(
            observe.native_calls(),
            6,
            "warm collisions must skip native"
        );
        consistent(&cache);
    }
}

#[test]
fn full_collision_bucket_falls_back_without_displacing_its_four_proofs() {
    let observe = crate::query_pair::tests::Observe::new(None, None, None);
    let mut cache = cache(8192);
    for value in 0..4 {
        assert!(matches!(
            check_unique_cached(
                &[],
                &Bv::from_u64(value, 8),
                Provider::Z3,
                DEFAULT_RLIMIT,
                scope(),
                &mut cache
            ),
            UniqueOutcome::Unique(_)
        ));
    }
    let previous = observe.native_calls();
    for _ in 0..2 {
        assert_eq!(
            check_unique_cached(
                &[],
                &Bv::from_u64(42, 8),
                Provider::Z3,
                DEFAULT_RLIMIT,
                scope(),
                &mut cache
            ),
            UniqueOutcome::Unique("#x2a".into())
        );
        consistent(&cache);
    }
    assert_eq!(observe.native_calls(), previous + 4);
    assert_eq!(cache.stats().insertions, 4);
    assert_eq!(cache.stats().evictions, 0);
    assert_eq!(cache.stats().unique_hits, 0);
    assert_eq!(cache.stats().bypasses, 2);
    assert_eq!(cache.entries.len(), 1);
    assert_eq!(cache.stats().entries, 4);
}

#[test]
fn colliding_model_targets_and_array_store_order_have_distinct_answers() {
    let mut cache = cache(8192);
    let symbols = Context::default();
    let x = symbols.fresh_bv("x", 8);
    let y = symbols.fresh_bv("y", 8);
    let base = [x.eq(Bv::from_u64(7, 8))];
    assert_eq!(
        unique(&mut cache, &base, &x),
        UniqueOutcome::Unique("#x07".into())
    );
    assert_eq!(unique(&mut cache, &base, &y), UniqueOutcome::Multiple);
    let array = symbols.fresh_array("a", 8, 8);
    let zero = Bv::from_u64(0, 8);
    let one = Bv::from_u64(1, 8);
    let two = Bv::from_u64(2, 8);
    let forward = array.store(&zero, &one).store(&zero, &two).select(&zero);
    let reverse = array.store(&zero, &two).store(&zero, &one).select(&zero);
    for _ in 0..2 {
        assert_eq!(
            unique(&mut cache, &[], &forward),
            UniqueOutcome::Unique("#x02".into())
        );
        assert_eq!(
            unique(&mut cache, &[], &reverse),
            UniqueOutcome::Unique("#x01".into())
        );
    }
    assert_eq!(cache.stats().unique_hits, 2);
    assert_eq!(cache.stats().entries, 4);
    consistent(&cache);
}

#[test]
fn collision_lru_removes_one_age_not_the_whole_bucket_and_overflow_bypasses() {
    let mut cache = cache(2);
    for value in [1, 2, 1, 3, 1, 2] {
        assert_eq!(
            unique(&mut cache, &[], &Bv::from_u64(value, 8)),
            UniqueOutcome::Unique(format!("#x{value:02x}"))
        );
        consistent(&cache);
    }
    assert_eq!(cache.stats().entries, 2);
    assert_eq!(cache.entries.len(), 1);
    assert_eq!(cache.stats().unique_hits, 2);
    assert_eq!(cache.stats().evictions, 2);
    let previous = cache.stats();
    cache.clock = u64::MAX;
    assert_eq!(
        unique(&mut cache, &[], &Bv::from_u64(1, 8)),
        UniqueOutcome::Unique("#x01".into())
    );
    assert_eq!(cache.stats().unique_hits, previous.unique_hits);
    assert_eq!(cache.stats().payload_bytes, previous.payload_bytes);
    consistent(&cache);
}

#[test]
fn colliding_warm_queries_keep_cancel_and_original_work_before_lookup() {
    let observe = crate::query_pair::tests::Observe::new(None, None, None);
    let mut cache = cache(4);
    let x = Context::default().fresh_bv("x", 8);
    let values = [Bv::from_u64(1, 8), Bv::from_u64(2, 8)];
    for value in &values {
        assert_eq!(
            candidate_current(
                &[x.eq(value)],
                Provider::Z3,
                DEFAULT_RLIMIT,
                scope(),
                &mut cache
            ),
            Outcome::Sat(None)
        );
    }
    let native = observe.native_calls();
    let token = Cancellation::new();
    token.cancel();
    crate::with_cancellation(&token, || {
        assert_eq!(
            candidate_current(
                &[x.eq(&values[0])],
                Provider::Z3,
                DEFAULT_RLIMIT,
                scope(),
                &mut cache
            ),
            Outcome::Unknown(Unknown::Cancelled)
        );
    });
    let mut used = 0;
    let mut exhausted = false;
    let mut query = QueryBudget::new(99, &mut used, &mut exhausted, Cancellation::default(), 100)
        .with_cache(&mut cache, scope());
    assert_eq!(
        check_candidates_budgeted(
            &[],
            &x,
            &values,
            &[],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut query
        ),
        [BudgetedOutcome::Work]
    );
    drop(query);
    assert!(exhausted);
    assert_eq!(used, 0);
    assert_eq!(observe.native_calls(), native);
    assert_eq!(cache.stats().candidate_hits, 0);
    consistent(&cache);
    // A paid query with no optional allowance executes the correct oracle and
    // preserves the bounded bucket and existing ages instead of false hits.
    let mut used = 0;
    let mut query = CacheQuery::new(0, &mut used, Cancellation::default(), &mut cache, scope());
    assert_eq!(
        super::super::candidate_current(
            &[x.eq(&values[1])],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut query
        ),
        Outcome::Sat(None)
    );
    drop(query);
    assert_eq!(observe.native_calls(), native + 1);
    assert_eq!(used, 0);
    consistent(&cache);
}

#[test]
fn long_full_keys_hit_with_the_original_quarter_query_allowance() {
    let observe = crate::query_pair::tests::Observe::new(None, None, None);
    let mut cache = DecidedCache::default();
    for value in 0..64 {
        check_unique_cached(
            &[],
            &Bv::from_u64(value, 8),
            Provider::Z3,
            DEFAULT_RLIMIT,
            scope(),
            &mut cache,
        );
    }
    let native = observe.native_calls();
    let maintenance = cache.stats().maintenance_work;
    let rlimit = 10_000_000;
    let original_fee = 2 * rlimit as usize;
    let mut used = 0;
    for _ in 0..2 {
        let symbols = Context::default();
        let mut base = Vec::new();
        let mut model = None;
        for value in 0..64 {
            let variable = symbols.fresh_bv("input", 256);
            base.push(variable.eq(Bv::from_u64(value, 256)));
            if value == 0 {
                model = Some(variable);
            }
        }
        let model = model.unwrap();
        assert!(key(&base, Some(&model)).len() > 4096);
        // The original two-check admission is still paid on a warm hit.
        used += original_fee;
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            Cancellation::default(),
            &mut cache,
            scope(),
        )
        .with_allowance(original_fee / 4);
        assert_eq!(
            super::super::check_unique_cached(&base, &model, Provider::Z3, rlimit, &mut query),
            UniqueOutcome::Unique(format!("#x{:064x}", 0))
        );
    }
    assert_eq!(cache.stats().unique_hits, 1);
    assert_eq!(observe.native_calls(), native + 2);
    assert_eq!(cache.stats().entries, 65);
    assert_eq!(
        used,
        2 * original_fee + (cache.stats().maintenance_work - maintenance) as usize
    );
    consistent(&cache);
}
