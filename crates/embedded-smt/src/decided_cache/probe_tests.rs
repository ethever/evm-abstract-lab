use super::{CacheQuery, CacheScope, DecidedCache, Limits, probe_unique_cached};
use crate::{
    Bool, Bv, Cancellation, Context, DEFAULT_RLIMIT, Provider, UniqueOutcome, Unknown,
    current_cancellation, with_cancellation,
};

fn scope() -> CacheScope {
    CacheScope {
        enabled: true,
        constraints: 2048,
        nodes: 16_384,
        depth: 256,
    }
}

// Seed cache internals directly so probe tests execute no oracle, even while
// arranging a warm cache or an invalid retained answer.
fn remember(cache: &mut DecidedCache, base: &[Bool], value: &Bv, answer: &UniqueOutcome) {
    let mut used = 0;
    let mut query = CacheQuery::new(
        usize::MAX,
        &mut used,
        current_cancellation(),
        cache,
        scope(),
    );
    let key = query
        .cache
        .key(
            base,
            Some(value),
            Provider::Z3,
            DEFAULT_RLIMIT,
            query.scope,
            &mut query.budget,
        )
        .expect("bounded complete key");
    query.cache.remember_unique(key, answer, &mut query.budget);
}

fn probe(
    cache: &mut DecidedCache,
    base: &[Bool],
    value: &Bv,
    provider: Provider,
    rlimit: u32,
    scope: CacheScope,
) -> Result<Option<UniqueOutcome>, Unknown> {
    let mut used = 0;
    let mut query = CacheQuery::new(usize::MAX, &mut used, current_cancellation(), cache, scope);
    probe_unique_cached(base, value, provider, rlimit, &mut query)
}

#[test]
fn lookup_reuses_only_mathematical_unique_answers_without_native_checks() {
    let observe = crate::query_pair::tests::Observe::new(None, None, None);
    let mut cache = DecidedCache::default();
    let x = Context::default().fresh_bv("x", 8);
    for (base, answer) in [
        (
            vec![x.eq(Bv::from_u64(42, 8))],
            UniqueOutcome::Unique("#x2a".into()),
        ),
        (vec![], UniqueOutcome::Multiple),
        (
            vec![x.eq(Bv::from_u64(1, 8)), x.eq(Bv::from_u64(2, 8))],
            UniqueOutcome::Infeasible,
        ),
    ] {
        remember(&mut cache, &base, &x, &answer);
        assert_eq!(
            probe(&mut cache, &base, &x, Provider::Z3, DEFAULT_RLIMIT, scope()),
            Ok(Some(answer))
        );
    }
    let unproved = [x.eq(Bv::from_u64(43, 8))];
    remember(
        &mut cache,
        &unproved,
        &x,
        &UniqueOutcome::Unknown(Unknown::ResourceLimit),
    );
    assert_eq!(
        probe(
            &mut cache,
            &unproved,
            &x,
            Provider::Z3,
            DEFAULT_RLIMIT,
            scope()
        ),
        Ok(None)
    );
    assert_eq!(cache.stats().unique_hits, 3);
    assert_eq!(observe.native_calls(), 0);
}

#[test]
fn cold_oversize_and_zero_limit_probes_never_solve_or_install_answers() {
    let observe = crate::query_pair::tests::Observe::new(None, None, None);
    let x = Context::default().fresh_bv("x", 8);
    let base = [x.eq(Bv::from_u64(42, 8))];
    let mut cache = DecidedCache::default();
    assert_eq!(
        probe(&mut cache, &base, &x, Provider::Z3, DEFAULT_RLIMIT, scope()),
        Ok(None)
    );
    assert_eq!(cache.stats().misses, 1);
    assert_eq!(
        probe(&mut cache, &base, &x, Provider::Z3, 0, scope()),
        Ok(None)
    );
    cache.limits = Limits {
        key_bytes: 8,
        ..Limits::default()
    };
    assert_eq!(
        probe(&mut cache, &base, &x, Provider::Z3, DEFAULT_RLIMIT, scope()),
        Ok(None)
    );
    assert_eq!(cache.stats().bypasses, 1);
    assert_eq!(cache.stats().insertions, 0);
    assert_eq!(cache.stats().undecided, 0);
    assert_eq!(observe.native_calls(), 0);
}

#[test]
fn lookup_requires_complete_single_formula_and_original_namespace() {
    let observe = crate::query_pair::tests::Observe::new(None, None, None);
    let mut cache = DecidedCache::default();
    let symbols = Context::default();
    let x = symbols.fresh_bv("x", 8);
    let y = symbols.fresh_bv("definition", 8);
    let base = [x.eq(&y), y.eq(Bv::from_u64(7, 8))];
    remember(&mut cache, &base, &x, &UniqueOutcome::Unique("#x07".into()));
    for (provider, rlimit, scope) in [
        (Provider::Bitwuzla, DEFAULT_RLIMIT, scope()),
        (Provider::Z3, DEFAULT_RLIMIT + 1, scope()),
        (
            Provider::Z3,
            DEFAULT_RLIMIT,
            CacheScope {
                depth: 257,
                ..scope()
            },
        ),
    ] {
        assert_eq!(
            probe(&mut cache, &base, &x, provider, rlimit, scope),
            Ok(None)
        );
    }
    assert_eq!(
        probe(
            &mut cache,
            &[x.eq(&y)],
            &x,
            Provider::Z3,
            DEFAULT_RLIMIT,
            scope()
        ),
        Ok(None)
    );
    assert_eq!(
        probe(&mut cache, &base, &y, Provider::Z3, DEFAULT_RLIMIT, scope()),
        Ok(None)
    );
    assert_eq!(cache.stats().unique_hits, 0);
    assert_eq!(observe.native_calls(), 0);
}

#[test]
fn optional_maintenance_refusal_preserves_work_and_cancellation_wins() {
    let observe = crate::query_pair::tests::Observe::new(None, None, None);
    let mut cache = DecidedCache::default();
    let x = Context::default().fresh_bv("x", 8);
    let base = [x.eq(Bv::from_u64(42, 8))];
    remember(&mut cache, &base, &x, &UniqueOutcome::Unique("#x2a".into()));
    let mut used = 37;
    {
        let mut query = CacheQuery::new(38, &mut used, current_cancellation(), &mut cache, scope())
            .with_allowance(0);
        assert_eq!(
            probe_unique_cached(&base, &x, Provider::Z3, DEFAULT_RLIMIT, &mut query),
            Ok(None)
        );
    }
    assert_eq!(used, 37);
    let token = Cancellation::new();
    token.cancel();
    with_cancellation(&token, || {
        assert_eq!(
            probe(&mut cache, &base, &x, Provider::Z3, DEFAULT_RLIMIT, scope()),
            Err(Unknown::Cancelled)
        );
    });
    assert_eq!(cache.stats().unique_hits, 0);
    assert_eq!(observe.native_calls(), 0);
}
