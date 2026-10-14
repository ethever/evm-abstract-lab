use super::{
    Bool, BudgetedUniqueOutcome, Bv, Context, DEFAULT_RLIMIT, Observe, Provider, QueryManyError,
    UniqueOutcome, check_unique, check_unique_many, check_unique_many_budgeted,
};
use crate::{
    CacheScope, Cancellation, DecidedCache, Error, QueryBudget, Unknown, with_cancellation,
};

#[test]
fn work_refusals_charge_only_permitted_queries_and_keep_only_actual_proofs() {
    use BudgetedUniqueOutcome::{Solver, Work};
    let context = Context::default();
    let x = context.fresh_bv("unique", 8);
    let targets = [x.clone(), context.fresh_bv("multiple", 8)];
    let base = [x.eq(Bv::from_u64(42, 8))];
    for maximum in [0, 7, 14, 21, 28] {
        let observe = Observe::new(None, None, None);
        let (mut used, mut exhausted) = (0, false);
        let actual = {
            let mut budget =
                QueryBudget::new(maximum, &mut used, &mut exhausted, Cancellation::new(), 7);
            check_unique_many_budgeted(&base, &targets, Provider::Z3, DEFAULT_RLIMIT, &mut budget)
                .unwrap()
        };
        let expected = match maximum {
            0 | 7 | 14 => vec![Work; 2],
            21 => vec![Work, Solver(UniqueOutcome::Multiple)],
            _ => vec![
                Solver(UniqueOutcome::Unique("#x2a".into())),
                Solver(UniqueOutcome::Multiple),
            ],
        };
        assert_eq!(actual, expected);
        assert_eq!(used, maximum);
        assert_eq!(exhausted, maximum < 28);
        assert_eq!(observe.native_calls(), maximum / 7);
        assert!(
            observe
                .native_contexts()
                .windows(2)
                .all(|contexts| contexts[0] == contexts[1])
        );
    }
    let observe = Observe::new(None, None, None);
    let (mut used, mut exhausted) = (0, false);
    let mut budget = QueryBudget::new(0, &mut used, &mut exhausted, Cancellation::new(), 7);
    let impossible_to_pack = [context.fresh_bv("huge", u32::MAX), x];
    // Work is refused before even the width-dependent packing preparation.
    assert_eq!(
        check_unique_many_budgeted(
            &[],
            &impossible_to_pack,
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut budget
        )
        .unwrap(),
        vec![Work; 2]
    );
    assert_eq!(used, 0);
    assert!(exhausted);
    assert_eq!(observe.native_calls(), 0);
}

#[test]
fn unsat_resource_limit_and_errors_do_not_refund_a_reserved_original_cost() {
    use BudgetedUniqueOutcome::Solver;
    let context = Context::default();
    let x = context.fresh_bv("resource", 8);
    let targets = [x.clone(), context.fresh_bv("other", 8)];
    let nonlinear = [x.bvmul(&x).eq(Bv::from_u64(4, 8))];
    assert_eq!(
        check_unique(&nonlinear, &x, Provider::Z3, 1),
        UniqueOutcome::Unknown(Unknown::ResourceLimit)
    );
    for (base, rlimit, expected, native) in [
        (
            &[Bool::from_bool(false)][..],
            DEFAULT_RLIMIT,
            UniqueOutcome::Infeasible,
            1,
        ),
        (
            &nonlinear[..],
            1,
            UniqueOutcome::Unknown(Unknown::ResourceLimit),
            1,
        ),
        (
            &[][..],
            0,
            UniqueOutcome::Error(Error {
                provider: Provider::Z3,
                message: "rlimit must be positive".into(),
            }),
            0,
        ),
    ] {
        let observe = Observe::new(None, None, None);
        let (mut used, mut exhausted) = (3, false);
        let mut budget = QueryBudget::new(100, &mut used, &mut exhausted, Cancellation::new(), 7);
        assert!(budget.reserve());
        assert!(budget.reserve());
        assert_eq!(
            check_unique_many_budgeted(base, &targets, Provider::Z3, rlimit, &mut budget).unwrap(),
            vec![Solver(expected); 2]
        );
        assert_eq!(used, 10);
        assert!(!exhausted);
        assert_eq!(observe.native_calls(), native);
    }
    let observe = Observe::new(None, None, None);
    let (mut used, mut exhausted) = (0, false);
    let mut budget = QueryBudget::new(14, &mut used, &mut exhausted, Cancellation::new(), 7);
    let targets = [context.fresh_bv("wide", 4096), context.fresh_bv("one", 1)];
    assert!(budget.reserve());
    assert_eq!(
        check_unique_many_budgeted(&[], &targets, Provider::Z3, DEFAULT_RLIMIT, &mut budget),
        Err(QueryManyError::PackedWidthLimit { actual: 4097 })
    );
    // Width failure used that reservation, so it cannot subsidize a later query.
    let target = [context.fresh_bv("next", 8)];
    assert_eq!(
        check_unique_many_budgeted(&[], &target, Provider::Z3, DEFAULT_RLIMIT, &mut budget)
            .unwrap(),
        [BudgetedUniqueOutcome::Work]
    );
    assert_eq!(used, 14);
    assert!(exhausted);
    assert_eq!(observe.native_calls(), 1);
}

#[test]
fn empty_targets_and_attached_caches_do_not_invent_charges_or_reuse_sat_models() {
    let context = Context::default();
    let x = context.fresh_bv("cached", 8);
    let y = context.fresh_bv("cached", 8);
    let targets = [x.clone(), y.clone()];
    let base = [x.eq(Bv::from_u64(42, 8)), y.eq(Bv::from_u64(7, 8))];
    let observe = Observe::new(None, None, None);
    let (mut used, mut exhausted) = (0, false);
    {
        let mut budget = QueryBudget::new(0, &mut used, &mut exhausted, Cancellation::new(), 7);
        assert_eq!(
            check_unique_many_budgeted(&base, &[], Provider::Z3, 0, &mut budget).unwrap(),
            []
        );
    }
    assert_eq!(used, 0);
    assert!(!exhausted);
    let mut cache = DecidedCache::default();
    let before = cache.stats();
    {
        let mut budget = QueryBudget::new(42, &mut used, &mut exhausted, Cancellation::new(), 7)
            .with_cache(
                &mut cache,
                CacheScope {
                    enabled: true,
                    constraints: 128,
                    nodes: 1024,
                    depth: 32,
                },
            );
        for _ in 0..2 {
            let actual = check_unique_many_budgeted(
                &base,
                &targets,
                Provider::Z3,
                DEFAULT_RLIMIT,
                &mut budget,
            )
            .unwrap();
            assert!(actual.iter().all(|outcome| matches!(
                outcome,
                BudgetedUniqueOutcome::Solver(UniqueOutcome::Unique(_))
            )));
        }
    }
    assert_eq!(used, 42);
    assert!(!exhausted);
    assert_eq!(observe.native_calls(), 6);
    assert_eq!(cache.stats(), before);
}

#[test]
fn current_and_captured_cancellation_keep_the_existing_paid_and_unpaid_semantics() {
    use BudgetedUniqueOutcome::{Solver, Work};
    let target = [Context::default().fresh_bv("cancel", 8)];
    let token = Cancellation::new();
    token.cancel();
    let observe = Observe::new(None, None, None);
    let (mut used, mut exhausted) = (0, false);
    {
        let mut budget = QueryBudget::new(100, &mut used, &mut exhausted, Cancellation::new(), 7);
        assert_eq!(
            with_cancellation(&token, || check_unique_many_budgeted(
                &[],
                &target,
                Provider::Z3,
                0,
                &mut budget
            ))
            .unwrap(),
            [Solver(UniqueOutcome::Unknown(Unknown::Cancelled))]
        );
    }
    assert_eq!(used, 7);
    assert!(!exhausted);
    assert_eq!(
        with_cancellation(&token, || check_unique_many(
            &[],
            &target,
            Provider::Z3,
            DEFAULT_RLIMIT
        ))
        .unwrap(),
        [UniqueOutcome::Unknown(Unknown::Cancelled)]
    );
    let (mut used, mut exhausted) = (0, false);
    {
        let mut budget = QueryBudget::new(100, &mut used, &mut exhausted, token, 7);
        assert_eq!(
            check_unique_many_budgeted(&[], &target, Provider::Z3, DEFAULT_RLIMIT, &mut budget)
                .unwrap(),
            [Work]
        );
    }
    assert_eq!(used, 0);
    assert!(exhausted);
    assert_eq!(observe.native_calls(), 0);
    for scope_current in [false, true] {
        let token = Cancellation::new();
        let (mut used, mut exhausted) = (0, false);
        let mut budget = QueryBudget::new(100, &mut used, &mut exhausted, token.clone(), 7);
        assert!(budget.reserve());
        token.cancel();
        let mut run = || {
            check_unique_many_budgeted(&[], &target, Provider::Z3, DEFAULT_RLIMIT, &mut budget)
                .unwrap()
        };
        let actual = if scope_current {
            with_cancellation(&token, run)
        } else {
            run()
        };
        assert_eq!(
            actual,
            if scope_current {
                vec![Solver(UniqueOutcome::Unknown(Unknown::Cancelled))]
            } else {
                vec![Work]
            }
        );
        assert_eq!(used, 7);
        assert_eq!(exhausted, !scope_current);
    }
}

#[test]
fn cancellation_after_a_partial_proof_stops_and_a_fresh_batch_is_clean() {
    use BudgetedUniqueOutcome::{Solver, Work};
    let context = Context::default();
    let x = context.fresh_bv("unique", 8);
    let targets = [x.clone(), context.fresh_bv("multiple", 8)];
    let base = [x.eq(Bv::from_u64(42, 8))];
    for budgeted in [false, true] {
        let token = Cancellation::new();
        let observe = Observe::new(None, None, Some((3, token.clone())));
        let (mut used, mut exhausted) = (0, false);
        let actual = with_cancellation(&token, || {
            if budgeted {
                let mut budget = QueryBudget::new(100, &mut used, &mut exhausted, token.clone(), 7);
                check_unique_many_budgeted(
                    &base,
                    &targets,
                    Provider::Z3,
                    DEFAULT_RLIMIT,
                    &mut budget,
                )
                .unwrap()
            } else {
                check_unique_many(&base, &targets, Provider::Z3, DEFAULT_RLIMIT)
                    .unwrap()
                    .into_iter()
                    .map(Solver)
                    .collect()
            }
        });
        assert_eq!(
            actual,
            [
                if budgeted {
                    Work
                } else {
                    Solver(UniqueOutcome::Unknown(Unknown::Cancelled))
                },
                Solver(UniqueOutcome::Multiple),
            ]
        );
        assert_eq!(observe.native_calls(), 3);
        assert_eq!(used, if budgeted { 21 } else { 0 });
        assert_eq!(exhausted, budgeted);
        drop(observe);
        assert_eq!(
            check_unique_many(&base, &targets[..1], Provider::Z3, DEFAULT_RLIMIT).unwrap(),
            [UniqueOutcome::Unique("#x2a".into())]
        );
    }
}
