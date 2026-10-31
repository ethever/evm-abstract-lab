use super::*;
use revm_bytecode::opcode;

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}

fn policy() -> StateLimits {
    StateLimits {
        max_nodes: 65_536,
        max_depth: 512,
    }
}

fn word(seed: u64) -> ByteStateId {
    let value = ExprId::operation(
        opcode::ADD,
        &[ExprId::fresh().unwrap(), c(seed)],
        policy().into(),
    )
    .unwrap();
    ByteStateId::zero()
        .write_word(c(0), value, policy())
        .unwrap()
}

fn compare(target: &ByteStateId, source: ByteView, dst: ExprId, len: ExprId) {
    for limits in [
        policy(),
        StateLimits {
            max_nodes: target.nodes(),
            ..policy()
        },
        StateLimits {
            max_depth: target.depth(),
            ..policy()
        },
    ] {
        let expected =
            target.copy_recanonicalizing(dst.clone(), source.clone(), len.clone(), limits);
        let actual = target.copy(dst.clone(), source.clone(), len.clone(), limits);
        assert_eq!(actual, expected);
        if let Ok(actual) = actual {
            let expected = expected.unwrap();
            assert_eq!(actual.nodes(), expected.nodes());
            assert_eq!(actual.depth(), expected.depth());
        }
    }
}

#[test]
fn canonical_source_preserves_every_target_guard_and_recursive_history() {
    let backing = word(7);
    let sliced = backing.slice(c(0), c(32), policy()).unwrap();
    let view = ByteView::new(sliced, c(0), c(32));
    let canonical = view.canonicalize();
    assert_eq!(canonical.state, backing);
    let empty = ByteStateId::zero();
    let copied = empty.copy(c(0), view.clone(), c(32), policy()).unwrap();
    let identical = copied.copy(c(0), view.clone(), c(32), policy()).unwrap();
    assert_eq!(identical, copied);
    let different = ByteView::new(word(9), c(0), c(32));
    let overwritten = copied
        .copy(c(0), different.clone(), c(32), policy())
        .unwrap();
    assert!(
        matches!(overwritten.kind(), ByteStateKind::Copy { previous, .. } if previous == &empty)
    );
    compare(&copied, view.clone(), c(0), c(32));
    compare(&copied, different, c(0), c(32));
    let guarded_overwrite = copied
        .join_guarded(&overwritten, ExprId::fresh().unwrap(), policy())
        .unwrap();
    compare(&guarded_overwrite, view.clone(), c(0), c(32));
    assert_eq!(
        guarded_overwrite.copy(c(0), view.clone(), c(32), policy()),
        Ok(copied)
    );

    // The source equals only one selected leaf. Its per-leaf self-copy shortcut
    // must survive sharing canonicalization above the target's Ite recursion.
    let alias = backing
        .join_guarded(&empty, ExprId::fresh().unwrap(), policy())
        .unwrap();
    compare(&alias, view.clone(), c(0), c(32));
    let alias_copy = alias.copy(c(0), view.clone(), c(32), policy()).unwrap();
    assert!(
        matches!(alias_copy.kind(), ByteStateKind::Ite { then_state, .. } if then_state == &backing)
    );

    let other = ByteStateId::exact(Arc::from([0x22]));
    let pair = empty
        .join_guarded(&other, ExprId::fresh().unwrap(), policy())
        .unwrap();
    let mut selected = pair.clone();
    let third = ByteStateId::exact(Arc::from([0x33]));
    // Rotate distinct byte-0 contents, outside the copy window at 64: no new
    // leaf equals either immediate child, so shared-branch factoring cannot fire.
    // Shared children still count per incoming path: 2 + 17 = 19 leaves.
    for leaf in [&third, &empty, &other].into_iter().cycle().take(17) {
        selected = selected
            .join_guarded(leaf, ExprId::fresh().unwrap(), policy())
            .unwrap();
    }
    assert_eq!(selected.copy_leaves(), 19);
    compare(&selected, view.clone(), c(64), c(32));
    let mut history = pair;
    for at in 0..3 {
        history = history.write_byte(c(at), c(at + 1), policy()).unwrap();
    }
    assert_eq!(history.copy_leaves(), 1);
    let guarded_history = history
        .join_guarded(&other, ExprId::fresh().unwrap(), policy())
        .unwrap();
    compare(&guarded_history, view.clone(), c(64), c(32));

    let term = ExprId::operation(
        opcode::ADD,
        &[ExprId::fresh().unwrap(), c(32)],
        policy().into(),
    )
    .unwrap();
    compare(&selected, view.clone(), term.clone(), term.clone());
    compare(
        &selected,
        ByteView::new(backing, term.clone(), term.clone()),
        c(64),
        term,
    );
    compare(
        &selected,
        ByteView::new(word(11), c(99), c(0)),
        c(64),
        c(32),
    );
}

#[test]
fn zero_copy_checks_target_before_invalid_source_and_keeps_cancel_priority() {
    let target = ByteStateId::zero();
    let source = ByteView::new(word(7), ExprId::fresh().unwrap(), c(32));
    let limits = StateLimits {
        max_nodes: 1,
        max_depth: 1,
    };
    assert_eq!(
        target.copy(c(0), source.clone(), c(0), limits),
        Ok(target.clone())
    );
    assert_eq!(
        target.copy(
            c(0),
            source.clone(),
            c(0),
            StateLimits {
                max_nodes: 0,
                ..limits
            }
        ),
        Err(StateError::Limit { nodes: 1, depth: 1 })
    );
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        for len in [c(0), c(32)] {
            assert_eq!(
                target.copy(c(0), source.clone(), len.clone(), limits),
                target.copy_recanonicalizing(c(0), source.clone(), len, limits)
            );
            assert_eq!(
                target.copy(c(0), source.clone(), c(0), limits),
                Err(StateError::Cancelled)
            );
        }
    });
}
