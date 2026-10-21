use super::*;

fn withdrawal_phases() -> usize {
    let mut s = Scenario::new();
    let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
    run.current_generation(&s.f.entry).unwrap();
    let before = run.meter().calls;
    run.withdraw_generation(&mut s.f.entry).unwrap();
    run.meter().calls - before
}

#[test]
fn every_withdrawal_refusal_and_cancellation_preserves_selection_payload_handles_guards_and_wakes()
{
    let phases = withdrawal_phases();
    assert!(phases > 0);
    for stop in [Stop::Work, Stop::Cancelled] {
        for phase in 1..=phases {
            let mut s = Scenario::new();
            let bound = s.read_b();
            let queue = s.f.kernel.worklist().clone();
            let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
            let before = run.current_generation(&s.f.entry).unwrap();
            run.meter().stop_after(phase, stop);
            assert_eq!(
                run.withdraw_generation(&mut s.f.entry),
                Err(KernelError::Stopped(stop)),
                "{stop:?} at reservation {phase}"
            );
            run.meter().clear_stop();
            assert_eq!(run.current_generation(&s.f.entry).unwrap(), before);
            drop(run);
            assert_eq!(s.f.kernel.worklist(), &queue);
            assert!(s.f.kernel.task::<Stop>(s.f.reader).is_ok());
            assert!(!s.f.stale(0) && !s.f.stale(1));
            assert_eq!(s.read_b(), bound);
        }
    }
}

fn reactivation_phases() -> usize {
    let mut s = empty();
    let (next, _) = next_source(&mut s, 3);
    let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
    run.current_generation(&s.f.entry).unwrap();
    let before = run.meter().calls;
    run.replace_generation(&mut s.f.entry, next).unwrap();
    run.meter().calls - before
}

#[test]
fn every_reactivation_refusal_and_cancellation_preserves_empty_premises_and_can_retry_same_scope() {
    let phases = reactivation_phases();
    assert!(phases > 0);
    for stop in [Stop::Work, Stop::Cancelled] {
        for phase in 1..=phases {
            let mut s = empty();
            let (next, _) = next_source(&mut s, 3);
            let queue = s.f.kernel.worklist().clone();
            let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
            let before = run.current_generation(&s.f.entry).unwrap();
            assert_eq!(before.state, Availability::Absent);
            run.meter().stop_after(phase, stop);
            assert_eq!(
                run.replace_generation(&mut s.f.entry, next),
                Err(KernelError::Stopped(stop)),
                "{stop:?} at reservation {phase}"
            );
            run.meter().clear_stop();
            assert_eq!(run.current_generation(&s.f.entry).unwrap(), before);
            drop(run);
            assert_eq!(s.f.kernel.worklist(), &queue);
            assert!(!s.f.stale(0) && !s.f.stale(1));
            assert_eq!(s.read_b().value(), Some(&1));
            let mut run = s.f.kernel.begin(s.f.owner, &mut s.f.meter).unwrap();
            run.replace_generation(&mut s.f.entry, next).unwrap();
            assert_eq!(
                run.current_generation(&s.f.entry).unwrap().value(),
                Some(&next)
            );
            run.finish(LocalClosure::Stable).unwrap();
            assert!(s.f.stale(1));
        }
    }
}
