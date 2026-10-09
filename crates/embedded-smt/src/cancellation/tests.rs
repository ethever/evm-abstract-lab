use super::{Cancellation, current_cancellation, interruptible, with_cancellation};
use crate::{Bv, Context, DEFAULT_RLIMIT, Outcome, Provider, Unknown, check};
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[test]
fn cancellation_is_scoped_and_does_not_poison_other_queries_or_threads() {
    let cancelled = Cancellation::new();
    cancelled.cancel();
    for provider in [Provider::Z3, Provider::Bitwuzla, Provider::Cvc5] {
        let result = with_cancellation(&cancelled, || check(&[], None, provider, DEFAULT_RLIMIT));
        assert_eq!(result, Outcome::Unknown(Unknown::Cancelled));
        assert!(!current_cancellation().is_cancelled());
        assert_eq!(
            check(&[], None, provider, DEFAULT_RLIMIT),
            Outcome::Sat(None)
        );
    }
    let outer = Cancellation::new();
    with_cancellation(&outer, || {
        with_cancellation(&cancelled, || {
            assert!(current_cancellation().is_cancelled())
        });
        assert!(!current_cancellation().is_cancelled());
        thread::scope(|scope| {
            scope.spawn(|| assert!(!current_cancellation().is_enabled()));
        });
    });
}

#[test]
fn monitor_delivers_interrupt_and_is_joined_before_returning() {
    let token = Cancellation::new();
    let (entered, ready) = mpsc::sync_channel(1);
    let (interrupt, delivered) = mpsc::sync_channel(1);
    thread::scope(|scope| {
        let cancellation = token.clone();
        scope.spawn(move || {
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
            cancellation.cancel();
        });
        interruptible(
            &token,
            || interrupt.send(()).unwrap(),
            || {
                entered.send(()).unwrap();
                delivered.recv_timeout(Duration::from_secs(2)).unwrap();
            },
        );
    });
}

#[test]
fn active_native_queries_cancel_without_consuming_their_full_resource_allowance() {
    for provider in [Provider::Z3, Provider::Bitwuzla] {
        let token = Cancellation::new();
        let (started, ready) = mpsc::sync_channel(1);
        let (finished, result) = mpsc::sync_channel(1);
        thread::scope(|scope| {
            scope.spawn(|| {
                let context = Context::default();
                let target = Bv::from_str(256, "115792089237316195423570985008687907853269984665640564039457584007908834671663").unwrap();
                let mut assertions = Vec::new();
                for index in 0..8 {
                    let x = context.fresh_bv(&format!("x{index}"), 256);
                    let y = context.fresh_bv(&format!("y{index}"), 256);
                    assertions.extend([x.bvmul(&y).eq(&target), x.bvugt(Bv::from_u64(1, 256)), y.bvugt(Bv::from_u64(1, 256))]);
                }
                started.send(()).unwrap();
                let outcome = with_cancellation(&token, || check(&assertions, None, provider, u32::MAX));
                finished.send(outcome).unwrap();
            });
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
            thread::sleep(Duration::from_millis(25));
            let at_cancel = Instant::now();
            token.cancel();
            let outcome = result.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(outcome, Outcome::Unknown(Unknown::Cancelled), "{provider}");
            assert!(at_cancel.elapsed() < Duration::from_secs(5));
        });
        assert_eq!(
            check(&[], None, provider, DEFAULT_RLIMIT),
            Outcome::Sat(None)
        );
    }
}
