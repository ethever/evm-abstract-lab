use super::{CacheQuery, CacheScope, DecidedCache, Limits};
use crate::{
    Bool, Bv, Context, DEFAULT_RLIMIT, FunctionArg, FunctionSort, Outcome, Provider, UniqueOutcome,
    check, check_unique,
};

mod collisions;
mod feasibility;
mod resources;

fn scope() -> CacheScope {
    CacheScope {
        enabled: true,
        constraints: 2048,
        nodes: 16_384,
        depth: 256,
    }
}
fn check_unique_cached(
    base: &[Bool],
    value: &Bv,
    provider: Provider,
    rlimit: u32,
    scope: CacheScope,
    cache: &mut DecidedCache,
) -> UniqueOutcome {
    let mut used = 0;
    let mut query = CacheQuery::new(
        usize::MAX,
        &mut used,
        crate::current_cancellation(),
        cache,
        scope,
    );
    super::check_unique_cached(base, value, provider, rlimit, &mut query)
}
fn candidate_current(
    base: &[Bool],
    provider: Provider,
    rlimit: u32,
    scope: CacheScope,
    cache: &mut DecidedCache,
) -> Outcome {
    let mut used = 0;
    let mut query = CacheQuery::new(
        usize::MAX,
        &mut used,
        crate::current_cancellation(),
        cache,
        scope,
    );
    super::candidate_current(base, provider, rlimit, &mut query)
}
fn unique(cache: &mut DecidedCache, base: &[Bool], value: &Bv) -> UniqueOutcome {
    let expected = check_unique(base, value, Provider::Z3, DEFAULT_RLIMIT);
    let actual = check_unique_cached(base, value, Provider::Z3, DEFAULT_RLIMIT, scope(), cache);
    assert_eq!(actual, expected);
    actual
}
fn candidate(cache: &mut DecidedCache, base: &[Bool]) -> Outcome {
    let expected = check(base, None, Provider::Z3, DEFAULT_RLIMIT);
    let actual = candidate_current(base, Provider::Z3, DEFAULT_RLIMIT, scope(), cache);
    assert_eq!(actual, expected);
    actual
}
fn key(base: &[Bool], value: Option<&Bv>) -> Vec<u8> {
    let mut used = 0;
    let mut cache = DecidedCache::default();
    let mut query = CacheQuery::new(
        usize::MAX,
        &mut used,
        crate::current_cancellation(),
        &mut cache,
        scope(),
    );
    super::key::query(
        base,
        value,
        Provider::Z3,
        DEFAULT_RLIMIT,
        scope(),
        Limits::default(),
        &mut query.budget,
    )
    .unwrap()
}

#[test]
fn fresh_names_reuse_proved_unique_multiple_and_infeasible_answers() {
    let mut cache = DecidedCache::default();
    for _ in 0..2 {
        let x = Context::default().fresh_bv("fresh", 8);
        assert_eq!(
            unique(&mut cache, &[x.eq(Bv::from_u64(42, 8))], &x),
            UniqueOutcome::Unique("#x2a".into())
        );
        assert_eq!(
            unique(&mut cache, &[x.bvule(Bv::from_u64(7, 8))], &x),
            UniqueOutcome::Multiple
        );
        assert_eq!(
            unique(
                &mut cache,
                &[x.eq(Bv::from_u64(1, 8)), x.eq(Bv::from_u64(2, 8))],
                &x
            ),
            UniqueOutcome::Infeasible
        );
    }
    assert_eq!(cache.stats().entries, 3);
    assert_eq!(cache.stats().unique_hits, 3);
}

#[test]
fn alias_partition_is_shared_across_all_assertion_roots() {
    let mut cache = DecidedCache::default();
    let x = Context::default().fresh_bv("x", 8);
    let y = Context::default().fresh_bv("y", 8);
    let shared = [x.eq(Bv::from_u64(0, 8)), x.eq(Bv::from_u64(1, 8))];
    let separate = [x.eq(Bv::from_u64(0, 8)), y.eq(Bv::from_u64(1, 8))];
    assert_ne!(key(&shared, None), key(&separate, None));
    assert_eq!(candidate(&mut cache, &shared), Outcome::Unsat);
    assert_eq!(candidate(&mut cache, &separate), Outcome::Sat(None));
    assert_eq!(cache.stats().candidate_hits, 0);
    let renamed = Context::default().fresh_bv("z", 8);
    assert_eq!(
        candidate(
            &mut cache,
            &[
                renamed.eq(Bv::from_u64(0, 8)),
                renamed.eq(Bv::from_u64(1, 8))
            ]
        ),
        Outcome::Unsat
    );
    assert_eq!(cache.stats().candidate_hits, 1);
}

#[test]
fn model_target_joins_the_same_alias_scope_as_the_conjunction() {
    let mut cache = DecidedCache::default();
    let symbols = Context::default();
    let x = symbols.fresh_bv("x", 8);
    let y = symbols.fresh_bv("y", 8);
    let base = [x.eq(Bv::from_u64(7, 8))];
    assert_ne!(key(&base, Some(&x)), key(&base, Some(&y)));
    assert_eq!(
        unique(&mut cache, &base, &x),
        UniqueOutcome::Unique("#x07".into())
    );
    assert_eq!(unique(&mut cache, &base, &y), UniqueOutcome::Multiple);
    assert_eq!(cache.stats().unique_hits, 0);
}

#[test]
fn every_snapshot_fact_and_definition_is_part_of_the_key() {
    let mut cache = DecidedCache::default();
    let x = Context::default().fresh_bv("x", 8);
    let seven = [x.eq(Bv::from_u64(7, 8))];
    let eight = [x.eq(Bv::from_u64(8, 8))];
    assert_eq!(
        unique(&mut cache, &seven, &x),
        UniqueOutcome::Unique("#x07".into())
    );
    assert_eq!(
        unique(&mut cache, &eight, &x),
        UniqueOutcome::Unique("#x08".into())
    );
    assert_eq!(unique(&mut cache, &[], &x), UniqueOutcome::Multiple);
    let y = Context::default().fresh_bv("definition", 8);
    assert_eq!(
        unique(&mut cache, &[x.eq(&y), y.eq(Bv::from_u64(9, 8))], &x),
        UniqueOutcome::Unique("#x09".into())
    );
    assert_eq!(unique(&mut cache, &[x.eq(&y)], &x), UniqueOutcome::Multiple);
    assert_eq!(cache.stats().unique_hits, 0);
}

#[test]
fn function_declaration_partition_and_complete_signature_survive_renaming() {
    let mut cache = DecidedCache::default();
    for repeat in 0..2 {
        let symbols = Context::default();
        let f = symbols.fresh_function("f", &[FunctionSort::Bv(8)], 8);
        let g = symbols.fresh_function("f", &[FunctionSort::Bv(8)], 8);
        let zero = Bv::from_u64(0, 8);
        let shared = [
            f.apply(&[FunctionArg::Bv(&zero)]).eq(Bv::from_u64(0, 8)),
            f.clone()
                .apply(&[FunctionArg::Bv(&zero)])
                .eq(Bv::from_u64(1, 8)),
        ];
        let separate = [
            f.apply(&[FunctionArg::Bv(&zero)]).eq(Bv::from_u64(0, 8)),
            g.apply(&[FunctionArg::Bv(&zero)]).eq(Bv::from_u64(1, 8)),
        ];
        assert_ne!(key(&shared, None), key(&separate, None));
        assert_eq!(candidate(&mut cache, &shared), Outcome::Unsat);
        assert_eq!(candidate(&mut cache, &separate), Outcome::Sat(None));
        assert_eq!(cache.stats().candidate_hits, repeat * 2);
        let array = symbols.fresh_array("array", 8, 8);
        let h = symbols.fresh_function(
            "f",
            &[FunctionSort::Array {
                index_width: 8,
                element_width: 8,
            }],
            8,
        );
        assert_ne!(
            key(
                &[h.apply(&[FunctionArg::Array(&array)])
                    .eq(Bv::from_u64(0, 8))],
                None
            ),
            key(
                &[f.apply(&[FunctionArg::Bv(&zero)]).eq(Bv::from_u64(0, 8))],
                None
            )
        );
    }
}

#[test]
fn array_aliases_widths_snapshots_and_store_order_are_preserved() {
    let mut cache = DecidedCache::default();
    let index = Bv::from_u64(3, 8);
    let one = Bv::from_u64(1, 8);
    let two = Bv::from_u64(2, 8);
    let original = Context::default().fresh_array("a", 8, 8);
    let forward = original
        .store(&index, &one)
        .store(&index, &two)
        .select(&index);
    let reverse = original
        .store(&index, &two)
        .store(&index, &one)
        .select(&index);
    assert_ne!(key(&[], Some(&forward)), key(&[], Some(&reverse)));
    assert_eq!(
        unique(&mut cache, &[], &forward),
        UniqueOutcome::Unique("#x02".into())
    );
    assert_eq!(
        unique(&mut cache, &[], &reverse),
        UniqueOutcome::Unique("#x01".into())
    );
    let a = Context::default().fresh_array("a", 8, 8);
    let b = Context::default().fresh_array("b", 8, 8);
    assert_eq!(candidate(&mut cache, &[a.eq(&a).not()]), Outcome::Unsat);
    assert_eq!(candidate(&mut cache, &[a.eq(&b).not()]), Outcome::Sat(None));
    assert_ne!(
        key(&[], Some(&original.select(&index))),
        key(
            &[],
            Some(
                &Context::default()
                    .fresh_array("wide", 16, 8)
                    .select(&Bv::from_u64(3, 16))
            )
        )
    );
    assert_eq!(
        unique(&mut cache, &[], &original.select(&index)),
        UniqueOutcome::Multiple
    );
}

#[test]
fn names_are_renamed_by_sort_and_all_operator_parameters_are_kept() {
    use crate::term::{Node, Op, Sort, Term};
    use std::sync::Arc;
    let x8 = Bv(Term(Arc::new(Node {
        op: Op::Variable("same".into()),
        sort: Sort::Bv(8),
        args: vec![],
    })));
    let x16 = Bv(Term(Arc::new(Node {
        op: Op::Variable("same".into()),
        sort: Sort::Bv(16),
        args: vec![],
    })));
    let mut cache = DecidedCache::default();
    let assertions = [x8.eq(Bv::from_u64(1, 8)), x16.eq(Bv::from_u64(2, 16))];
    assert_eq!(candidate(&mut cache, &assertions), Outcome::Sat(None));
    assert_ne!(
        key(&[], Some(&x16.extract(7, 0))),
        key(&[], Some(&x16.extract(15, 8)))
    );
    assert_ne!(
        key(&[], Some(&x8.zero_ext(8))),
        key(&[], Some(&x8.zero_ext(16)))
    );
    assert_ne!(
        key(&[], Some(&x8.bvsub(Bv::from_u64(1, 8)))),
        key(&[], Some(&Bv::from_u64(1, 8).bvsub(&x8)))
    );
    assert_ne!(
        key(&[], Some(&x8.bvadd(Bv::from_u64(1, 8)))),
        key(&[], Some(&x8.bvsub(Bv::from_u64(1, 8))))
    );
    assert_ne!(
        key(&[Bool::and(&assertions)], None),
        key(&[Bool::or(&assertions)], None)
    );
}
