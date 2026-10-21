use super::{
    contracts::{Bounds, IntersectIntervals, Interval, Variable},
    support::{Conflict, Ledger, RuleError, Stop, scope},
};
use analysis_kernel::{
    Accepts, Analyzer, Availability, Change, Closure, Cons, Context, Descending, Here, Kernel,
    KernelError, LocalClosure, Nil, PublishError, Reduction, Registered, Scope, Scoped,
};

#[derive(Clone, Copy)]
struct SeedInput {
    target: Scope,
    value: Interval,
}
struct Seed;
impl<Cx> Reduction<Cx> for Seed
where
    Cx: Accepts<Bounds, Here, Error = PublishError<Conflict, Stop>>,
{
    type Task = SeedInput;
    type Error = RuleError;
    fn reduce(&mut self, task: &SeedInput, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        cx.accept(Scoped::new(task.target, Variable::X), task.value)?;
        Ok(LocalClosure::Stable)
    }
}
fn seed(
    kernel: &mut Kernel<SeedInput>,
    stores: &mut Cons<analysis_kernel::Channel<Bounds, Descending<IntersectIntervals>>, Nil>,
    meter: &mut Ledger,
    job_scope: Scope,
    task: SeedInput,
) -> Result<(), RuleError> {
    let job = kernel.register(job_scope, task, meter).unwrap();
    let mut run = kernel.begin(job, meter).unwrap();
    Registered::new(Seed).evaluate(&task, &mut Context::new(&mut run, stores))?;
    run.finish(LocalClosure::Stable)?;
    Ok(())
}

#[test]
fn first_descending_write_requires_registered_authority_and_cannot_cross_generations() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::<SeedInput>::new().unwrap();
    let a = scope(&mut kernel, &mut meter, 10, 1);
    let b = scope(&mut kernel, &mut meter, 10, 2);
    let bounds = kernel
        .channel::<Bounds, _, _>(Descending(IntersectIntervals), &mut meter)
        .unwrap();
    let mut stores = Cons::new(bounds, Nil);
    let task = SeedInput {
        target: a,
        value: Interval::new(0, 1),
    };
    let normal = kernel.register(a, task, &mut meter).unwrap();
    let mut run = kernel.begin(normal, &mut meter).unwrap();
    assert_eq!(
        run.publish(&mut stores.head, Scoped::new(a, Variable::X), task.value),
        Err(PublishError::Kernel(KernelError::UnregisteredReduction))
    );
    assert!(stores.head.is_empty());
    run.finish(LocalClosure::Stable).unwrap();
    seed(&mut kernel, &mut stores, &mut meter, a, task).unwrap();
    assert!(matches!(
        seed(&mut kernel, &mut stores, &mut meter, b, task),
        Err(RuleError::Publication(PublishError::Kernel(
            KernelError::WrongScope
        )))
    ));
    assert!(matches!(kernel.closure(), Closure::Open(open) if open.interrupted == 1));
}

#[test]
fn replacement_uses_the_kernel_stable_entry_and_fresh_registered_seed() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::<SeedInput>::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 19, 0);
    let old = scope(&mut kernel, &mut meter, 20, 1);
    let bounds = kernel
        .channel::<Bounds, _, _>(Descending(IntersectIntervals), &mut meter)
        .unwrap();
    let mut stores = Cons::new(bounds, Nil);
    seed(
        &mut kernel,
        &mut stores,
        &mut meter,
        old,
        SeedInput {
            target: old,
            value: Interval::new(0, 0),
        },
    )
    .unwrap();
    let task = SeedInput {
        target: owner,
        value: Interval::new(0, 0),
    };
    let updater = kernel.register(owner, task, &mut meter).unwrap();
    let reader = kernel.register(owner, task, &mut meter).unwrap();
    let mut current = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let selected = *run.current_generation(&current).unwrap().value().unwrap();
    assert_eq!(
        run.query(&mut stores.head, Scoped::new(selected, Variable::X))
            .unwrap()
            .value(),
        Some(&Interval::new(0, 0))
    );
    run.finish(LocalClosure::Stable).unwrap();

    // Same-generation descending publication cannot restore a larger universe.
    seed(
        &mut kernel,
        &mut stores,
        &mut meter,
        old,
        SeedInput {
            target: old,
            value: Interval::new(0, 1),
        },
    )
    .unwrap();
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    assert_eq!(
        run.query(&mut stores.head, Scoped::new(old, Variable::X))
            .unwrap()
            .value(),
        Some(&Interval::new(0, 0)),
        "same-generation refinement cannot enlarge the original input"
    );
    run.finish(LocalClosure::Stable).unwrap();
    let gap = kernel.obligation(owner, &mut meter).unwrap();
    let new = scope(&mut kernel, &mut meter, 20, 2);
    seed(
        &mut kernel,
        &mut stores,
        &mut meter,
        new,
        SeedInput {
            target: new,
            value: Interval::new(0, 1),
        },
    )
    .unwrap();
    let mut run = kernel.begin(updater, &mut meter).unwrap();
    assert_eq!(
        run.replace_generation(&mut current, new),
        Err(KernelError::UnsealedScope)
    );
    run.finish(LocalClosure::Stable).unwrap();
    kernel.seal_scope(new, &mut meter).unwrap();
    let mut run = kernel.begin(updater, &mut meter).unwrap();
    assert_eq!(
        run.replace_generation(&mut current, new).unwrap(),
        Change::Changed
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(kernel.next(&mut meter).unwrap(), Some(reader));
    let mut run = kernel.begin(reader, &mut meter).unwrap();
    let selected = *run.current_generation(&current).unwrap().value().unwrap();
    assert_eq!(selected, new);
    assert_eq!(
        run.query(&mut stores.head, Scoped::new(selected, Variable::X))
            .unwrap()
            .state,
        Availability::Value(Interval::new(0, 1)),
        "must not inherit the old narrowed x=0"
    );
    assert_eq!(
        run.query(&mut stores.head, Scoped::new(old, Variable::X)),
        Err(KernelError::InactiveScope)
    );
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(
        kernel.scope(old.key(), &mut meter),
        Err(KernelError::InactiveScope)
    );
    kernel.resolve_obligation(gap, &mut meter).unwrap();
    kernel.seal_scope(owner, &mut meter).unwrap();
    assert_eq!(kernel.closure(), Closure::Closed);
}

#[test]
fn generation_selector_has_one_owner_and_clones_observe_authoritative_replacement() {
    let mut meter = Ledger::default();
    let mut kernel = Kernel::<()>::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 81, 0);
    let other_owner = scope(&mut kernel, &mut meter, 82, 0);
    let old = scope(&mut kernel, &mut meter, 83, 1);
    let other = scope(&mut kernel, &mut meter, 83, 2);
    let next = scope(&mut kernel, &mut meter, 83, 3);
    for generation in [old, other, next] {
        kernel.seal_scope(generation, &mut meter).unwrap();
    }
    let mut entry = kernel.generation_entry(owner, old, &mut meter).unwrap();
    let cloned = entry.clone();
    assert!(matches!(
        kernel.generation_entry(other_owner, old, &mut meter),
        Err(KernelError::GenerationInUse)
    ));
    assert_eq!(
        kernel.retire_scope(old, &mut meter),
        Err(KernelError::GenerationInUse)
    );
    assert_eq!(
        kernel.retire_scope(owner, &mut meter),
        Err(KernelError::GenerationInUse)
    );
    let other_entry = kernel
        .generation_entry(other_owner, other, &mut meter)
        .unwrap();
    let writer = kernel.register(owner, (), &mut meter).unwrap();
    let mut run = kernel.begin(writer, &mut meter).unwrap();
    let before = run.current_generation(&entry).unwrap();
    assert_eq!(
        run.replace_generation(&mut entry, other),
        Err(KernelError::GenerationInUse)
    );
    assert_eq!(run.current_generation(&cloned).unwrap(), before);
    assert_eq!(
        run.current_generation(&other_entry).unwrap().value(),
        Some(&other)
    );
    run.replace_generation(&mut entry, next).unwrap();
    assert_eq!(
        run.current_generation(&cloned).unwrap().value(),
        Some(&next)
    );
    assert_eq!(run.current_generation(&entry).unwrap().value(), Some(&next));
    run.finish(LocalClosure::Stable).unwrap();
    assert_eq!(
        kernel.scope(old.key(), &mut meter),
        Err(KernelError::InactiveScope)
    );
    assert_eq!(
        kernel.retire_scope(next, &mut meter),
        Err(KernelError::GenerationInUse)
    );
}
