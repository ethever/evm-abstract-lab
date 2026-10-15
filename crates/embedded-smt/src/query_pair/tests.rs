use super::{
    BudgetedOutcome, UniqueOutcome, check_candidates, check_candidates_budgeted, check_unique,
};
use crate::{
    Array, Bool, Bv, Cancellation, Context, DEFAULT_RLIMIT, FunctionArg, FunctionSort, Outcome,
    Provider, QueryBudget, Unknown, check, with_cancellation,
};
use std::{
    sync::{Arc, Barrier, Mutex, mpsc},
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

mod literal_family;

// Record only events from the actual typed APIs; there is no second batch implementation.
#[derive(Clone, Default)]
struct Trace {
    contexts: Vec<usize>,
    queries: Vec<usize>,
    native: Vec<usize>,
    preparations: Vec<usize>,
    resources: Vec<(u32, Option<u32>, Option<u32>)>,
    finished: usize,
}
struct Watch {
    owner: ThreadId,
    trace: Arc<Mutex<Trace>>,
    first_context: Option<Arc<Barrier>>,
    first_native: Option<mpsc::Sender<()>>,
    cancel_after: Option<(usize, Cancellation)>,
}
static WATCHES: Mutex<Vec<Watch>> = Mutex::new(Vec::new());
pub(crate) struct Observe {
    owner: ThreadId,
    trace: Arc<Mutex<Trace>>,
}
impl Observe {
    pub(crate) fn new(
        first_context: Option<Arc<Barrier>>,
        first_native: Option<mpsc::Sender<()>>,
        cancel_after: Option<(usize, Cancellation)>,
    ) -> Self {
        let owner = thread::current().id();
        let trace = Arc::new(Mutex::new(Trace::default()));
        let mut watches = WATCHES.lock().unwrap();
        assert!(!watches.iter().any(|watch| watch.owner == owner));
        watches.push(Watch {
            owner,
            trace: trace.clone(),
            first_context,
            first_native,
            cancel_after,
        });
        Self { owner, trace }
    }
    fn trace(&self) -> Trace {
        self.trace.lock().unwrap().clone()
    }
    pub(crate) fn native_calls(&self) -> usize {
        self.trace.lock().unwrap().native.len()
    }
    pub(crate) fn native_contexts(&self) -> Vec<usize> {
        self.trace.lock().unwrap().native.clone()
    }
}
impl Drop for Observe {
    fn drop(&mut self) {
        WATCHES
            .lock()
            .unwrap()
            .retain(|watch| watch.owner != self.owner);
    }
}
fn context_id() -> usize {
    ::z3::Context::thread_local().get_z3_context().as_ptr() as usize
}
pub(super) fn observe_context() {
    let barrier = {
        let mut watches = WATCHES.lock().unwrap();
        watches
            .iter_mut()
            .find(|watch| watch.owner == thread::current().id())
            .and_then(|watch| {
                watch.trace.lock().unwrap().contexts.push(context_id());
                watch.first_context.take()
            })
    };
    if let Some(barrier) = barrier {
        barrier.wait();
    }
}
pub(super) fn observe_query() {
    if let Some(watch) = WATCHES
        .lock()
        .unwrap()
        .iter()
        .find(|watch| watch.owner == thread::current().id())
    {
        watch.trace.lock().unwrap().queries.push(context_id());
    }
}
pub(super) fn observe_preparation() {
    if let Some(watch) = WATCHES
        .lock()
        .unwrap()
        .iter()
        .find(|watch| watch.owner == thread::current().id())
    {
        watch.trace.lock().unwrap().preparations.push(context_id());
    }
}
pub(super) fn observe_resources(rlimit: u32, before: Option<u32>, after: Option<u32>) {
    if let Some(watch) = WATCHES
        .lock()
        .unwrap()
        .iter()
        .find(|watch| watch.owner == thread::current().id())
    {
        watch
            .trace
            .lock()
            .unwrap()
            .resources
            .push((rlimit, before, after));
    }
}
pub(super) fn observe_native() {
    let started = {
        let mut watches = WATCHES.lock().unwrap();
        watches
            .iter_mut()
            .find(|watch| watch.owner == thread::current().id())
            .and_then(|watch| {
                watch.trace.lock().unwrap().native.push(context_id());
                watch.first_native.take()
            })
    };
    if let Some(started) = started {
        started.send(()).unwrap();
    }
}
pub(super) fn observe_finished_query() {
    let cancellation = {
        let mut watches = WATCHES.lock().unwrap();
        watches
            .iter_mut()
            .find(|watch| watch.owner == thread::current().id())
            .and_then(|watch| {
                let mut trace = watch.trace.lock().unwrap();
                trace.finished += 1;
                if watch
                    .cancel_after
                    .as_ref()
                    .is_some_and(|(count, _)| *count == trace.finished)
                {
                    watch.cancel_after.take().map(|(_, token)| token)
                } else {
                    None
                }
            })
    };
    if let Some(cancellation) = cancellation {
        cancellation.cancel();
    }
}
fn parse_model(value: &str, width: u32) -> Bv {
    let number = if let Some(hex) = value.strip_prefix("#x") {
        u64::from_str_radix(hex, 16).unwrap()
    } else {
        u64::from_str_radix(value.strip_prefix("#b").unwrap(), 2).unwrap()
    };
    Bv::from_u64(number, width)
}
fn fresh_candidates(
    base: &[Bool],
    value: &Bv,
    candidates: &[Bv],
    provider: Provider,
    rlimit: u32,
) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    for candidate in candidates {
        let mut assertions = base.to_vec();
        assertions.push(value.eq(candidate));
        let outcome = check(&assertions, None, provider, rlimit);
        let stop = matches!(outcome, Outcome::Unknown(_) | Outcome::Error(_));
        outcomes.push(outcome);
        if stop {
            break;
        }
    }
    outcomes
}

#[test]
fn typed_unique_matches_fresh_unique_multiple_and_infeasible_oracles() {
    for provider in [Provider::Z3, Provider::Bitwuzla, Provider::Cvc5] {
        let x = Context::default().fresh_bv("x", 8);
        let base = [x.eq(Bv::from_u64(42, 8))];
        let UniqueOutcome::Unique(model) = check_unique(&base, &x, provider, DEFAULT_RLIMIT) else {
            panic!("not unique: {provider}");
        };
        assert_eq!(
            check(&base, Some(&x), provider, DEFAULT_RLIMIT),
            Outcome::Sat(Some(model.clone()))
        );
        assert_eq!(
            parse_model(&model, 8).literal_hex(),
            Bv::from_u64(42, 8).literal_hex()
        );
        assert_eq!(
            check(
                &[base[0].clone(), x.eq(parse_model(&model, 8)).not()],
                None,
                provider,
                DEFAULT_RLIMIT
            ),
            Outcome::Unsat
        );
        assert_eq!(
            check_unique(&[x.bvule(Bv::from_u64(7, 8))], &x, provider, DEFAULT_RLIMIT),
            UniqueOutcome::Multiple
        );
        assert_eq!(
            check_unique(
                &[x.eq(Bv::from_u64(1, 8)), x.eq(Bv::from_u64(2, 8))],
                &x,
                provider,
                DEFAULT_RLIMIT
            ),
            UniqueOutcome::Infeasible
        );
    }
}

#[test]
fn candidates_use_independent_solvers_and_original_rlimit_on_every_query() {
    let context = Context::default();
    let x = context.fresh_bv("x", 8);
    let rlimit = 100;
    let mut candidates = (0..31).map(|n| Bv::from_u64(n, 8)).collect::<Vec<_>>();
    // One symbolic candidate keeps the entire chunk on the fresh-Solver path.
    candidates.push(context.fresh_bv("candidate", 8));
    for allowance in [rlimit, DEFAULT_RLIMIT, rlimit] {
        let observe = Observe::new(None, None, None);
        assert_eq!(
            check_candidates(&[], &x, &candidates, Provider::Z3, allowance),
            vec![Outcome::Sat(None); 32]
        );
        let trace = observe.trace();
        assert_eq!(trace.contexts.len(), 1);
        assert_eq!(trace.queries, vec![trace.contexts[0]; 32]);
        assert_eq!(trace.native, trace.queries);
        assert_eq!(trace.preparations, trace.queries);
        assert_eq!(trace.resources.len(), 32);
        let used = trace
            .resources
            .iter()
            .map(|(limit, before, after)| {
                assert_eq!(*limit, allowance);
                let delta = after
                    .expect("native resource counter")
                    .checked_sub(before.expect("native resource counter"))
                    .expect("monotonic context resource counter");
                assert!(delta <= allowance);
                u64::from(delta)
            })
            .sum::<u64>();
        assert!(
            used > u64::from(rlimit),
            "fixture must exercise cumulative work beyond one low allowance: {used}"
        );
    }

    let square = x.bvmul(&x);
    let candidates = [Bv::from_u64(4, 8), Bv::from_u64(9, 8)];
    {
        let observe = Observe::new(None, None, None);
        assert_eq!(
            check_candidates(&[], &square, &candidates, Provider::Z3, 1),
            [Outcome::Unknown(Unknown::ResourceLimit)]
        );
        let trace = observe.trace();
        assert_eq!(trace.contexts.len(), 1);
        assert_eq!(trace.queries, trace.contexts);
        assert_eq!(trace.preparations, trace.queries);
        assert_eq!(trace.native, trace.queries);
        assert_eq!(trace.resources.len(), 1);
        let (limit, before, after) = trace.resources[0];
        assert_eq!(limit, 1);
        let used = after
            .expect("native resource counter")
            .checked_sub(before.expect("native resource counter"))
            .expect("monotonic context resource counter");
        assert!(used >= limit);
    }
    assert_eq!(
        check_candidates(&[], &x, &candidates, Provider::Z3, DEFAULT_RLIMIT),
        [Outcome::Sat(None), Outcome::Sat(None)]
    );

    // Near a resource cutoff, separate Contexts need not follow the same search
    // trajectory. At the full allowance, require exact mathematical answers.
    for (numbers, expected) in [
        ([4, 9], [Outcome::Sat(None), Outcome::Sat(None)]),
        ([3, 4], [Outcome::Unsat, Outcome::Sat(None)]),
    ] {
        let candidates = numbers.map(|n| Bv::from_u64(n, 8));
        let fresh = fresh_candidates(&[], &square, &candidates, Provider::Z3, DEFAULT_RLIMIT);
        assert_eq!(fresh, expected);
        assert_eq!(
            check_candidates(&[], &square, &candidates, Provider::Z3, DEFAULT_RLIMIT),
            fresh
        );
    }
    assert_eq!(
        check_unique(&[square.eq(Bv::from_u64(4, 8))], &x, Provider::Z3, 1),
        UniqueOutcome::Unknown(Unknown::ResourceLimit)
    );
    assert_eq!(
        check_unique(
            &[x.eq(Bv::from_u64(42, 8))],
            &x,
            Provider::Z3,
            DEFAULT_RLIMIT
        ),
        UniqueOutcome::Unique("#x2a".into())
    );
}

#[test]
fn complete_512_candidates_match_fresh_oracle_with_at_most_32_checks_per_context() {
    let x = Context::default().fresh_bv("batch", 16);
    let base = [Bool::or(&[
        x.eq(Bv::from_u64(5, 16)),
        x.eq(Bv::from_u64(9, 16)),
    ])];
    let candidates = (0..512)
        .map(|candidate| Bv::from_u64(candidate, 16))
        .collect::<Vec<_>>();
    let fresh = fresh_candidates(&base, &x, &candidates, Provider::Z3, DEFAULT_RLIMIT);
    let observe = Observe::new(None, None, None);
    let batched = check_candidates(&base, &x, &candidates, Provider::Z3, DEFAULT_RLIMIT);
    assert_eq!(batched, fresh);
    assert_eq!(
        batched
            .iter()
            .filter(|outcome| **outcome == Outcome::Sat(None))
            .count(),
        2
    );
    let trace = observe.trace();
    assert_eq!(trace.contexts.len(), 16);
    assert_eq!(trace.queries.len(), 512);
    assert_eq!(trace.native.len(), 512);
    assert_eq!(trace.preparations.len(), 16);
    for (context, queries) in trace.contexts.iter().zip(trace.queries.chunks(32)) {
        assert!(queries.iter().all(|query| query == context));
    }
}

#[test]
fn actual_typed_apis_restore_caller_context_configuration_and_assertions() {
    let mut config = ::z3::Config::new();
    config.set_param_value("rlimit", "1");
    ::z3::with_z3_config(&config, || {
        let caller = ::z3::Context::thread_local();
        let caller_id = caller.get_z3_context().as_ptr() as usize;
        let solver = ::z3::Solver::new_for_logic("QF_ABV").unwrap();
        let native = ::z3::ast::BV::new_const("outside", 8);
        solver.assert(native.bvmul(&native).eq(::z3::ast::BV::from_u64(4, 8)));
        let x = Context::default().fresh_bv("inside", 8);
        let observe = Observe::new(None, None, None);
        assert_eq!(
            check_unique(
                &[x.eq(Bv::from_u64(42, 8))],
                &x,
                Provider::Z3,
                DEFAULT_RLIMIT
            ),
            UniqueOutcome::Unique("#x2a".into())
        );
        assert_eq!(
            check_candidates(
                &[],
                &x,
                &[Bv::from_u64(42, 8), Bv::from_u64(43, 8)],
                Provider::Z3,
                DEFAULT_RLIMIT
            ),
            [Outcome::Sat(None), Outcome::Sat(None)]
        );
        assert_eq!(observe.trace().contexts.len(), 2);
        assert!(
            observe
                .trace()
                .contexts
                .iter()
                .all(|context| *context != caller_id)
        );
        assert_eq!(::z3::Context::thread_local(), caller);
        assert_eq!(solver.get_assertions().len(), 1);
        assert_eq!(solver.check(), ::z3::SatResult::Unknown);
    });
}

#[test]
fn actual_typed_apis_own_distinct_live_contexts_across_eight_threads() {
    let barrier = Arc::new(Barrier::new(8));
    thread::scope(|scope| {
        let (finished, results) = mpsc::channel();
        for index in 0..8 {
            let barrier = barrier.clone();
            let finished = finished.clone();
            scope.spawn(move || {
                let observe = Observe::new(Some(barrier), None, None);
                let x = Context::default().fresh_bv("same_prefix", 8);
                let UniqueOutcome::Unique(model) = check_unique(
                    &[x.eq(Bv::from_u64(index, 8))],
                    &x,
                    Provider::Z3,
                    DEFAULT_RLIMIT,
                ) else {
                    panic!("not unique");
                };
                assert_eq!(
                    parse_model(&model, 8).literal_hex(),
                    Bv::from_u64(index, 8).literal_hex()
                );
                assert_eq!(
                    check_candidates(
                        &[],
                        &x,
                        &[Bv::from_u64(index, 8), Bv::from_u64(index + 16, 8)],
                        Provider::Z3,
                        DEFAULT_RLIMIT
                    ),
                    [Outcome::Sat(None), Outcome::Sat(None)]
                );
                let trace = observe.trace();
                assert_eq!(trace.contexts.len(), 2);
                assert_eq!(trace.native.len(), 4);
                finished.send(trace.contexts[0]).unwrap();
            });
        }
        let mut identities = (0..8)
            .map(|_| results.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect::<Vec<_>>();
        identities.sort_unstable();
        identities.dedup();
        assert_eq!(identities.len(), 8);
    });
}

#[test]
fn actual_typed_apis_keep_array_and_function_congruence_semantics() {
    let context = Context::default();
    let array = context.fresh_array("a", 8, 8);
    let i = context.fresh_bv("i", 8);
    let selected = array.select(&i);
    assert_eq!(
        check_candidates(
            &[selected.eq(Bv::from_u64(5, 8))],
            &selected,
            &[Bv::from_u64(5, 8), Bv::from_u64(9, 8)],
            Provider::Z3,
            DEFAULT_RLIMIT
        ),
        [Outcome::Sat(None), Outcome::Unsat]
    );
    let empty = Array::constant(8, &Bv::from_u64(0, 8));
    assert_eq!(
        check_candidates(
            &[],
            &empty.select(&Bv::from_u64(0, 8)),
            &[Bv::from_u64(0, 8), Bv::from_u64(1, 8)],
            Provider::Z3,
            DEFAULT_RLIMIT
        ),
        [Outcome::Sat(None), Outcome::Unsat]
    );
    let function = context.fresh_function(
        "digest",
        &[FunctionSort::Array {
            index_width: 8,
            element_width: 8,
        }],
        8,
    );
    let other = context.fresh_array("b", 8, 8);
    let fa = function.apply(&[FunctionArg::Array(&array)]);
    let fb = function.apply(&[FunctionArg::Array(&other)]);
    let base = [array.eq(&other), fa.eq(Bv::from_u64(42, 8))];
    assert_eq!(
        check_candidates(
            &base,
            &fb,
            &[Bv::from_u64(42, 8), Bv::from_u64(43, 8)],
            Provider::Z3,
            DEFAULT_RLIMIT
        ),
        [Outcome::Sat(None), Outcome::Unsat]
    );
    assert_eq!(
        check_unique(&base, &fb, Provider::Z3, DEFAULT_RLIMIT),
        UniqueOutcome::Unique("#x2a".into())
    );
}

#[test]
fn actual_native_cancellation_stops_batch_and_same_worker_next_batch_is_clean() {
    for count in [2, 3] {
        let candidates = (1..=count)
            .map(|n| Bv::from_u64(n, 256))
            .collect::<Vec<_>>();
        let token = Cancellation::new();
        let (started, native_started) = mpsc::channel();
        let observe = Observe::new(None, Some(started), None);
        thread::scope(|scope| {
            let context = Context::default();
            let target = Bv::from_str(
                256,
                "115792089237316195423570985008687907853269984665640564039457584007908834671663",
            )
            .unwrap();
            let mut base = Vec::new();
            for index in 0..8 {
                let x = context.fresh_bv(&format!("x{index}"), 256);
                let y = context.fresh_bv(&format!("y{index}"), 256);
                base.extend([
                    x.bvmul(&y).eq(&target),
                    x.bvugt(Bv::from_u64(1, 256)),
                    y.bvugt(Bv::from_u64(1, 256)),
                ]);
            }
            let value = context.fresh_bv("candidate", 256);
            // Fixture construction can be descheduled by other parallel checks.
            // Start the native-call watchdog only after that setup is complete;
            // the query and cancellation still have the same five-second bound.
            let cancellation = token.clone();
            scope.spawn(move || {
                native_started.recv_timeout(Duration::from_secs(5)).unwrap();
                thread::sleep(Duration::from_millis(30));
                cancellation.cancel();
            });
            let start = Instant::now();
            assert_eq!(
                with_cancellation(&token, || check_candidates(
                    &base,
                    &value,
                    &candidates,
                    Provider::Z3,
                    u32::MAX
                )),
                [Outcome::Unknown(Unknown::Cancelled)]
            );
            assert!(start.elapsed() < Duration::from_secs(5));
            assert_eq!(observe.trace().native.len(), 1);
            assert_eq!(observe.trace().queries.len(), 1);
            assert_eq!(
                check_candidates(
                    &[],
                    &value,
                    &[Bv::from_u64(1, 256), Bv::from_u64(2, 256)],
                    Provider::Z3,
                    DEFAULT_RLIMIT
                ),
                [Outcome::Sat(None), Outcome::Sat(None)]
            );
            assert_eq!(observe.trace().contexts.len(), 2);
            assert_eq!(observe.trace().native.len(), 3);
            assert_eq!(observe.trace().preparations.len(), 3);
        });
    }
}

#[test]
fn typed_batches_stop_on_first_error_or_unknown_and_do_not_query_empty_inputs() {
    let x = Context::default().fresh_bv("limited", 256);
    assert_eq!(
        check_candidates(
            &[],
            &x.bvmul(&x),
            &[Bv::from_u64(4, 256), Bv::from_u64(9, 256)],
            Provider::Z3,
            1
        ),
        [Outcome::Unknown(Unknown::ResourceLimit)]
    );
    assert!(matches!(
        check_candidates(
            &[],
            &x,
            &[Bv::from_u64(1, 256), Bv::from_u64(2, 256)],
            Provider::Z3,
            0
        )
        .as_slice(),
        [Outcome::Error(_)]
    ));
    assert!(matches!(
        check_candidates(
            &[],
            &x,
            &[Bv::from_u64(1, 8), Bv::from_u64(2, 256)],
            Provider::Z3,
            DEFAULT_RLIMIT
        )
        .as_slice(),
        [Outcome::Error(_)]
    ));
    let token = Cancellation::new();
    token.cancel();
    assert_eq!(
        with_cancellation(&token, || check_candidates(
            &[],
            &x,
            &[Bv::from_u64(1, 256), Bv::from_u64(2, 256)],
            Provider::Z3,
            DEFAULT_RLIMIT
        )),
        [Outcome::Unknown(Unknown::Cancelled)]
    );
    assert_eq!(
        check_candidates(
            &[],
            &x,
            &[Bv::from_u64(1, 256)],
            Provider::Z3,
            DEFAULT_RLIMIT
        ),
        [Outcome::Sat(None)]
    );
    let observe = Observe::new(None, None, None);
    assert_eq!(
        check_candidates(&[], &x, &[], Provider::Z3, DEFAULT_RLIMIT),
        []
    );
    assert!(observe.trace().contexts.is_empty());
    assert!(observe.trace().native.is_empty());
}

#[test]
fn typed_lease_preserves_per_query_cost_and_first_work_or_unknown_order() {
    let x = Context::default().fresh_bv("paid", 256);
    let candidates = [
        Bv::from_u64(4, 256),
        Bv::from_u64(9, 256),
        Bv::from_u64(16, 256),
    ];
    for maximum in [0, 7, 14, 21] {
        let (mut used, mut exhausted) = (0, false);
        let mut lease = QueryBudget::new(
            maximum,
            &mut used,
            &mut exhausted,
            Cancellation::default(),
            7,
        );
        let results = check_candidates_budgeted(
            &[],
            &x,
            &candidates,
            &[],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut lease,
        );
        let permitted = (maximum / 7).min(3);
        let mut expected = vec![BudgetedOutcome::Solver(Outcome::Sat(None)); permitted];
        if permitted < 3 {
            expected.push(BudgetedOutcome::Work);
        }
        assert_eq!(results, expected);
        assert_eq!(used, permitted * 7);
        assert_eq!(exhausted, permitted < 3);
    }
    let (mut used, mut exhausted) = (0, false);
    let mut lease = QueryBudget::new(7, &mut used, &mut exhausted, Cancellation::default(), 7);
    assert_eq!(
        check_candidates_budgeted(
            &[],
            &x.bvmul(&x),
            &candidates,
            &[],
            Provider::Z3,
            1,
            &mut lease
        ),
        [BudgetedOutcome::Solver(Outcome::Unknown(
            Unknown::ResourceLimit
        ))]
    );
    assert_eq!(used, 7);
    assert!(!exhausted);
    let (mut used, mut exhausted) = (0, false);
    let mut lease = QueryBudget::new(0, &mut used, &mut exhausted, Cancellation::default(), 7);
    assert_eq!(
        check_candidates_budgeted(
            &[],
            &x.bvmul(&x),
            &candidates,
            &[],
            Provider::Z3,
            1,
            &mut lease
        ),
        [BudgetedOutcome::Work]
    );
    assert_eq!(used, 0);
    assert!(exhausted);
}

#[test]
fn reserved_encoder_permit_is_not_double_charged_or_reclassified_after_cancellation() {
    let x = Context::default().fresh_bv("reserve", 8);
    let (mut used, mut exhausted) = (0, false);
    let mut lease = QueryBudget::new(14, &mut used, &mut exhausted, Cancellation::default(), 7);
    assert!(lease.reserve());
    assert!(lease.reserve());
    assert_eq!(
        check_candidates_budgeted(
            &[],
            &x,
            &[Bv::from_u64(1, 8), Bv::from_u64(2, 8)],
            &[],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut lease
        ),
        [
            BudgetedOutcome::Solver(Outcome::Sat(None)),
            BudgetedOutcome::Solver(Outcome::Sat(None))
        ]
    );
    assert_eq!(used, 14);
    assert!(!exhausted);
    let token = Cancellation::new();
    let (mut used, mut exhausted) = (0, false);
    let mut lease = QueryBudget::new(100, &mut used, &mut exhausted, token.clone(), 7);
    assert!(lease.reserve());
    token.cancel();
    assert_eq!(
        with_cancellation(&token, || check_candidates_budgeted(
            &[],
            &x,
            &[Bv::from_u64(1, 8), Bv::from_u64(2, 8)],
            &[],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut lease
        )),
        [BudgetedOutcome::Solver(Outcome::Unknown(
            Unknown::Cancelled
        ))]
    );
    assert_eq!(used, 7);
    assert!(!exhausted);
    let (mut used, mut exhausted) = (3, false);
    let mut lease = QueryBudget::new(100, &mut used, &mut exhausted, token, 7);
    assert_eq!(
        check_candidates_budgeted(
            &[],
            &x,
            &[Bv::from_u64(1, 8)],
            &[],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut lease
        ),
        [BudgetedOutcome::Work]
    );
    assert_eq!(used, 3);
    assert!(exhausted);
    let (mut used, mut exhausted) = (0, false);
    let mut lease = QueryBudget::new(0, &mut used, &mut exhausted, Cancellation::default(), 7);
    assert_eq!(
        check_candidates_budgeted(&[], &x, &[], &[], Provider::Z3, DEFAULT_RLIMIT, &mut lease),
        []
    );
    assert_eq!(used, 0);
    assert!(!exhausted);
}

#[test]
fn every_budgeted_query_keeps_the_complete_definition_suffix() {
    let value = Context::default().fresh_bv("defined", 8);
    let after = [value.eq(Bv::from_u64(42, 8))];
    let candidates = [
        Bv::from_u64(41, 8),
        Bv::from_u64(42, 8),
        Bv::from_u64(43, 8),
    ];
    let (mut used, mut exhausted) = (0, false);
    let mut lease = QueryBudget::new(21, &mut used, &mut exhausted, Cancellation::default(), 7);
    assert_eq!(
        check_candidates_budgeted(
            &[],
            &value,
            &candidates,
            &after,
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut lease
        ),
        [
            BudgetedOutcome::Solver(Outcome::Unsat),
            BudgetedOutcome::Solver(Outcome::Sat(None)),
            BudgetedOutcome::Solver(Outcome::Unsat)
        ]
    );
    assert_eq!(used, 21);
    assert!(!exhausted);
}

#[test]
fn cancellation_between_context_chunks_refuses_the_next_unpaid_operation_as_work() {
    let token = Cancellation::new();
    let observe = Observe::new(None, None, Some((32, token.clone())));
    let x = Context::default().fresh_bv("boundary", 8);
    let candidates = (0..33)
        .map(|value| Bv::from_u64(value, 8))
        .collect::<Vec<_>>();
    let (mut used, mut exhausted) = (0, false);
    let mut lease = QueryBudget::new(33 * 7, &mut used, &mut exhausted, token.clone(), 7);
    let actual = with_cancellation(&token, || {
        check_candidates_budgeted(
            &[],
            &x,
            &candidates,
            &[],
            Provider::Z3,
            DEFAULT_RLIMIT,
            &mut lease,
        )
    });
    let mut expected = vec![BudgetedOutcome::Solver(Outcome::Sat(None)); 32];
    expected.push(BudgetedOutcome::Work);
    assert_eq!(actual, expected);
    assert_eq!(used, 32 * 7);
    assert!(exhausted);
    assert_eq!(observe.trace().contexts.len(), 1);
    assert_eq!(observe.trace().native.len(), 32);
    assert_eq!(observe.trace().preparations.len(), 1);
}
