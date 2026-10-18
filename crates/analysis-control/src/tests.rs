use super::{Cancellation, current_cancellation, with_cancellation};

#[test]
fn defaults_are_disabled_and_clones_share_one_way_cancellation() {
    let disabled = Cancellation::default();
    disabled.cancel();
    assert!(!disabled.is_enabled());
    assert!(!disabled.is_cancelled());
    let token = Cancellation::new();
    let worker = token.clone();
    worker.cancel();
    assert!(token.is_cancelled());
}

#[test]
#[should_panic(expected = "restore on unwind")]
fn nested_scope_and_unwind_restore_the_enclosing_operation() {
    enum RestoredScope<'a> {
        Baseline,
        Outer(&'a Cancellation),
    }
    impl Drop for RestoredScope<'_> {
        fn drop(&mut self) {
            let current = current_cancellation();
            match self {
                Self::Baseline => assert!(!current.is_enabled()),
                Self::Outer(outer) => {
                    assert!(current.is_enabled());
                    assert!(!current.is_cancelled());
                    current.cancel();
                    assert!(outer.is_cancelled());
                }
            }
        }
    }

    let outer = Cancellation::new();
    let inner = Cancellation::new();
    inner.cancel();
    // This witness drops after the outer scope's Restore during unwinding.
    let _baseline_restored = RestoredScope::Baseline;
    with_cancellation(&outer, || {
        // This witness drops after the inner Restore and before the outer Restore.
        let _outer_restored = RestoredScope::Outer(&outer);
        with_cancellation(&inner, || {
            assert!(current_cancellation().is_cancelled());
            panic!("restore on unwind");
        });
    });
}

#[test]
fn thread_scope_is_explicit_but_captured_identity_is_shared() {
    let token = Cancellation::new();
    with_cancellation(&token, || {
        std::thread::scope(|scope| {
            let captured = current_cancellation();
            scope.spawn(move || {
                assert!(!current_cancellation().is_enabled());
                with_cancellation(&captured, || current_cancellation().cancel());
                assert!(!current_cancellation().is_enabled());
            });
        });
        assert!(current_cancellation().is_cancelled());
    });
    assert!(token.is_cancelled());
    assert!(!current_cancellation().is_enabled());
}
