use super::support::{Conflict, Ledger, Stop, scope};
use analysis_kernel::{
    AdmissionPolicy, Change, Closure, Kernel, KernelError, LocalClosure, Prepared, PublishError,
};
use std::cell::Cell;

struct Cover<'a>(&'a Cell<usize>);
impl AdmissionPolicy for Cover<'_> {
    type Value = u32;
    type Metadata = Vec<u32>;
    type Error = Conflict;
    fn work(&self, _: &u32, _: &u32, history: &Vec<u32>) -> usize {
        16 + history.len()
    }
    fn prepare(
        &self,
        current: &u32,
        incoming: &u32,
        history: &Vec<u32>,
    ) -> Result<Prepared<u32, Vec<u32>>, Conflict> {
        self.0.set(self.0.get() + 1);
        let value = current | incoming;
        let mut metadata = history.clone();
        metadata.push(*incoming);
        Ok(Prepared {
            value,
            metadata,
            change: if value == *current {
                Change::Unchanged
            } else {
                Change::Changed
            },
            install_work: 16,
        })
    }
}

#[test]
fn admission_failure_after_pure_preparation_does_not_install_separate_evidence_or_value() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 41, 0);
    let slot = kernel.slot::<u32, _>(scope, &mut meter).unwrap();
    let writer = kernel.register(scope, (), &mut meter).unwrap();
    let reader = kernel.register(scope, (), &mut meter).unwrap();
    let mut value = 1;
    let mut metadata = vec![1];
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let before = run.read_slot(&slot, &value, 1).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let preparations = Cell::new(0);
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    let fail_at = run.meter().calls + 2; // Paid prepare, then final atomic installation.
    run.meter().deny_call = Some(fail_at);
    assert_eq!(
        run.admit(&slot, &mut value, &mut metadata, &2, &Cover(&preparations)),
        Err(PublishError::Kernel(KernelError::Stopped(Stop::Work)))
    );
    assert_eq!(
        preparations.get(),
        1,
        "failure must occur after pure preparation"
    );
    assert_eq!((value, &metadata), (1, &vec![1]));
    assert_eq!(run.slot_version(&slot).unwrap(), before.version);
    drop(run);
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.interrupted == 1));
    meter.deny_call = None;
    kernel.retry(writer, &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    run.admit(&slot, &mut value, &mut metadata, &2, &Cover(&preparations))
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!((value, metadata), (3, vec![1, 2]));
    assert_eq!(kernel.next(&mut meter).unwrap(), Some(reader));
    kernel
        .begin(reader, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
}

#[test]
fn separate_borrowed_fields_share_one_atomic_install_and_failed_callback_is_never_invoked() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 42, 0);
    let slot = kernel.slot::<u32, _>(scope, &mut meter).unwrap();
    let writer = kernel.register(scope, (), &mut meter).unwrap();
    let mut value = 1;
    let mut evidence = vec![1];
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    let version = run.slot_version(&slot).unwrap();
    run.meter().deny_next = true;
    assert_eq!(
        run.publish_prepared_parts(
            &slot,
            &mut value,
            3,
            &mut evidence,
            vec![1, 2],
            Change::Changed,
            16
        ),
        Err(KernelError::Stopped(Stop::Work))
    );
    assert_eq!((value, &evidence), (1, &vec![1]));
    assert_eq!(run.slot_version(&slot).unwrap(), version);
    let installed = Cell::new(false);
    run.meter().deny_next = true;
    assert_eq!(
        run.publish_prepared_with(&slot, Change::Changed, 4, || installed.set(true)),
        Err(KernelError::Stopped(Stop::Work))
    );
    assert!(!installed.get());
    drop(run);
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.interrupted == 1));
}

#[test]
fn unpublished_fork_failure_preserves_live_graph_and_aborted_job_handles_never_alias() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 43, 0);
    let slot = kernel.slot::<u32, _>(scope, &mut meter).unwrap();
    let writer = kernel.register(scope, (), &mut meter).unwrap();
    let mut live_value = 1;
    let mut fork = kernel.fork(&mut meter).unwrap();
    let leaked = fork.kernel().register(scope, (), &mut meter).unwrap();
    let mut staged_value = live_value;
    let mut run = fork.kernel().begin(writer, &mut meter).unwrap();
    run.publish_prepared(&slot, &mut staged_value, 3, Change::Changed, 1)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let used = meter.used;
    meter.deny_next = true;
    assert_eq!(
        kernel.commit_with(fork, &mut meter, 1, || live_value = staged_value),
        Err(KernelError::Stopped(Stop::Work))
    );
    assert_eq!(live_value, 1);
    assert_eq!(
        meter.used, used,
        "failed final reservation refunds no previously paid fork work"
    );
    let replacement = kernel.register(scope, (), &mut meter).unwrap();
    assert_ne!(leaked, replacement);
    assert_eq!(kernel.task::<Stop>(leaked), Err(KernelError::InvalidJob));
    let mut fork = kernel.fork(&mut meter).unwrap();
    let mut staged_value = live_value;
    let mut run = fork.kernel().begin(writer, &mut meter).unwrap();
    run.publish_prepared(&slot, &mut staged_value, 3, Change::Changed, 1)
        .unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    kernel
        .commit_with(fork, &mut meter, 1, || live_value = staged_value)
        .unwrap();
    assert_eq!(live_value, 3);
}

#[test]
fn suspended_parent_can_observe_nested_work_without_closing_its_own_obligation() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 44, 0);
    let child_scope = scope(&mut kernel, &mut meter, 45, 0);
    let parent = kernel.register(owner, (), &mut meter).unwrap();
    let child = kernel.register(child_scope, (), &mut meter).unwrap();
    kernel.seal_scope(owner, &mut meter).unwrap();
    kernel.seal_scope(child_scope, &mut meter).unwrap();
    let mut token = kernel.begin(parent, &mut meter).unwrap().suspend();
    kernel
        .begin(child, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
    assert_eq!(
        kernel.closure_in::<Stop>(child_scope).unwrap(),
        Closure::Closed
    );
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.in_flight == 1));
    kernel
        .resume(&mut token, &mut meter)
        .unwrap()
        .finish(LocalClosure::Stable)
        .unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
}
