use super::{Budget, Stop, scope};
use crate::{Change, Closure, Kernel, KernelError, LocalClosure};
use std::cell::Cell;

#[test]
fn abort_preserves_live_graph_and_charges_and_never_revives_escaped_ids() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::<()>::new().unwrap();
    let s = scope(&mut kernel, &mut meter, 1, 0);
    kernel.seal_scope(s, &mut meter).unwrap();
    let before = kernel.statistics();
    let used = meter.used;
    let mut fork = kernel.fork(&mut meter).unwrap();
    let escaped_job = fork.kernel().register(s, (), &mut meter).unwrap();
    let escaped_slot = fork.kernel().slot::<u32, _>(s, &mut meter).unwrap();
    let escaped_obligation = fork.kernel().obligation(s, &mut meter).unwrap();
    let escaped_scope = scope(fork.kernel(), &mut meter, 9, 0);
    drop(fork);
    assert_eq!(kernel.statistics(), before);
    assert_eq!(kernel.closure(), Closure::Closed);
    assert!(
        meter.used > used,
        "aborted speculative work remains charged"
    );
    let job = kernel.register(s, (), &mut meter).unwrap();
    let slot = kernel.slot::<u32, _>(s, &mut meter).unwrap();
    let obligation = kernel.obligation(s, &mut meter).unwrap();
    let live_scope = scope(&mut kernel, &mut meter, 9, 0);
    assert_ne!(job, escaped_job);
    assert_ne!(live_scope, escaped_scope);
    assert!(matches!(
        kernel.begin(escaped_job, &mut meter),
        Err(KernelError::InvalidJob)
    ));
    assert_eq!(
        kernel.resolve_obligation(escaped_obligation, &mut meter),
        Err(KernelError::InvalidHandle)
    );
    assert_eq!(
        kernel.register(escaped_scope, (), &mut meter),
        Err(KernelError::InactiveScope)
    );
    let mut run = kernel.begin(job, &mut meter).unwrap();
    assert_eq!(
        run.read_slot(&escaped_slot, &0, 1),
        Err(KernelError::InvalidHandle)
    );
    assert!(run.read_slot(&slot, &0, 1).is_ok());
    run.finish(LocalClosure::Stable).unwrap();
    kernel.resolve_obligation(obligation, &mut meter).unwrap();
}

#[test]
fn replay_commits_detached_values_metadata_and_notifications_together() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let s = scope(&mut kernel, &mut meter, 1, 0);
    let writer = kernel.register(s, (), &mut meter).unwrap();
    let reader = kernel.register(s, (), &mut meter).unwrap();
    let meta = kernel.slot(s, &mut meter).unwrap();
    let mut value = vec![1];
    let mut evidence = vec![1];
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let before = run.read_slot(&meta, &value, 1).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut token = kernel.begin(writer, &mut meter).unwrap().suspend();
    let mut fork = kernel.fork(&mut meter).unwrap();
    let staged_meta = meta.clone();
    // The adapter prepays its own potentially deep semantic snapshot.
    let mut run = fork.kernel().resume(&mut token, &mut meter).unwrap();
    run.reserve(4).unwrap();
    let mut staged_value = value.clone();
    let mut staged_evidence = evidence.clone();
    run.publish_prepared_parts(
        &staged_meta,
        &mut staged_value,
        vec![1, 2],
        &mut staged_evidence,
        vec![1, 2],
        Change::Changed,
        6,
    )
    .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(value, [1]);
    assert!(!kernel.worklist().contains(&reader));
    kernel
        .commit_with(fork, &mut meter, value.len() + evidence.len(), || {
            value = staged_value;
            evidence = staged_evidence;
        })
        .unwrap();
    assert_eq!((&value, &evidence), (&vec![1, 2], &vec![1, 2]));
    assert!(kernel.worklist().contains(&reader));
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let after = run.read_slot(&meta, &value, 2).unwrap();
    assert_eq!(after.version.get(), before.version.get() + 1);
    run.finish(LocalClosure::Stable).unwrap();
    assert!(matches!(
        kernel.resume(&mut token, &mut meter),
        Err(KernelError::NotRunning)
    ));
}

#[test]
fn commit_refusal_and_live_changes_do_not_install_staged_side_effects() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::<()>::new().unwrap();
    let s = scope(&mut kernel, &mut meter, 1, 0);
    let mut fork = kernel.fork(&mut meter).unwrap();
    fork.kernel().register(s, (), &mut meter).unwrap();
    let installed = Cell::new(false);
    meter.fail_on = Some(meter.calls + 2); // scan succeeds, final install refuses
    assert_eq!(
        kernel.commit_with(fork, &mut meter, 1, || installed.set(true)),
        Err(KernelError::Stopped(Stop::Work))
    );
    assert!(!installed.get());
    assert_eq!(kernel.statistics().jobs, 0);
    meter.fail_on = None;
    let fork = kernel.fork(&mut meter).unwrap();
    kernel.obligation(s, &mut meter).unwrap();
    assert_eq!(
        kernel.commit_with(fork, &mut meter, 1, || installed.set(true)),
        Err(KernelError::StaleFork)
    );
    assert!(!installed.get());
}

#[test]
fn two_forks_cannot_alias_their_new_handles_or_overwrite_a_committed_sibling() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::<()>::new().unwrap();
    let s = scope(&mut kernel, &mut meter, 1, 0);
    let mut left = kernel.fork(&mut meter).unwrap();
    let mut right = kernel.fork(&mut meter).unwrap();
    let a = left.kernel().register(s, (), &mut meter).unwrap();
    let b = right.kernel().register(s, (), &mut meter).unwrap();
    assert_ne!(a, b);
    kernel.commit(left, &mut meter).unwrap();
    assert_eq!(
        kernel.commit(right, &mut meter),
        Err(KernelError::StaleFork)
    );
    assert!(matches!(
        kernel.begin(b, &mut meter),
        Err(KernelError::InvalidJob)
    ));
    kernel
        .begin(a, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
}

#[test]
fn an_aborted_evaluation_token_cannot_resume_a_later_live_evaluation() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let s = scope(&mut kernel, &mut meter, 1, 0);
    let job = kernel.register(s, (), &mut meter).unwrap();
    let mut fork = kernel.fork(&mut meter).unwrap();
    let mut escaped = fork.kernel().begin(job, &mut meter).unwrap().suspend();
    drop(fork);
    let mut live = kernel.begin(job, &mut meter).unwrap().suspend();
    assert!(matches!(
        kernel.resume(&mut escaped, &mut meter),
        Err(KernelError::NotRunning)
    ));
    kernel
        .resume(&mut live, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
}

#[test]
fn nested_fork_from_an_uncommitted_sibling_cannot_replace_the_live_branch() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let s = scope(&mut kernel, &mut meter, 1, 0);
    let mut a = kernel.fork(&mut meter).unwrap();
    let mut b = kernel.fork(&mut meter).unwrap();
    let live_job = a.kernel().register(s, (), &mut meter).unwrap();
    b.kernel().register(s, (), &mut meter).unwrap();
    b.kernel().seal_scope(s, &mut meter).unwrap();
    let nested = b.view().fork(&mut meter).unwrap();
    kernel.commit(a, &mut meter).unwrap();
    assert_eq!(
        kernel.commit(nested, &mut meter),
        Err(KernelError::StaleFork)
    );
    kernel
        .begin(live_job, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
}
