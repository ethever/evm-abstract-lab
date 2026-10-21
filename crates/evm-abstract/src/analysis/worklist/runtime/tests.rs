use super::{Runtime, Task, TaskKind};
use crate::{analysis::worklist::MachineMeter, resource::WorkBudget};
use analysis_kernel::{Change, Closure, ScopeKey};

#[test]
fn interruption_retains_every_task_family_and_a_self_dirty_rerun() {
    for dirty in [false, true] {
        const WORK: usize = 1_000_000;
        let mut budget = WorkBudget::new(WORK);
        let mut runtime = Runtime::<u8>::new(&mut budget).unwrap();
        runtime.register_world(0, 1, &mut budget).unwrap();
        let (_, mut token) = runtime.next(false, &mut budget).unwrap().unwrap();
        runtime.register_world(1, 1, &mut budget).unwrap();
        runtime
            .register_relative(TaskKind::Body(7), &mut budget)
            .unwrap();
        runtime
            .register_relative(TaskKind::Resume(8), &mut budget)
            .unwrap();
        let mut meter = MachineMeter(&mut budget);
        let scope = runtime
            .kernel()
            .scope(
                ScopeKey {
                    context: 2,
                    generation: 0,
                    policy: 0,
                },
                &mut meter,
            )
            .unwrap();
        let extension = runtime
            .kernel()
            .register(scope, Task(TaskKind::Extension(9)), &mut meter)
            .unwrap();
        runtime.kernel().seal_scope(scope, &mut meter).unwrap();
        let world = runtime.world;
        let slot = runtime
            .kernel()
            .slot(world, &mut MachineMeter(&mut budget))
            .unwrap();
        let mut value = 0usize;
        if dirty {
            runtime
                .with_run(&mut token, &mut budget, |run| {
                    run.read_slot(&slot, &value, 1).unwrap();
                    run.publish_prepared(&slot, &mut value, 1, Change::Changed, 1)
                        .unwrap();
                    Ok(())
                })
                .unwrap();
        }
        let worlds: Vec<_> = runtime.pending_world().collect();
        let relative: Vec<_> = runtime.pending_relative().collect();
        assert!(worlds.contains(&1));
        assert_eq!(worlds.contains(&0), dirty);
        assert_eq!(relative, [TaskKind::Body(7), TaskKind::Resume(8)]);
        assert!(budget.charge(WORK - budget.used()));
        assert!(runtime.finish(token, &mut budget).is_err());
        assert!(budget.exhausted());
        assert_eq!(runtime.pending_world().collect::<Vec<_>>(), worlds);
        assert_eq!(runtime.pending_relative().collect::<Vec<_>>(), relative);
        assert!(runtime.view().worklist().contains(&extension));
        let Closure::Open(open) = runtime.view().closure() else {
            panic!("unpaid finish cannot close the shared kernel")
        };
        assert_eq!(open.pending, 4 + usize::from(dirty));
        assert_eq!(open.in_flight + open.interrupted, 1);
        assert!(runtime.relative_closed(&mut budget).is_err());
    }
}
