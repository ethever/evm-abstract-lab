use super::Counter;
use std::{sync::Barrier, thread};

#[test]
fn non_copy_payload_moves_out_and_scope_resets() {
    let counter = Counter::new(None::<String>);
    {
        let _scope = counter.scope();
        assert_eq!(counter.replace(Some(String::from("pending slot"))), None);
        assert_eq!(counter.replace(None).as_deref(), Some("pending slot"));
        assert_eq!(counter.replace(None), None);
    }
    let _scope = counter.scope();
    assert_eq!(counter.replace(None), None);
}

#[test]
fn concurrent_scopes_keep_independent_thread_values() {
    let counter = Counter::new(0usize);
    let _scope = counter.scope();
    counter.set(7);
    let barrier = Barrier::new(3);
    thread::scope(|scope| {
        for value in [11, 23] {
            let counter = &counter;
            let barrier = &barrier;
            scope.spawn(move || {
                let _scope = counter.scope();
                counter.set(value);
                barrier.wait();
                assert_eq!(counter.get(), value);
            });
        }
        barrier.wait();
        assert_eq!(counter.get(), 7);
    });
    assert_eq!(counter.get(), 7);
}

#[test]
#[should_panic(expected = "counter update panic")]
fn poisoned_update_still_cleans_scope_during_unwinding() {
    struct CheckCleanup<'a>(&'a Counter<usize>);
    impl Drop for CheckCleanup<'_> {
        fn drop(&mut self) {
            // Runs after the old scope drops while the thread is unwinding.
            let _scope = self.0.scope();
            assert_eq!(self.0.get(), 0);
            self.0.set(19);
            assert_eq!(self.0.get(), 19);
        }
    }

    let counter = Counter::new(0usize);
    let _cleanup = CheckCleanup(&counter);
    let _scope = counter.scope();
    counter.update(|value| {
        *value = 31;
        panic!("counter update panic");
    });
}
