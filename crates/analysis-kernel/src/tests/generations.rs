use super::{Budget, Stop, scope};
use crate::{Change, Closure, Kernel, KernelError, LocalClosure};

#[test]
fn stable_entry_discovers_fresh_input_instead_of_reusing_old_narrowed_guarantees() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 1, 0);
    let old = scope(&mut kernel, &mut meter, 2, 0);
    let reader = kernel.register(owner, (), &mut meter).unwrap();
    let coordinator = kernel.register(owner, (), &mut meter).unwrap();
    let old_rule = kernel.register(old, (), &mut meter).unwrap();
    let mut entry = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let old_fact = kernel.slot(old, &mut meter).unwrap();
    let old_value = vec![0];
    let mut run = kernel.begin(old_rule, &mut meter).unwrap();
    run.read_slot(&old_fact, &old_value, 1).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(run.current_generation(&entry).unwrap().value(), Some(&old));
    run.read_slot(&old_fact, &old_value, 1).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let edge_count = kernel.dependency_count();
    let obligation = kernel.obligation(owner, &mut meter).unwrap();
    let next = scope(&mut kernel, &mut meter, 2, 1);
    let new_fact = kernel.slot(next, &mut meter).unwrap();
    let new_value = vec![0, 1]; // Fresh seed retains the widened input universe.
    let mut run = kernel.begin(coordinator, &mut meter).unwrap();
    assert_eq!(
        run.replace_generation(&mut entry, next),
        Err(KernelError::UnsealedScope)
    );
    run.finish(LocalClosure::Stable).unwrap();
    kernel.seal_scope(next, &mut meter).unwrap();
    let mut run = kernel.begin(coordinator, &mut meter).unwrap();
    assert_eq!(
        run.replace_generation(&mut entry, next).unwrap(),
        Change::Changed
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(
        kernel.dependency_count() < edge_count,
        "retirement releases old jobs' read edges"
    );
    assert!(matches!(kernel.closure(), Closure::Open(o) if o.obligations == 1));
    assert!(kernel.worklist().contains(&reader));
    assert_eq!(
        kernel.register(old, (), &mut meter),
        Err(KernelError::InactiveScope)
    );
    assert_eq!(
        kernel.scope(old.key(), &mut meter),
        Err(KernelError::InactiveScope)
    );
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(run.current_generation(&entry).unwrap().value(), Some(&next));
    assert_eq!(
        run.read_slot(&old_fact, &old_value, 1),
        Err(KernelError::InactiveScope)
    );
    assert_eq!(
        run.read_slot(&new_fact, &new_value, 2).unwrap().value(),
        Some(&vec![0, 1])
    );
    run.finish(LocalClosure::Stable).unwrap();
    kernel.resolve_obligation(obligation, &mut meter).unwrap();
    kernel.seal_scope(owner, &mut meter).unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
}

#[test]
fn failed_selector_publication_keeps_old_generation_and_entry_live() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 1, 0);
    let old = scope(&mut kernel, &mut meter, 2, 0);
    let next = scope(&mut kernel, &mut meter, 2, 1);
    kernel.seal_scope(next, &mut meter).unwrap();
    let job = kernel.register(owner, (), &mut meter).unwrap();
    let mut entry = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let mut run = kernel.begin(job, &mut meter).unwrap();
    let before = run.current_generation(&entry).unwrap();
    run.meter().fail_on = Some(run.meter().calls + 2); // retirement quote, combined publication
    assert_eq!(
        run.replace_generation(&mut entry, next),
        Err(KernelError::Stopped(Stop::Work))
    );
    run.meter().fail_on = None;
    assert_eq!(run.current_generation(&entry).unwrap(), before);
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.scope(old.key(), &mut meter).unwrap(), old);
}

#[test]
fn each_kernel_epoch_rejects_other_composition_handles_even_with_identical_keys() {
    let mut meter = Budget::default();
    let mut left = Kernel::<()>::new().unwrap();
    let mut right = Kernel::<()>::new().unwrap();
    let a = scope(&mut left, &mut meter, 1, 0);
    let b = scope(&mut right, &mut meter, 1, 0);
    assert_eq!(a.key(), b.key());
    assert_ne!(a.epoch(), b.epoch());
    assert_eq!(
        right.register(a, (), &mut meter),
        Err(KernelError::WrongEpoch)
    );
    let meta = left.slot(a, &mut meter).unwrap();
    let job = right.register(b, (), &mut meter).unwrap();
    let mut run = right.begin(job, &mut meter).unwrap();
    assert_eq!(run.read_slot(&meta, &1, 1), Err(KernelError::WrongEpoch));
}

#[test]
fn repeated_generation_replacement_releases_old_cross_scope_dependencies() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 1, 0);
    let first = scope(&mut kernel, &mut meter, 2, 0);
    let reader = kernel.register(owner, (), &mut meter).unwrap();
    let writer = kernel.register(owner, (), &mut meter).unwrap();
    let mut entry = kernel.generation_entry(owner, first, &mut meter).unwrap();
    let mut current = first;
    for generation in 1..=16 {
        let meta = kernel.slot(current, &mut meter).unwrap();
        let mut run = kernel.begin(reader, &mut meter).unwrap();
        run.current_generation(&entry).unwrap();
        run.read_slot(&meta, &generation, 1).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        assert_eq!(kernel.dependency_count(), 4);
        let next = scope(&mut kernel, &mut meter, 2, generation);
        kernel.seal_scope(next, &mut meter).unwrap();
        let mut run = kernel.begin(writer, &mut meter).unwrap();
        run.replace_generation(&mut entry, next).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        assert_eq!(
            kernel.dependency_count(),
            2,
            "only stable owner+entry edges remain"
        );
        current = next;
    }
}
