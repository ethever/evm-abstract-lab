use super::{Budget, Stop, scope};
use crate::{Change, Closure, GenerationEntry, Kernel, KernelError, LocalClosure, Scope};
use std::cell::Cell;

fn observe(
    kernel: &mut Kernel<()>,
    meter: &mut Budget,
    owner: Scope,
    entry: &GenerationEntry,
) -> Scope {
    let reader = kernel.register(owner, (), meter).unwrap();
    let mut run = kernel.begin(reader, meter).unwrap();
    let current = *run.current_generation(entry).unwrap().value().unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    current
}

#[test]
fn duplicate_constructor_from_the_public_probe_is_rejected_and_shared_readers_wake() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 1, 0);
    let old = scope(&mut kernel, &mut meter, 2, 0);
    let next = scope(&mut kernel, &mut meter, 2, 1);
    let mut first = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let before = kernel.statistics();
    assert!(matches!(
        kernel.generation_entry(owner, old, &mut meter),
        Err(KernelError::GenerationInUse)
    ));
    assert_eq!(
        kernel.statistics(),
        before,
        "no second selector is allocated"
    );
    let second = first.clone(); // Share one authoritative selector, rather than construct another.
    let job = kernel.register(owner, (), &mut meter).unwrap();
    let reader = kernel.register(owner, (), &mut meter).unwrap();
    for s in [owner, old, next] {
        kernel.seal_scope(s, &mut meter).unwrap();
    }
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let before = run.current_generation(&second).unwrap();
    assert_eq!(before.value(), Some(&old));
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(job, &mut meter).unwrap();
    run.replace_generation(&mut first, next).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(kernel.worklist().contains(&reader));
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let after = run.current_generation(&second).unwrap();
    assert_eq!(after.value(), Some(&next));
    assert!(after.version > before.version);
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
}

#[test]
fn refused_replacement_preserves_both_independent_chains_and_can_be_retried() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 1, 0);
    let other_owner = scope(&mut kernel, &mut meter, 3, 0);
    let old = scope(&mut kernel, &mut meter, 2, 0);
    let occupied = scope(&mut kernel, &mut meter, 2, 1);
    let free = scope(&mut kernel, &mut meter, 2, 2);
    let mut first = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let second = kernel
        .generation_entry(other_owner, occupied, &mut meter)
        .unwrap();
    for s in [owner, other_owner, old, occupied, free] {
        kernel.seal_scope(s, &mut meter).unwrap();
    }
    let writer = kernel.register(owner, (), &mut meter).unwrap();
    let reader = kernel.register(owner, (), &mut meter).unwrap();
    let other_reader = kernel.register(other_owner, (), &mut meter).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let before = run.current_generation(&first).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(other_reader, &mut meter).unwrap();
    let other_before = run.current_generation(&second).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    assert_eq!(
        run.replace_generation(&mut first, occupied),
        Err(KernelError::GenerationInUse)
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!kernel.worklist().contains(&reader));
    assert!(!kernel.worklist().contains(&other_reader));
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(run.current_generation(&first).unwrap(), before);
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(other_reader, &mut meter).unwrap();
    assert_eq!(run.current_generation(&second).unwrap(), other_before);
    run.finish(LocalClosure::Stable).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    assert_eq!(
        run.replace_generation(&mut first, free).unwrap(),
        Change::Changed
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert!(kernel.worklist().contains(&reader));
    assert!(!kernel.worklist().contains(&other_reader));
    assert_eq!(observe(&mut kernel, &mut meter, owner, &first), free);
    assert_eq!(
        observe(&mut kernel, &mut meter, other_owner, &second),
        occupied
    );
    assert_eq!(kernel.scope(occupied.key(), &mut meter).unwrap(), occupied);
}

#[test]
fn stable_owners_and_selected_scopes_cannot_be_repurposed_or_retired_directly() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 2, 9);
    let old = scope(&mut kernel, &mut meter, 2, 0);
    let next_owner = scope(&mut kernel, &mut meter, 2, 1);
    let child = scope(&mut kernel, &mut meter, 7, 0);
    let other = scope(&mut kernel, &mut meter, 3, 0);
    assert!(matches!(
        kernel.generation_entry(owner, owner, &mut meter),
        Err(KernelError::WrongScope)
    ));
    let mut entry = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let _nested = kernel
        .generation_entry(next_owner, child, &mut meter)
        .unwrap();
    assert!(matches!(
        kernel.generation_entry(old, other, &mut meter),
        Err(KernelError::GenerationInUse)
    ));
    assert!(matches!(
        kernel.generation_entry(other, owner, &mut meter),
        Err(KernelError::GenerationInUse)
    ));
    for s in [owner, old, next_owner, child, other] {
        kernel.seal_scope(s, &mut meter).unwrap();
    }
    assert_eq!(
        kernel.retire_scope(old, &mut meter),
        Err(KernelError::GenerationInUse)
    );
    assert_eq!(
        kernel.retire_scope(owner, &mut meter),
        Err(KernelError::GenerationInUse)
    );
    let writer = kernel.register(owner, (), &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    let before = run.current_generation(&entry).unwrap();
    assert_eq!(
        run.replace_generation(&mut entry, owner),
        Err(KernelError::WrongScope)
    );
    assert_eq!(
        run.replace_generation(&mut entry, next_owner),
        Err(KernelError::GenerationInUse)
    );
    assert_eq!(run.current_generation(&entry).unwrap(), before);
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!kernel.worklist().contains(&writer));
    assert_eq!(kernel.closure(), Closure::Closed);
}

#[test]
fn refused_entry_initialization_never_claims_either_scope() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::<()>::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 1, 0);
    let old = scope(&mut kernel, &mut meter, 2, 0);
    let before = kernel.statistics();
    meter.fail_on = Some(meter.calls + 2); // ownership reserve succeeds; slot allocation refuses.
    assert!(matches!(
        kernel.generation_entry(owner, old, &mut meter),
        Err(KernelError::Stopped(Stop::Work))
    ));
    assert_eq!(kernel.statistics(), before);
    meter.fail_on = None;
    // Swapping the roles would be forbidden if either failed claim leaked.
    kernel.generation_entry(old, owner, &mut meter).unwrap();
}

#[test]
fn refused_replacement_publication_keeps_ownership_versions_and_wakes_atomic() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 1, 0);
    let old = scope(&mut kernel, &mut meter, 2, 0);
    let next = scope(&mut kernel, &mut meter, 2, 1);
    let mut entry = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let writer = kernel.register(owner, (), &mut meter).unwrap();
    let readers: Vec<_> = (0..3)
        .map(|_| kernel.register(owner, (), &mut meter).unwrap())
        .collect();
    for s in [owner, old, next] {
        kernel.seal_scope(s, &mut meter).unwrap();
    }
    for reader in &readers {
        let mut run = kernel.begin(*reader, &mut meter).unwrap();
        run.current_generation(&entry).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
    }
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    let before = run.current_generation(&entry).unwrap();
    run.meter().fail_on = Some(run.meter().calls + 2); // paid retirement quote; refuse publication/wakes
    assert_eq!(
        run.replace_generation(&mut entry, next),
        Err(KernelError::Stopped(Stop::Work))
    );
    assert_eq!(run.current_generation(&entry.clone()).unwrap(), before);
    drop(run);
    assert!(matches!(kernel.closure(), Closure::Open(o) if o.interrupted == 1));
    assert!(
        readers
            .iter()
            .all(|reader| !kernel.worklist().contains(reader))
    );
    assert!(!kernel.worklist().contains(&writer));
    assert_eq!(kernel.scope(old.key(), &mut meter).unwrap(), old);
    meter.fail_on = None;
    kernel.retry(writer, &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    run.replace_generation(&mut entry, next).unwrap();
    let after = run.current_generation(&entry).unwrap();
    assert_eq!(after.value(), Some(&next));
    assert!(after.version > before.version);
    run.finish(LocalClosure::Stable).unwrap();
    assert!(
        readers
            .iter()
            .all(|reader| kernel.worklist().contains(reader))
    );
    assert!(kernel.worklist().contains(&writer));
    assert_eq!(
        kernel.retire_scope(next, &mut meter),
        Err(KernelError::GenerationInUse)
    );
}

#[test]
fn aborted_fork_keeps_live_selection_and_does_not_leak_scope_ownership() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 1, 0);
    let old = scope(&mut kernel, &mut meter, 2, 0);
    let next = scope(&mut kernel, &mut meter, 2, 1);
    let mut entry = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let writer = kernel.register(owner, (), &mut meter).unwrap();
    kernel.seal_scope(next, &mut meter).unwrap();
    let mut fork = kernel.fork(&mut meter).unwrap();
    let mut staged = entry.clone();
    let mut run = fork.kernel().begin(writer, &mut meter).unwrap();
    run.replace_generation(&mut staged, next).unwrap();
    assert_eq!(run.current_generation(&entry).unwrap().value(), Some(&next));
    run.finish(LocalClosure::Stable).unwrap();
    drop(fork);
    assert_eq!(observe(&mut kernel, &mut meter, owner, &staged), old);
    assert_eq!(kernel.scope(old.key(), &mut meter).unwrap(), old);
    // The root replacement succeeds: the abandoned fork did not claim `next`.
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    run.replace_generation(&mut entry, next).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
}

#[test]
fn fork_commit_advances_every_shared_handle_and_preserves_exclusive_ownership() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 1, 0);
    let old = scope(&mut kernel, &mut meter, 2, 0);
    let next = scope(&mut kernel, &mut meter, 2, 1);
    let entry = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let reader = kernel.register(owner, (), &mut meter).unwrap();
    let writer = kernel.register(owner, (), &mut meter).unwrap();
    kernel.seal_scope(next, &mut meter).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    run.current_generation(&entry).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut fork = kernel.fork(&mut meter).unwrap();
    let mut staged = entry.clone();
    let mut run = fork.kernel().begin(writer, &mut meter).unwrap();
    run.replace_generation(&mut staged, next).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    assert!(!kernel.worklist().contains(&reader));
    kernel.commit(fork, &mut meter).unwrap();
    assert!(kernel.worklist().contains(&reader));
    assert_eq!(observe(&mut kernel, &mut meter, owner, &entry), next);
    assert_eq!(
        kernel.retire_scope(next, &mut meter),
        Err(KernelError::GenerationInUse)
    );
    assert!(matches!(
        kernel.generation_entry(owner, next, &mut meter),
        Err(KernelError::GenerationInUse)
    ));
}

#[test]
fn refused_fork_commit_preserves_live_generation_and_notifies_no_reader() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 1, 0);
    let old = scope(&mut kernel, &mut meter, 2, 0);
    let next = scope(&mut kernel, &mut meter, 2, 1);
    let entry = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let writer = kernel.register(owner, (), &mut meter).unwrap();
    let reader = kernel.register(owner, (), &mut meter).unwrap();
    kernel.seal_scope(next, &mut meter).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let before = run.current_generation(&entry).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let mut fork = kernel.fork(&mut meter).unwrap();
    let mut staged = entry.clone();
    let mut run = fork.kernel().begin(writer, &mut meter).unwrap();
    run.replace_generation(&mut staged, next).unwrap();
    run.finish(LocalClosure::Stable).unwrap();
    let installed = Cell::new(false);
    meter.fail_on = Some(meter.calls + 2);
    assert_eq!(
        kernel.commit_with(fork, &mut meter, 1, || installed.set(true)),
        Err(KernelError::Stopped(Stop::Work))
    );
    assert!(!installed.get());
    assert!(!kernel.worklist().contains(&reader));
    meter.fail_on = None;
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(run.current_generation(&entry).unwrap(), before);
    run.finish(LocalClosure::Stable).unwrap();
    // The refused commit did not bind `next` in the live graph.
    kernel.generation_entry(owner, next, &mut meter).unwrap();
}
