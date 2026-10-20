use super::{
    bounds::BoundsRule,
    candidates::CandidateRule,
    contracts::{Bounds, Candidates, IntersectCandidates, IntersectIntervals, Interval, Variable},
    support::{Conflict, Ledger, RuleError, Stop, scope},
};
use analysis_kernel::{
    Accepts, Closure, Cons, Context, Descending, Either, Here, Kernel, LocalClosure, Nil, Pair,
    PublishError, Read, Reduction, Registered, Scope, Scoped, There, solve,
};
use std::collections::BTreeSet;

struct Seed;
impl<Cx> Reduction<Cx> for Seed
where
    Cx: Accepts<Bounds, Here, Error = PublishError<Conflict, Stop>>
        + Accepts<Candidates, There<Here>, Error = PublishError<Conflict, Stop>>,
{
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, scope: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        // Registered initialization asserts the fixed input universe. It is not
        // permission for normal analyzers to write the first descending value.
        <Cx as Accepts<Bounds>>::accept(
            cx,
            Scoped::new(*scope, Variable::X),
            Interval::new(0, 10),
        )?;
        <Cx as Accepts<Bounds>>::accept(cx, Scoped::new(*scope, Variable::Y), Interval::new(4, 6))?;
        <Cx as Accepts<Candidates, There<Here>>>::accept(
            cx,
            Scoped::new(*scope, Variable::X),
            0b10_1010_1010,
        )?;
        <Cx as Accepts<Candidates, There<Here>>>::accept(
            cx,
            Scoped::new(*scope, Variable::Y),
            0x7ff,
        )?;
        Ok(LocalClosure::Stable)
    }
}

struct ResultBounds {
    x: Interval,
    y: Interval,
    cx: u16,
    cy: u16,
    evaluations: (usize, usize),
}
fn run(import_candidates: bool, import_bounds: bool, reverse: bool) -> ResultBounds {
    type Task = Either<Scope, Either<Scope, Scope>>;
    let mut ledger = Ledger::default();
    let mut kernel = Kernel::<Task>::new().unwrap();
    let local = scope(&mut kernel, &mut ledger, 17, 0);
    let bounds = kernel
        .channel::<Bounds, _, _>(Descending(IntersectIntervals), &mut ledger)
        .unwrap();
    let candidates = kernel
        .channel::<Candidates, _, _>(Descending(IntersectCandidates), &mut ledger)
        .unwrap();
    let mut stores = Cons::new(bounds, Cons::new(candidates, Nil));
    let mut rules = Pair::new(
        Registered::new(Seed),
        Pair::new(
            Registered::new(BoundsRule {
                import_candidates,
                evaluations: 0,
            }),
            Registered::new(CandidateRule {
                import_bounds,
                evaluations: 0,
            }),
        ),
    );
    let mut tasks = [
        Either::Right(Either::Left(local)),
        Either::Right(Either::Right(local)),
    ];
    if reverse {
        tasks.reverse()
    }
    // Both consumers deliberately run before initialization. Only tracked reads
    // and publications can schedule the subsequent bidirectional cycle.
    for task in tasks {
        kernel.register(local, task, &mut ledger).unwrap();
    }
    kernel
        .register(local, Either::Left(local), &mut ledger)
        .unwrap();
    kernel.seal_scope(local, &mut ledger).unwrap();
    let report = solve(&mut kernel, &mut rules, &mut stores, &mut ledger, 64).unwrap();
    assert_eq!(report.closure, Closure::Closed);
    assert!(report.steps < 64);
    if import_candidates && import_bounds {
        assert!(kernel.dependency_count() >= 8);
    } else {
        assert!(kernel.dependency_count() >= 3);
    }
    let reader = kernel
        .register(local, Either::Left(local), &mut ledger)
        .unwrap();
    let mut run = kernel.begin(reader, &mut ledger).unwrap();
    let mut cx = Context::new(&mut run, &mut stores);
    let x = <_ as Read<Bounds>>::query(&mut cx, Scoped::new(local, Variable::X))
        .unwrap()
        .value()
        .copied()
        .unwrap();
    let y = <_ as Read<Bounds>>::query(&mut cx, Scoped::new(local, Variable::Y))
        .unwrap()
        .value()
        .copied()
        .unwrap();
    let candidates_x =
        <_ as Read<Candidates, There<Here>>>::query(&mut cx, Scoped::new(local, Variable::X))
            .unwrap()
            .value()
            .copied()
            .unwrap();
    let candidates_y =
        <_ as Read<Candidates, There<Here>>>::query(&mut cx, Scoped::new(local, Variable::Y))
            .unwrap()
            .value()
            .copied()
            .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
    ResultBounds {
        x,
        y,
        cx: candidates_x,
        cy: candidates_y,
        evaluations: (
            rules.right.left.rule().evaluations,
            rules.right.right.rule().evaluations,
        ),
    }
}

#[test]
fn independent_rules_reach_a_joint_fixed_point_with_both_removal_controls() {
    let concrete: BTreeSet<_> = (0..=10u8)
        .flat_map(|x| (0..=10u8).map(move |y| (x, y)))
        .filter(|(x, y)| x + y == 10 && (4..=6).contains(y) && x % 2 == 1)
        .collect();
    assert_eq!(concrete, BTreeSet::from([(5, 5)]));
    for reverse in [false, true] {
        let full = run(true, true, reverse);
        assert_eq!((full.x, full.y), (Interval::new(5, 5), Interval::new(5, 5)));
        assert_eq!((full.cx, full.cy), (1 << 5, 1 << 5));
        assert!(full.evaluations.0 >= 3 && full.evaluations.1 >= 3);
        let gamma: BTreeSet<_> = (0..=10u8)
            .flat_map(|x| (0..=10u8).map(move |y| (x, y)))
            .filter(|(x, y)| {
                full.x.contains(*x)
                    && full.y.contains(*y)
                    && full.cx & (1 << x) != 0
                    && full.cy & (1 << y) != 0
                    && x + y == 10
            })
            .collect();
        assert_eq!(gamma, concrete);
        let no_candidate_export = run(false, true, reverse);
        assert_eq!(no_candidate_export.x, Interval::new(4, 6));
        assert!(no_candidate_export.cy.count_ones() > full.cy.count_ones());
        let no_bounds_export = run(true, false, reverse);
        assert_eq!(no_bounds_export.x, Interval::new(4, 6));
        assert!(no_bounds_export.cx.count_ones() > full.cx.count_ones());
    }
}
