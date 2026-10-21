use super::*;
use crate::{Change, Closure, KernelError, Nil, PublishError, SolveError, solve};

#[derive(Clone, Copy)]
enum Task {
    Owner,
    Discover,
    Renew,
}
struct Discover {
    world: Scope,
    channel: Inputs,
    stop_after_publication: Option<Stop>,
}
impl<'r, 'k> Analyzer<Context<'r, 'k, Task, Budget, Nil>> for Discover {
    type Task = Task;
    type Error = KernelError<Stop>;
    fn evaluate(
        &mut self,
        task: &Task,
        cx: &mut Context<'r, 'k, Task, Budget, Nil>,
    ) -> Result<LocalClosure, Self::Error> {
        if matches!(task, Task::Owner) {
            return Ok(LocalClosure::Stable);
        }
        if matches!(task, Task::Discover) {
            assert!(cx.run().keys(&mut self.channel, self.world)?.is_empty());
        }
        let changed = cx
            .run()
            .publish(&mut self.channel, Scoped::new(self.world, 0), 1)
            .map_err(|error| match error {
                PublishError::Kernel(error) => error,
                PublishError::Policy(never) => match never {},
            })?;
        assert_eq!(changed, Change::Changed);
        match self.stop_after_publication {
            Some(Stop::Work) => {
                let meter = cx.run().meter();
                meter.fail_on = Some(meter.calls + 1);
            }
            Some(Stop::Cancelled) => cx.run().meter().cancelled.set(true),
            None => {}
        }
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn generic_solver_exposes_recoverable_self_invalidation_and_preserves_real_root_stops() {
    for stopped in [None, Some(Stop::Work), Some(Stop::Cancelled)] {
        let mut kernel = Kernel::new().unwrap();
        let mut meter = Budget::default();
        let world = scope(&mut kernel, &mut meter, 10, 0);
        let owner = scope(&mut kernel, &mut meter, 11, 0);
        let old = scope(&mut kernel, &mut meter, 12, 0);
        let coordinator = kernel.register(owner, Task::Owner, &mut meter).unwrap();
        kernel.register(old, Task::Discover, &mut meter).unwrap();
        let mut entry = kernel.generation_entry(owner, old, &mut meter).unwrap();
        let mut run = kernel.begin(coordinator, &mut meter).unwrap();
        run.current_generation(&entry).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        for scope in [world, owner, old] {
            kernel.seal_scope(scope, &mut meter).unwrap();
        }
        let channel = kernel.channel(Ascending(Union), &mut meter).unwrap();
        let mut analyzer = Discover {
            world,
            channel,
            stop_after_publication: stopped,
        };
        let expected = stopped.map_or(KernelError::StaleGeneration, KernelError::Stopped);
        assert_eq!(
            solve(&mut kernel, &mut analyzer, &mut Nil, &mut meter, 1),
            Err(SolveError::Kernel(expected))
        );
        assert!(kernel.worklist().contains(&coordinator));
        assert!(matches!(kernel.closure(), Closure::Open(o) if o.obligations == 1));
        meter.cancelled.set(false);
        meter.fail_on = None;
        assert!(kernel.scope_stale(old, &mut meter).unwrap());
        let next = scope(&mut kernel, &mut meter, 12, 1);
        kernel.register(next, Task::Renew, &mut meter).unwrap();
        kernel.seal_scope(next, &mut meter).unwrap();
        let mut run = kernel.begin(coordinator, &mut meter).unwrap();
        run.replace_generation(&mut entry, next).unwrap();
        run.finish(LocalClosure::Stable).unwrap();
        analyzer.stop_after_publication = None;
        let report = solve(&mut kernel, &mut analyzer, &mut Nil, &mut meter, 8).unwrap();
        assert_eq!(report.closure, Closure::Closed);
        assert!(!kernel.scope_stale(next, &mut meter).unwrap());
    }
}
