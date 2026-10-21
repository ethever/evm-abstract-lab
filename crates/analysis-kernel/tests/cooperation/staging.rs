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
    let readers: Vec<_> = (0..4)
        .map(|_| kernel.register(scope, (), &mut meter).unwrap())
        .collect();
    kernel.seal_scope(scope, &mut meter).unwrap();
    let mut value = 1;
    let mut metadata = vec![1];
    for reader in &readers {
        let mut run = kernel.begin(*reader, &mut meter).unwrap();
        run.read_slot(&slot, &value, 1).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    let preparations = Cell::new(0);
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    let before = run.read_slot(&slot, &value, 1).unwrap();
    let used = run.meter().used;
    run.meter().stop_after(2, Stop::Work); // Paid preparation, refused installation.
    assert_eq!(
        run.admit(&slot, &mut value, &mut metadata, &2, &Cover(&preparations)),
        Err(PublishError::Kernel(KernelError::Stopped(Stop::Work)))
    );
    assert_eq!(preparations.get(), 1, "refuse after pure preparation");
    assert_eq!((value, metadata.as_slice()), (1, &[1][..]));
    assert_eq!(run.slot_version(&slot).unwrap(), before.version);
    assert!(
        run.meter().used > used,
        "successful reservations are not refunded"
    );
    drop(run);
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.interrupted == 1));
    assert!(
        readers
            .iter()
            .all(|reader| !kernel.worklist().contains(reader))
    );
    assert!(!kernel.worklist().contains(&writer));
    meter.clear_stop();
    kernel.retry(writer, &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    assert_eq!(
        run.admit(&slot, &mut value, &mut metadata, &2, &Cover(&preparations))
            .unwrap(),
        Change::Changed
    );
    assert_eq!(
        run.slot_version(&slot).unwrap().get(),
        before.version.get() + 1
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!((value, metadata), (3, vec![1, 2]));
    assert_eq!(preparations.get(), 2);
    assert!(
        readers
            .iter()
            .all(|reader| kernel.worklist().contains(reader))
    );
    assert!(kernel.worklist().contains(&writer));
    let mut wakes = 0;
    while let Some(job) = kernel.next(&mut meter).unwrap() {
        let mut run = kernel.begin(job, &mut meter).unwrap();
        assert_eq!(run.read_slot(&slot, &value, 1).unwrap().value(), Some(&3));
        run.finish(LocalClosure::Stable).unwrap();
        wakes += 1;
    }
    assert_eq!(
        wakes, 5,
        "four external readers and the self-reading writer"
    );
    assert_eq!(kernel.closure(), Closure::Closed);
}

#[test]
fn metadata_only_admission_does_not_bump_semantic_version_or_wake() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::new().unwrap();
    let scope = scope(&mut kernel, &mut meter, 41, 0);
    let job = kernel.register(scope, (), &mut meter).unwrap();
    let slot = kernel.slot(scope, &mut meter).unwrap();
    let mut value = 3;
    let mut history = vec![3];
    let preparations = Cell::new(0);
    let mut run = kernel.begin(job, &mut meter).unwrap();
    let before = run.read_slot(&slot, &value, 1).unwrap().version;
    assert_eq!(
        run.admit(&slot, &mut value, &mut history, &2, &Cover(&preparations))
            .unwrap(),
        Change::Unchanged
    );
    assert_eq!(run.slot_version(&slot).unwrap(), before);
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!((value, history), (3, vec![3, 2]));
    assert_eq!(preparations.get(), 1);
    assert!(!kernel.worklist().contains(&job));
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
    run.meter().stop_after(1, Stop::Work);
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
    run.meter().stop_after(1, Stop::Work);
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
    meter.stop_after(1, Stop::Work);
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
