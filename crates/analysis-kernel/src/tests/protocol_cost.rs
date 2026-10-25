//! Cross-boundary controls: pure adapter quotes must match real reservations.

use super::{Budget, Stop, scope};
use crate::{
    Capability, Change, Descending, Kernel, KernelError, LocalClosure, RefinePolicy, Scoped,
    cost::{self, ReportCounts},
};
use std::convert::Infallible;

struct Values;
impl Capability for Values {
    type Key = usize;
    type Value = u32;
    fn key_work(key: &usize) -> usize {
        *key
    }
    fn value_work(_: &u32) -> usize {
        8
    }
    // Deliberately different from value_work: retained destruction is opaque.
    fn drop_work(_: &u32) -> usize {
        37
    }
}
struct Minimum;
impl RefinePolicy for Minimum {
    type Value = u32;
    type Error = Infallible;
    fn work(&self, _: &u32, _: &u32) -> usize {
        13
    }
    fn refine(&self, a: &u32, b: &u32) -> Result<u32, Infallible> {
        Ok((*a).min(*b))
    }
    fn equivalent(&self, a: &u32, b: &u32) -> bool {
        a == b
    }
}

#[test]
fn pure_quotes_match_real_interning_dependency_modes_and_replacement_drop() {
    // A zero-work key still needs indexed comparisons; an expensive opaque key
    // must keep its own tariff. Neither changes dependency or publication work.
    for key in [0, 17] {
        let mut meter = Budget::default();
        let mut kernel = Kernel::new().unwrap();
        let scope = scope(&mut kernel, &mut meter, 0, 0);
        assert_eq!(meter.used, cost::tree_work(0) + cost::scope_insert_work(0));
        let before = meter.used;
        let mut channel = kernel
            .channel::<Values, _, _>(Descending(Minimum), &mut meter)
            .unwrap();
        let job = kernel.register(scope, (), &mut meter).unwrap();
        kernel.seal_scope(scope, &mut meter).unwrap();
        assert_eq!(
            meter.used - before,
            cost::CHANNEL_WORK
                + cost::registration_work(0, cost::enqueue_work(0, 0, 1))
                + cost::SEAL_SCOPE_WORK
        );

        let before = meter.used;
        assert_eq!(kernel.next(&mut meter).unwrap(), Some(job));
        let _task = kernel.task_handle(job, &mut meter).unwrap();
        let mut run = kernel.begin(job, &mut meter).unwrap();
        assert_eq!(
            run.meter().used - before,
            cost::dispatch_work(1) - cost::FINISH_WORK
        );
        run.set_reduction(true);

        let before = run.meter().used;
        assert_eq!(
            run.publish(&mut channel, Scoped::new(scope, key), 100)
                .unwrap(),
            Change::Changed
        );
        let intern = cost::tree_work(0)
            + cost::channel_scope_insert_work(0, 1)
            + cost::key_lookup_work(0, key)
            + cost::key_insert_work(0, key, 2);
        let publication = cost::semantic_seed_work(0, 1)
            + cost::change_plan_work(2)
            + cost::change_install_work(2, 0, 0, cost::publication_value_work(8, 0));
        assert_eq!(run.meter().used - before, intern + publication);

        let intern = cost::tree_work(1) + cost::key_lookup_work(1, key);
        for (required, reads, readers, required_edges, demands) in [
            (false, 0, 0, 0, 0),
            (false, 2, 1, 0, 0),
            (true, 2, 1, 0, 0),
            (true, 2, 1, 2, 1),
            (false, 2, 1, 2, 1),
        ] {
            let before = run.meter().used;
            let answer = if required {
                run.query(&mut channel, Scoped::new(scope, key))
            } else {
                run.query_discovery(&mut channel, Scoped::new(scope, key))
            }
            .unwrap();
            assert_eq!(answer.value(), Some(&100));
            let mut quote = intern
                + 8
                + cost::observation_work(reads, readers)
                + cost::observation_work(reads.max(1), readers);
            if required {
                quote += cost::requirement_work(required_edges, demands)
                    + cost::requirement_work(required_edges.max(1), demands);
            }
            assert_eq!(run.meter().used - before, quote);
        }

        let before = run.meter().used;
        assert_eq!(
            run.publish(&mut channel, Scoped::new(scope, key), 50)
                .unwrap(),
            Change::Changed
        );
        let quote = intern
            + 13
            + cost::change_plan_work(1)
            + cost::change_install_work(
                1,
                1,
                cost::enqueue_work(0, 1, 1),
                cost::publication_value_work(8, 37),
            );
        assert_eq!(
            run.meter().used - before,
            quote,
            "the old carrier's drop is separately funded"
        );
        let before = run.meter().used;
        run.finish(LocalClosure::Stable).unwrap();
        assert_eq!(meter.used - before, cost::FINISH_WORK);
        assert_eq!(
            kernel.dependency_count(),
            2,
            "repeat/discovery calls do not duplicate edges"
        );
    }
}

#[test]
fn overflowing_public_bounds_are_refused_before_the_root_meter_or_checkpoint() {
    let overflowed = [
        cost::enqueue_work(0, 0, usize::MAX),
        cost::enqueue_work(2, 1, 0),
        cost::selection_work(usize::MAX),
        cost::priority_selection_work(usize::MAX / 2),
        cost::registration_work(0, usize::MAX),
        cost::key_lookup_work(1, usize::MAX),
        cost::key_insert_work(0, usize::MAX, 0),
        cost::semantic_seed_work(usize::MAX, 0),
        cost::change_plan_work(usize::MAX),
        cost::publication_value_work(1, usize::MAX),
        cost::change_install_work(0, usize::MAX, 0, 0),
        cost::report_metadata_work(usize::MAX),
        cost::report_work(ReportCounts {
            facts: usize::MAX,
            ..ReportCounts::default()
        }),
        cost::report_work(ReportCounts {
            pending: usize::MAX / 4,
            jobs: 1,
            ..ReportCounts::default()
        }),
    ];
    for work in overflowed {
        assert_eq!(work, usize::MAX);
        let mut meter = Budget::default();
        // Cancellation must not turn an overflow into an ordinary meter stop.
        meter.cancelled.set(true);
        assert_eq!(
            crate::kernel::reserve(&mut meter, work),
            Err(KernelError::<Stop>::CostOverflow)
        );
        assert_eq!((meter.used, meter.calls), (0, 0));
    }
}
