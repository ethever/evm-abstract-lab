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
fn nested_scope_and_unwind_restore_the_enclosing_operation() {
    let outer = Cancellation::new();
    let inner = Cancellation::new();
    inner.cancel();
    with_cancellation(&outer, || {
        let failure = std::panic::catch_unwind(|| {
            with_cancellation(&inner, || {
                assert!(current_cancellation().is_cancelled());
                panic!("restore on unwind");
            })
        });
        assert!(failure.is_err());
        assert!(!current_cancellation().is_cancelled());
        current_cancellation().cancel();
        assert!(outer.is_cancelled());
    });
    assert!(!current_cancellation().is_enabled());
}

#[test]
fn thread_scope_is_explicit_but_captured_identity_is_shared() {
    let token = Cancellation::new();
    with_cancellation(&token, || {
        std::thread::scope(|scope| {
            let captured = current_cancellation();
            scope
                .spawn(move || {
                    assert!(!current_cancellation().is_enabled());
                    with_cancellation(&captured, || current_cancellation().cancel());
                    assert!(!current_cancellation().is_enabled());
                })
                .join()
                .unwrap();
            assert!(current_cancellation().is_cancelled());
        })
    });
    assert!(token.is_cancelled());
}
