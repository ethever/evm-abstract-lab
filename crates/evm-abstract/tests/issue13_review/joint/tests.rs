use super::*;
use evm_abstract::domain::{
    identity::Symbol,
    relational::RelationLimits,
    symbolic::{SlotStateId, SourceId},
};

fn word(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}

fn ite(guard: &ExprId, yes: ExprId, no: ExprId) -> ExprId {
    ExprId::ite(guard.clone(), yes, no, ExprLimits::default()).unwrap()
}

// Zero is the all-success case; nonzero rejects the second call but retains
// the same persistent values and paired owner201 counter.
fn fixture(guard: &ExprId) -> Terms {
    let second = ite(guard, word(0), word(1));
    [
        word(1),
        word(1),
        word(2),
        word(2),
        second.clone(),
        word(0),
        word(0),
        word(1),
        second,
    ]
}

fn check(terms: &Terms) -> Result<Cover, Error> {
    verify(&RelationState::default(), terms, ExprLimits::default())
}

fn violation(terms: &Terms, expected: &str) {
    match check(terms) {
        Err(Error::Control { name, .. }) => assert_eq!(name, expected),
        other => panic!("expected {expected}, got {other:?}"),
    }
}

#[test]
fn covers_compound_truth_guards_and_replays_full_word_nonzero_maps() {
    let first = ExprId::fresh().unwrap();
    let second = ExprId::fresh().unwrap();
    // first is an ARM of a condition; its demand must still be Truth.
    let guard = ite(&second, word(2), first.clone());
    let terms = fixture(&guard);
    assert_eq!(admit(&terms, ExprLimits::default()).unwrap().len(), 2);
    let cover = check(&terms).unwrap();
    assert_eq!(cover.terminal_vectors, 2);
    assert!(cover.proof_nodes <= ExprLimits::default().max_nodes);

    // These operations remain symbolic until cofactoring: 2 & 1 = 0,
    // and MAX + 3 wraps to 2. Neither may use Boolean word semantics.
    let mut words = terms;
    let masked = ExprId::operation(
        opcode::AND,
        &[ite(&first, word(2), word(0)), word(1)],
        ExprLimits::default(),
    )
    .unwrap();
    words[3] = ExprId::operation(opcode::ADD, &[word(2), masked], ExprLimits::default()).unwrap();
    words[2] = ExprId::operation(
        opcode::ADD,
        &[
            ite(&first, ExprId::constant(U256::MAX), word(0)),
            ite(&first, word(3), word(2)),
        ],
        ExprLimits::default(),
    )
    .unwrap();
    check(&words).unwrap();
}

#[test]
fn catches_counter_and_owner_contamination_only_on_nonzero_branch() {
    let guard = ExprId::fresh().unwrap();
    for (index, expected) in [
        (
            3,
            "returned values independently determine owner201 counter",
        ),
        (5, "code/caller owner counter contamination"),
        (6, "code/caller owner counter contamination"),
    ] {
        let mut terms = fixture(&guard);
        terms[index] = ite(&guard, word(1), terms[index].clone());
        violation(&terms, expected);
    }
}

#[test]
fn different_fresh_in_same_shaped_route_does_not_pair_independent_choices() {
    let guard = ExprId::fresh().unwrap();
    let other = ExprId::fresh().unwrap();
    let mut terms = fixture(&guard);
    terms[8] = fixture(&other)[8].clone();
    violation(&terms, "qualified wrong owner counter");
}

#[test]
fn rejects_wrong_add_stale_counter_and_swapped_guard_arms() {
    let guard = ExprId::fresh().unwrap();
    let base = fixture(&guard);
    for wrong in [
        ExprId::operation(opcode::ADD, &[word(2), word(1)], ExprLimits::default()).unwrap(),
        word(1),
    ] {
        let mut terms = base.clone();
        terms[3] = ite(&guard, wrong, word(2));
        violation(
            &terms,
            "returned values independently determine owner201 counter",
        );
    }
    // Swapping only the route arms must fail despite both scalar projections
    // still containing exactly {0, 1}. Zero remains a common success witness.
    let other = ExprId::fresh().unwrap();
    let mut terms = fixture(&ite(&guard, other.clone(), word(0)));
    terms[8] = ite(&guard, ite(&other, word(1), word(0)), word(1));
    violation(&terms, "qualified wrong owner counter");
}

#[test]
fn rejects_wrong_route_missing_rejected_second_and_nonboolean_route_data() {
    let guard = ExprId::fresh().unwrap();
    let mut terms = fixture(&guard);
    terms[8] = word(1);
    violation(&terms, "qualified wrong owner counter");

    let mut terms = fixture(&guard);
    terms[4] = word(1);
    terms[8] = word(1);
    assert!(matches!(check(&terms), Err(Error::Missing(name)) if name == POSITIVES[3]));

    let mut terms = fixture(&guard);
    terms[7] = ite(&guard, word(2), word(1));
    violation(&terms, "route predicates must remain Boolean");
}

#[test]
fn rejects_nonempty_bottom_and_nontrue_path_f() {
    let guard = ExprId::fresh().unwrap();
    let terms = fixture(&guard);
    let limits = RelationLimits::default();
    let mut nonempty = RelationState::default();
    nonempty.assume(&guard, true, &limits);
    assert!(!nonempty.is_empty());
    assert!(nonempty.constraints().len() > 0);

    let mut bottom = RelationState::default();
    bottom.assume(&word(0), true, &limits);
    assert!(bottom.is_bottom());

    let path = nonempty
        .join_guarded(&RelationState::default(), ExprId::fresh().unwrap(), &limits)
        .unwrap();
    assert_eq!(path.constraints().len(), 0);
    assert!(!matches!(path.path().kind(), RelationPathKind::True));
    for relations in [nonempty, bottom, path] {
        assert!(matches!(
            verify(&relations, &terms, limits.expressions()),
            Err(Error::Relation { .. })
        ));
    }
}

#[test]
fn rejects_word_fresh_unsupported_input_and_source_read_without_aliasing() {
    let guard = ExprId::fresh().unwrap();
    let mut terms = fixture(&guard);
    // The SAME Fresh is valid in the route guard, invalid at a word demand.
    terms[5] = guard.clone();
    assert!(matches!(check(&terms), Err(Error::WordFresh(_))));
    terms[5] = ExprId::operation(
        opcode::AND,
        &[guard.clone(), word(1)],
        ExprLimits::default(),
    )
    .unwrap();
    assert!(matches!(check(&terms), Err(Error::WordFresh(_))));

    let input = ExprId::input(13, Symbol::CallValue);
    let read = ExprId::slot_read(
        SlotStateId::input(SourceId::scoped(guard.clone(), U256::ZERO)),
        word(0),
        ExprLimits::default(),
    )
    .unwrap();
    let unsupported = ExprId::operation(
        opcode::XOR,
        &[fixture(&guard)[4].clone(), word(2)],
        ExprLimits::default(),
    )
    .unwrap();
    for expression in [input, read, unsupported] {
        let mut terms = fixture(&guard);
        terms[5] = expression;
        assert!(matches!(check(&terms), Err(Error::Unsupported(_))));
    }
}

#[test]
fn cover_cap_refuses_even_after_visiting_successful_terminal() {
    let guards: [ExprId; 4] = std::array::from_fn(|_| ExprId::fresh().unwrap());
    let mut terms = fixture(&guards[3]);
    for (index, expected) in [1, 1, 2].into_iter().enumerate() {
        terms[index] = ite(&guards[index], word(0), word(expected));
    }
    // Eleven admitted DAG nodes, but 31 distinct coupled residual tuples.
    // The zero-first traversal visits the full success vector before refusal.
    let limits = ExprLimits {
        max_nodes: 12,
        ..ExprLimits::default()
    };
    assert!(admit(&terms, limits).is_ok());
    assert!(matches!(
        verify(&RelationState::default(), &terms, limits),
        Err(Error::Cap {
            phase: "shared tuple cover",
            limit: 12
        })
    ));
    let cover = check(&terms).unwrap();
    assert_eq!(cover.proof_nodes, 31);
    assert_eq!(cover.terminal_vectors, 16);
    // Shallow expressions can still require a deeper shared decision path.
    let shallow = ExprLimits {
        max_depth: 2,
        ..ExprLimits::default()
    };
    assert!(admit(&terms, shallow).is_ok());
    assert!(matches!(
        verify(&RelationState::default(), &terms, shallow),
        Err(Error::Cap {
            phase: "shared tuple cover depth",
            limit: 2
        })
    ));
}
