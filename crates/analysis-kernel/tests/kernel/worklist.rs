use super::support::{Budget, Stop, scope};
use analysis_kernel::{Kernel, KernelError, LocalClosure, Worklist};
use proptest::test_runner::{Config, FileFailurePersistence};
use std::{cell::Cell, collections::VecDeque};

#[test]
fn direct_seed_generations_retain_only_live_queue_records() {
    let mut meter = Budget::default();
    let mut kernel = Kernel::new().unwrap();
    let owner = scope(&mut kernel, &mut meter, 0);
    let pending = kernel.register(owner, (), &mut meter).unwrap();
    let mut first_selection_work = None;
    for generation in 1..=10_000 {
        let current = scope(&mut kernel, &mut meter, generation);
        let seed = kernel.register(current, (), &mut meter).unwrap();
        assert_eq!(kernel.worklist().records(), 2);
        kernel
            .begin(seed, &mut meter)
            .unwrap()
            .finish(LocalClosure::Stable)
            .unwrap();
        assert_eq!(kernel.worklist().records(), 1);
        assert_eq!(kernel.worklist().in_order().collect::<Vec<_>>(), [pending]);
        let before = meter.used;
        assert_eq!(kernel.next(&mut meter).unwrap(), Some(pending));
        let work = meter.used - before;
        assert_eq!(*first_selection_work.get_or_insert(work), work);
        kernel
            .begin(pending, &mut meter)
            .unwrap()
            .finish(LocalClosure::Stable)
            .unwrap();
        assert_eq!(kernel.worklist().records(), 0);
        kernel.retry(pending, &mut meter).unwrap();
    }
}

#[test]
fn satisfied_history_never_affects_priority_visits_or_selection_work() {
    let mut historical = Worklist::default();
    historical.enqueue(7);
    for id in 10..10_010 {
        historical.enqueue(id);
        historical.satisfy(id);
        assert_eq!(historical.records(), 1);
    }
    historical.enqueue(8);
    let fresh: Worklist<_> = [7, 8].into_iter().collect();
    for deepest in [false, true] {
        let mut costs = Vec::new();
        for mut queue in [fresh.clone(), historical.clone()] {
            let mut meter = Budget::default();
            let visited = Cell::new(0);
            let chosen = if deepest {
                queue.pop_deepest(&mut meter, |id| {
                    assert!(id == 7 || id == 8);
                    visited.set(visited.get() + 1);
                    1
                })
            } else {
                queue.pop_eligible(&mut meter, |id| {
                    assert!(id == 7 || id == 8);
                    visited.set(visited.get() + 1);
                    id == 8
                })
            }
            .unwrap();
            assert_eq!(chosen, Some(if deepest { 7 } else { 8 }));
            assert_eq!(visited.get(), 2);
            costs.push(meter.used);
        }
        assert_eq!(costs[0], costs[1], "only live work is charged");
    }
}

#[test]
fn cancelled_and_denied_selections_leave_fifo_and_priorities_unchanged() {
    for deepest in [false, true] {
        // Cancel from each callback, including the final one. A callback can
        // observe an asynchronous cancellation after its preceding checkpoint.
        for cancel_at in 0..3 {
            let mut queue: Worklist<_> = [0, 1, 2].into_iter().collect();
            queue.satisfy(1);
            queue.enqueue(1);
            let before = queue.clone();
            let mut meter = Budget::default();
            let cancelled = std::rc::Rc::clone(&meter.cancelled);
            let visited = Cell::new(0);
            let visit = |_| {
                if visited.get() == cancel_at {
                    cancelled.set(true);
                }
                visited.set(visited.get() + 1);
            };
            let result = if deepest {
                queue.pop_deepest(&mut meter, |id| {
                    visit(id);
                    1
                })
            } else {
                queue.pop_eligible(&mut meter, |id| {
                    visit(id);
                    visited.get() == 3
                })
            };
            assert_eq!(result, Err(KernelError::Stopped(Stop::Cancelled)));
            assert_eq!(queue, before);
            assert_eq!(queue.records(), queue.len());
            assert!(meter.used > 0);
            cancelled.set(false);
            meter.fail_on = Some(meter.calls + 1);
            let result = if deepest {
                queue.pop_deepest(&mut meter, |_| panic!("denied priority scan"))
            } else {
                queue.pop_eligible(&mut meter, |_| panic!("denied FIFO scan"))
            };
            assert_eq!(result, Err(KernelError::Stopped(Stop::Work)));
            assert_eq!(queue, before);
            meter.fail_on = None;
            assert_eq!(queue.in_order().collect::<Vec<_>>(), [0, 2, 1]);
            assert_eq!(queue.pop_deepest(&mut meter, |_| 0).unwrap(), None);
            assert_eq!(queue, before);
            let mut priorities = queue.clone();
            assert_eq!(
                priorities.pop_deepest(&mut meter, |id| id + 1).unwrap(),
                Some(2)
            );
            assert_eq!(priorities.in_order().collect::<Vec<_>>(), [0, 1]);
            assert_eq!(queue.pop_deepest(&mut meter, |_| 4).unwrap(), Some(0));
            queue.enqueue(0);
            assert_eq!(queue.pop_front(), Some(2));
            assert_eq!(queue.pop_front(), Some(1));
            assert_eq!(
                queue.pop_front(),
                Some(0),
                "requeue receives a fresh FIFO position"
            );
            assert_eq!(queue.pop_front(), None);
        }
    }
}

#[test]
fn dense_removals_match_a_fifo_reference_across_mixed_operations() {
    // Long-history stress stays deterministic. Generated traces explore order
    // and shrink independently: two six-operation / 31-key spaces per case.
    const GENERATED_STEPS: usize = 2 * 6 * 31;
    let boundaries = [(3, 0), (4, 0), (5, 0), (2, 0), (0, 0), (1, 0), (2, 0)];
    let dense = (0..31).map(|id| (0, id));
    let mixed = (0..20_000).map(|step| ((step % 6) as u8, (step * 17 + step / 6) % 31));
    assert_operations_match_reference(boundaries.into_iter().chain(dense).chain(mixed));
    let config = Config::with_failure_persistence(FileFailurePersistence::WithSource(
        "proptest-regressions",
    ));
    proptest::proptest!(config, |(operations in proptest::collection::vec((0_u8..6, 0_usize..31), 0..=GENERATED_STEPS))| {
        assert_operations_match_reference(operations);
    });
}

fn assert_operations_match_reference(operations: impl IntoIterator<Item = (u8, usize)>) {
    let mut queue = Worklist::default();
    let mut reference = VecDeque::new();
    let mut meter = Budget::default();
    for (operation, id) in operations {
        match operation {
            0 | 1 => {
                queue.enqueue(id);
                if !reference.contains(&id) {
                    reference.push_back(id);
                }
            }
            2 => {
                queue.satisfy(id);
                reference.retain(|entry| *entry != id);
            }
            3 => assert_eq!(queue.pop_front(), reference.pop_front()),
            4 => {
                let expected = reference
                    .iter()
                    .position(|entry| entry % 3 == id % 3)
                    .and_then(|position| reference.remove(position));
                assert_eq!(
                    queue
                        .pop_eligible(&mut meter, |entry| entry % 3 == id % 3)
                        .unwrap(),
                    expected
                );
            }
            5 => {
                let maximum = reference.iter().map(|entry| entry % 4).max().unwrap_or(0);
                let expected = if maximum == 0 {
                    None
                } else {
                    reference
                        .iter()
                        .position(|entry| entry % 4 == maximum)
                        .and_then(|position| reference.remove(position))
                };
                assert_eq!(
                    queue.pop_deepest(&mut meter, |entry| entry % 4).unwrap(),
                    expected
                );
            }
            _ => unreachable!("worklist operation outside the generated six-way space"),
        }
        assert_eq!(
            queue.in_order().collect::<Vec<_>>(),
            reference.iter().copied().collect::<Vec<_>>()
        );
        assert_eq!(queue.records(), reference.len());
        assert_eq!(queue.len(), reference.len());
        let mut sorted: Vec<_> = reference.iter().copied().collect();
        sorted.sort();
        assert_eq!(queue.pending().collect::<Vec<_>>(), sorted);
        for candidate in 0..31 {
            assert_eq!(queue.contains(&candidate), reference.contains(&candidate));
        }
    }
}
