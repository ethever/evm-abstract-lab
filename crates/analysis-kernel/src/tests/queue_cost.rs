//! Check allocation bounds against real queue growth, including several doublings.

use super::Worklist;
use crate::{
    KernelError, cost,
    tests::{Budget, Stop},
};

#[test]
fn batch_quote_funds_every_reallocation_and_duplicate_wake() {
    for pending in [0, 1, 2, 3, 4, 7, 8, 15, 16, 31, 32, 63, 64, 65] {
        for additional in [0, 1, 2, 3, 16, 129, 257] {
            let mut queue: Worklist<_> = (0..pending).collect();
            let quote = queue.enqueue_work_for(additional);
            assert_eq!(
                quote,
                cost::enqueue_work(pending, queue.nodes.capacity(), additional)
            );
            let upper = cost::enqueue_work(pending, pending, additional);
            assert!(quote <= upper, "a full queue bounds any spare capacity");

            let mut copied_fields = 0;
            for id in pending..pending + additional {
                if queue.nodes.len() == queue.nodes.capacity() {
                    copied_fields += queue.nodes.len() * 3;
                }
                queue.enqueue(id);
            }
            let index_and_links = additional * (cost::tree_work(pending + additional) + 6);
            assert!(
                quote >= index_and_links + copied_fields,
                "batch quote must prepay every copied three-field node: pending={pending} additional={additional}"
            );
            assert_eq!(
                queue.in_order().collect::<Vec<_>>(),
                (0..pending + additional).collect::<Vec<_>>()
            );

            let duplicate_quote = queue.enqueue_work_for(additional);
            for id in pending..pending + additional {
                queue.enqueue(id);
            }
            assert!(duplicate_quote <= cost::enqueue_work(queue.len(), queue.len(), additional));
            assert_eq!(queue.len(), pending + additional);
        }
    }
}

#[test]
fn pure_selection_prices_match_paid_operations_and_refusal_keeps_the_queue() {
    for pending in [1, 2, 3, 4, 8, 65] {
        for priority in [false, true] {
            let mut queue: Worklist<_> = (0..pending).collect();
            let before = queue.clone();
            let quote = if priority {
                cost::priority_selection_work(pending)
            } else {
                cost::selection_work(pending)
            };
            let mut meter = Budget::default();
            for stop in [Stop::Work, Stop::Cancelled] {
                meter.stop_after(1, stop);
                let result = if priority {
                    queue.pop_deepest(&mut meter, |_| panic!("refused priority scan"))
                } else {
                    queue.pop_eligible(&mut meter, |_| panic!("refused FIFO scan"))
                };
                assert_eq!(result, Err(KernelError::Stopped(stop)));
                assert_eq!(queue, before);
            }
            assert_eq!(meter.used, quote, "only the cancelled reservation was paid");
            meter.clear_stop();
            let paid_before = meter.used;
            let result = if priority {
                queue.pop_deepest(&mut meter, |id| id + 1)
            } else {
                queue.pop_eligible(&mut meter, |id| id + 1 == pending)
            };
            assert_eq!(result.unwrap(), Some(pending - 1));
            assert_eq!(meter.used - paid_before, quote);
            assert_eq!(queue.len(), pending - 1);
        }
    }
}
