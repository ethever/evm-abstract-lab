use super::{Budget, Stop, scope};
use crate::{
    AdmissionPolicy, Change, Closure, Kernel, KernelError, LocalClosure, Prepared, PublishError,
};
use std::convert::Infallible;

struct Cover;
impl AdmissionPolicy for Cover {
    type Value = u32;
    type Metadata = Vec<u32>;
    type Error = Infallible;
    fn work(&self, _: &u32, _: &u32, history: &Vec<u32>) -> usize {
        history.len() + 4
    }
    fn prepare(
        &self,
        current: &u32,
        incoming: &u32,
        history: &Vec<u32>,
    ) -> Result<Prepared<u32, Vec<u32>>, Infallible> {
        let value = (*current).max(*incoming);
        let mut metadata = history.clone();
        metadata.push(*incoming);
        Ok(Prepared {
            value,
            change: if value == *current {
                Change::Unchanged
            } else {
                Change::Changed
            },
            install_work: history.len() + metadata.len() + 2,
            metadata,
        })
    }
}

#[test]
fn covering_admission_keeps_history_value_version_and_fanout_atomic() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let s = scope(&mut kernel, &mut meter, 1, 0);
    let writer = kernel.register(s, (), &mut meter).unwrap();
    let readers: Vec<_> = (0..4)
        .map(|_| kernel.register(s, (), &mut meter).unwrap())
        .collect();
    let slot = kernel.slot(s, &mut meter).unwrap();
    let mut value = 3;
    let mut history = vec![3];
    kernel.seal_scope(s, &mut meter).unwrap();
    for job in &readers {
        let mut run = kernel.begin(*job, &mut meter).unwrap();
        run.read_slot(&slot, &value, 1).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    let before = run.read_slot(&slot, &value, 1).unwrap().version;
    let used = run.meter().used;
    run.meter().fail_on = Some(run.meter().calls + 2); // paid prepare, refused all installs/wakes
    assert_eq!(
        run.admit(&slot, &mut value, &mut history, &9, &Cover),
        Err(PublishError::Kernel(KernelError::Stopped(Stop::Work)))
    );
    assert_eq!((value, history.as_slice()), (3, &[3][..]));
    assert_eq!(run.slot_version(&slot).unwrap(), before);
    assert!(run.meter().used > used);
    drop(run);
    assert!(readers.iter().all(|r| !kernel.worklist().contains(r)));
    assert!(matches!(kernel.closure(), Closure::Open(o) if o.interrupted == 1));
    meter.fail_on = None;
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    assert_eq!(
        run.admit(&slot, &mut value, &mut history, &9, &Cover)
            .unwrap(),
        Change::Changed
    );
    assert_eq!(run.slot_version(&slot).unwrap().get(), before.get() + 1);
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!((value, history), (9, vec![3, 9]));
    assert!(readers.iter().all(|r| kernel.worklist().contains(r)));
    assert!(
        kernel.worklist().contains(&writer),
        "self-read also schedules the next evaluation"
    );
}

#[test]
fn metadata_only_admission_does_not_bump_semantic_version_or_wake() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let s = scope(&mut kernel, &mut meter, 1, 0);
    let job = kernel.register(s, (), &mut meter).unwrap();
    let slot = kernel.slot(s, &mut meter).unwrap();
    let mut value = 3;
    let mut history = vec![3];
    let mut run = kernel.begin(job, &mut meter).unwrap();
    let before = run.read_slot(&slot, &value, 1).unwrap().version;
    assert_eq!(
        run.admit(&slot, &mut value, &mut history, &2, &Cover)
            .unwrap(),
        Change::Unchanged
    );
    assert_eq!(run.slot_version(&slot).unwrap(), before);
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(history, [3, 2]);
    assert!(!kernel.worklist().contains(&job));
}
