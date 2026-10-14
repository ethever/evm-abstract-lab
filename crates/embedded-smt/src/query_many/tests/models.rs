use super::{Bool, BudgetedUniqueOutcome, Bv, Context, Provider, UniqueOutcome};
use crate::{Error, Outcome, Unknown};

// Supply scripted failures to the production state machine. Every requested
// model is the same complete projection; every query keeps the same DAG prefix.
fn scripted(base: &[Bool], targets: &[Bv], outcomes: &[Outcome]) -> Vec<BudgetedUniqueOutcome> {
    let width = targets.iter().map(Bv::width).sum();
    let mut calls = 0;
    let mut packed_id = None;
    let actual = super::super::solve_many(base, targets, width, None, |assertions, packed| {
        assert_eq!(assertions.len(), base.len() + usize::from(calls != 0));
        for (actual, expected) in assertions.iter().zip(base) {
            assert_eq!(actual.0.id(), expected.0.id());
        }
        assert_eq!(packed.width(), width);
        assert_eq!(*packed_id.get_or_insert(packed.0.id()), packed.0.id());
        let outcome = outcomes.get(calls).expect("unexpected extra query").clone();
        calls += 1;
        outcome
    });
    assert_eq!(calls, outcomes.len());
    actual
}

#[test]
fn successive_complete_models_resolve_only_changed_remaining_targets_after_one_unproved_and_attempt()
 {
    let context = Context::default();
    let targets: Vec<_> = (0..3).map(|_| context.fresh_bv("step", 8)).collect();
    let base = [
        Bool::or(&[
            targets[0].eq(Bv::from_u64(1, 8)),
            targets[0].eq(Bv::from_u64(17, 8)),
        ]),
        targets[1].eq(Bv::from_u64(2, 8)),
        Bool::or(&[
            targets[2].eq(Bv::from_u64(3, 8)),
            targets[2].eq(Bv::from_u64(19, 8)),
        ]),
    ];
    assert_eq!(
        scripted(
            &base,
            &targets,
            &[
                Outcome::Sat(Some("#x010203".into())),
                Outcome::Unsat, // AND cannot change the fixed middle target.
                Outcome::Sat(Some("#x110203".into())),
                Outcome::Sat(Some("#x110213".into())),
                Outcome::Unsat,
            ]
        ),
        [
            BudgetedUniqueOutcome::Solver(UniqueOutcome::Multiple),
            BudgetedUniqueOutcome::Solver(UniqueOutcome::Unique("#x02".into())),
            BudgetedUniqueOutcome::Solver(UniqueOutcome::Multiple),
        ],
    );
}

#[test]
fn absent_illegal_incomplete_and_nonprogressing_models_stop_without_fabricating_a_proof() {
    use BudgetedUniqueOutcome::Solver;
    let target = Context::default().fresh_bv("model", 8);
    let missing = Solver(UniqueOutcome::ModelUnavailable);
    for bad in [
        Outcome::Sat(None),
        Outcome::Sat(Some("#x0".into())),
        Outcome::Sat(Some("#x000".into())),
        Outcome::Sat(Some("#xgg".into())),
        Outcome::Sat(Some("#b00000002".into())),
        Outcome::Sat(Some("(_ bv0 8)".into())),
    ] {
        assert_eq!(
            scripted(
                &[],
                std::slice::from_ref(&target),
                std::slice::from_ref(&bad)
            )
            .as_slice(),
            std::slice::from_ref(&missing)
        );
        assert_eq!(
            scripted(
                &[],
                std::slice::from_ref(&target),
                &[Outcome::Sat(Some("#x00".into())), bad,]
            )
            .as_slice(),
            std::slice::from_ref(&missing)
        );
    }
    assert_eq!(
        scripted(
            &[],
            &[target],
            &[
                Outcome::Sat(Some("#x00".into())),
                Outcome::Sat(Some("#b00000000".into())),
            ]
        )
        .as_slice(),
        std::slice::from_ref(&missing)
    );

    let context = Context::default();
    let targets = [context.fresh_bv("first", 8), context.fresh_bv("second", 8)];
    // The third model changes only an already resolved target. It proves
    // nothing about the remaining target and must not spin or label it Multiple.
    assert_eq!(
        scripted(
            &[],
            &targets,
            &[
                Outcome::Sat(Some("#x0000".into())),
                Outcome::Unsat, // speculative AND supplies no proof
                Outcome::Sat(Some("#x0100".into())),
                Outcome::Sat(Some("#x0200".into())),
            ]
        ),
        [Solver(UniqueOutcome::Multiple), missing]
    );
}

#[test]
fn original_unknown_and_error_reasons_survive_partial_proofs() {
    use BudgetedUniqueOutcome::Solver;
    let context = Context::default();
    let targets = [context.fresh_bv("first", 8), context.fresh_bv("second", 8)];
    let error = Error {
        provider: Provider::Cvc5,
        message: "original native failure".into(),
    };
    for (outcome, expected) in [
        (
            Outcome::Unknown(Unknown::Cancelled),
            UniqueOutcome::Unknown(Unknown::Cancelled),
        ),
        (
            Outcome::Unknown(Unknown::ResourceLimit),
            UniqueOutcome::Unknown(Unknown::ResourceLimit),
        ),
        (
            Outcome::Unknown(Unknown::Solver("original reason".into())),
            UniqueOutcome::Unknown(Unknown::Solver("original reason".into())),
        ),
        (Outcome::Error(error.clone()), UniqueOutcome::Error(error)),
        (Outcome::Sat(None), UniqueOutcome::ModelUnavailable),
    ] {
        assert_eq!(
            scripted(&[], &targets, std::slice::from_ref(&outcome)),
            vec![Solver(expected.clone()); 2]
        );
        assert_eq!(
            scripted(
                &[],
                &targets,
                &[
                    Outcome::Sat(Some("#x0000".into())),
                    Outcome::Unsat, // speculative AND supplies no proof
                    Outcome::Sat(Some("#x0100".into())),
                    outcome,
                ]
            ),
            [Solver(UniqueOutcome::Multiple), Solver(expected)]
        );
    }
}

#[test]
fn mutually_exclusive_changes_need_or_after_and_proves_nothing() {
    use BudgetedUniqueOutcome::Solver;
    let context = Context::default();
    let x = context.fresh_bv("x", 8);
    let y = context.fresh_bv("y", 8);
    let zero = Bv::from_u64(0, 8);
    let one = Bv::from_u64(1, 8);
    let base = [Bool::or(&[
        Bool::and(&[x.eq(&zero), y.eq(&zero)]),
        Bool::and(&[x.eq(&one), y.eq(&zero)]),
        Bool::and(&[x.eq(&zero), y.eq(&one)]),
    ])];
    let mut exclusion = base.to_vec();
    exclusion.push(Bool::and(&[x.eq(&zero).not(), y.eq(&zero).not()]));
    assert_eq!(
        crate::check(&exclusion, None, Provider::Z3, crate::DEFAULT_RLIMIT),
        Outcome::Unsat
    );
    assert_eq!(
        scripted(
            &base,
            &[x, y],
            &[
                Outcome::Sat(Some("#x0000".into())),
                Outcome::Unsat,
                Outcome::Sat(Some("#x0100".into())),
                Outcome::Sat(Some("#x0001".into())),
            ]
        ),
        [
            Solver(UniqueOutcome::Multiple),
            Solver(UniqueOutcome::Multiple)
        ]
    );
}

#[test]
fn speculative_unknown_falls_through_but_partial_and_model_is_not_a_proof() {
    use BudgetedUniqueOutcome::Solver;
    let context = Context::default();
    let targets = [context.fresh_bv("x", 8), context.fresh_bv("y", 8)];
    assert_eq!(
        scripted(
            &[],
            &targets,
            &[
                Outcome::Sat(Some("#x0000".into())),
                Outcome::Unknown(Unknown::ResourceLimit),
                Outcome::Sat(Some("#x0100".into())),
                Outcome::Sat(Some("#x0001".into())),
            ]
        ),
        [
            Solver(UniqueOutcome::Multiple),
            Solver(UniqueOutcome::Multiple)
        ]
    );
    assert_eq!(
        scripted(
            &[],
            &targets,
            &[
                Outcome::Sat(Some("#x0000".into())),
                Outcome::Sat(Some("#x0100".into())),
            ]
        ),
        [
            Solver(UniqueOutcome::ModelUnavailable),
            Solver(UniqueOutcome::ModelUnavailable)
        ]
    );
}
