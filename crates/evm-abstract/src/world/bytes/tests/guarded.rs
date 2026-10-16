//! One selector chooses a whole word, never independent bytes from both states.

use super::super::*;
use crate::domain::{
    identity::Symbol,
    provenance::Origin,
    relational::{CheckResult, RelationLimits, RelationState, SmtProvider, ValueQuery},
    symbolic::ExprKind,
};

fn policy(provider: SmtProvider) -> RelationLimits {
    RelationLimits {
        provider,
        max_nodes: 16_384,
        max_depth: 256,
        max_constraints: 2048,
        rlimit: 10_000_000,
        ..RelationLimits::default()
    }
}

fn observed(scope: u64, value: usize, origin: Origin) -> AbstractValue {
    let mut input = AbstractValue::top();
    input.set_origin(origin);
    input
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
        .with_numeric(constant(value).numeric().clone())
}

fn joined(
    a: &AbstractValue,
    b: &AbstractValue,
    domain: Domain,
    limits: RelationLimits,
) -> (ByteArray, ExprId) {
    let source = ByteArray::exact(&[0xab]);
    let mut yes = ByteArray::memory();
    let mut no = ByteArray::memory();
    yes.write_word(&constant(64), a, 4096, domain).unwrap();
    no.write_word(&constant(64), b, 4096, domain).unwrap();
    for iteration in 0..24 {
        for branch in [&mut yes, &mut no] {
            branch
                .copy_from(
                    &constant(512 + iteration * 64),
                    &source,
                    &zero(),
                    &constant(1),
                    4096,
                    domain,
                )
                .unwrap();
        }
    }
    // Different update kinds require a real snapshot Ite, not a joined value
    // already factored into one latest word write.
    yes.copy_from(
        &constant(3000),
        &source,
        &zero(),
        &constant(1),
        4096,
        domain,
    )
    .unwrap();
    no.write_byte(&constant(3010), &constant(7), 4096, domain)
        .unwrap();
    let guard = ExprId::fresh().unwrap();
    let joined = yes
        .join_guarded(&no, domain, guard.clone(), limits.expressions())
        .unwrap();
    assert!(matches!(
        joined.state_root().kind(),
        ByteStateKind::Ite { .. }
    ));
    (joined, guard)
}

#[test]
fn guarded_word_reads_keep_whole_word_correlation_and_old_solver_sources() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let domain = Domain::from_spec(Domain::default().spec().with_relations(limits));
        let a = observed(991, 0x0100, Origin::Calldata);
        let b = observed(992, 0xff01, Origin::Returndata);
        let (mut memory, guard) = joined(&a, &b, domain, limits);
        let snapshot = memory.clone();
        let read = memory.read_word_checked(&constant(64), domain).unwrap();
        // Before normalization this read retains311 nodes and adds the false
        // numeric combinations0x0101/0xff00 by joining individual bytes.
        assert!(read.expression().unwrap().nodes() <= 16);
        assert_eq!(
            read.constants(),
            Some(&BTreeSet::from([U256::from(0x0100), U256::from(0xff01)]))
        );
        assert!(
            matches!(read.expression().unwrap().kind(), ExprKind::Ite { condition, .. } if condition == &guard)
        );
        let mut facts = RelationState::default();
        for (expression, value) in memory.symbolic_facts() {
            facts.observe(expression, value.numeric(), &limits).unwrap();
        }
        for (yes, expected) in [(true, 0x0100), (false, 0xff01)] {
            let mut relations = facts.clone();
            assert_eq!(relations.assume(&guard, yes, &limits), CheckResult::Sat);
            assert_eq!(
                relations.unique_value(&expr(&read).unwrap(), &limits),
                ValueQuery::Unique(U256::from(expected))
            );
            assert_eq!(
                relations.can_equal(&expr(&read).unwrap(), U256::from(0x0101), &limits),
                CheckResult::Unsat
            );
        }
        memory
            .write_word(&constant(64), &constant(11), 4096, domain)
            .unwrap();
        assert_eq!(
            memory.read_word(&constant(64), domain).singleton(),
            Some(U256::from(11))
        );
        assert_eq!(snapshot.read_word(&constant(64), domain), read);
        for (yes, expected) in [(true, 0x0100), (false, 0xff01)] {
            let mut relations = facts.clone();
            assert_eq!(relations.assume(&guard, yes, &limits), CheckResult::Sat);
            assert_eq!(
                relations.unique_value(&expr(&read).unwrap(), &limits),
                ValueQuery::Unique(U256::from(expected))
            );
        }
    }
}

#[test]
fn guarded_equal_numbers_keep_independent_sources_and_identity_meet() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let domain = Domain::from_spec(Domain::default().spec().with_relations(limits));
        let a = observed(993, 7, Origin::Calldata);
        let b = observed(994, 7, Origin::Returndata);
        let (memory, _) = joined(&a, &b, domain, limits);
        let read = memory.read_word_checked(&constant(64), domain).unwrap();
        assert_eq!(read.singleton(), Some(U256::from(7)));
        assert!(matches!(
            read.expression().unwrap().kind(),
            ExprKind::Ite { .. }
        ));
        assert!(!read.same_identity(&a));
        assert!(!read.same_identity(&b));
        assert!(read.identity().input().is_none());
        for origin in [Origin::Calldata, Origin::Returndata] {
            assert!(
                read.provenance()
                    .origins()
                    .sources()
                    .unwrap()
                    .contains(&origin)
            );
        }
        for current in [
            &memory,
            &memory.widening_form(domain, limits.expressions()).unwrap(),
        ] {
            let mut relations = RelationState::default();
            for (expression, value) in current.symbolic_facts() {
                relations
                    .observe(expression, value.numeric(), &limits)
                    .unwrap();
            }
            for original in [&a, &b] {
                assert_eq!(
                    current.operands.get(original.expression().unwrap()),
                    Some(original)
                );
            }
            let read = current.read_word_checked(&constant(64), domain).unwrap();
            assert_eq!(
                relations.unique_value(&expr(&read).unwrap(), &limits),
                ValueQuery::Unique(U256::from(7))
            );
        }
    }
}

#[test]
fn guarded_word_views_preserve_padding_symbolic_offset_limits_and_cancellation() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let term = |n| ExprId::constant(n);
        let yes = ByteStateId::zero()
            .write_byte(
                term(U256::MAX),
                term(U256::from(0xab)),
                limits.expressions().into(),
            )
            .unwrap();
        let no = ByteStateId::zero()
            .write_word(
                term(U256::MAX),
                term(U256::MAX),
                limits.expressions().into(),
            )
            .unwrap();
        let guard = ExprId::fresh().unwrap();
        let state = yes
            .join_guarded(&no, guard.clone(), limits.expressions().into())
            .unwrap();
        assert!(matches!(state.kind(), ByteStateKind::Ite { .. }));
        let view = ByteView::new(state.clone(), term(U256::MAX), term(U256::from(32)));
        let read = ExprId::word_read(view.clone(), term(U256::ZERO), limits.expressions()).unwrap();
        for (yes, expected) in [(true, 0xab), (false, 0xff)] {
            let mut relations = RelationState::default();
            assert_eq!(relations.assume(&guard, yes, &limits), CheckResult::Sat);
            assert_eq!(
                relations.unique_value(&read, &limits),
                ValueQuery::Unique(U256::from(expected) << 248)
            );
        }
        let padded = ByteView::new(state.clone(), term(U256::ZERO), term(U256::from(31)));
        let padded_read =
            ExprId::word_read(padded.clone(), term(U256::ZERO), limits.expressions()).unwrap();
        assert_eq!(
            RelationState::default().unique_value(&padded_read, &limits),
            ValueQuery::Unique(U256::ZERO)
        );
        let symbolic = ExprId::word_read(
            ByteView::new(
                state.clone(),
                ExprId::fresh().unwrap(),
                term(U256::from(32)),
            ),
            term(U256::ZERO),
            limits.expressions(),
        )
        .unwrap();
        assert!(matches!(symbolic.kind(), ExprKind::WordRead { view, .. } if view.state == state));
        assert!(matches!(
            ExprId::word_read(
                padded.clone(),
                term(U256::ZERO),
                ExprLimits {
                    max_nodes: 1,
                    ..limits.expressions()
                }
            ),
            Err(ExprError::Limit { .. })
        ));
        let cancellation = analysis_control::Cancellation::new();
        cancellation.cancel();
        analysis_control::with_cancellation(&cancellation, || {
            assert_eq!(
                ExprId::word_read(view, term(U256::ZERO), limits.expressions()),
                Err(ExprError::Cancelled)
            );
        });
    }
}
