use std::rc::Rc;

use analysis_control::{Cancellation, with_cancellation};

use super::super::{Arrival, classify, clone_key};
use super::{
    HeavyKey, PolicyKey, Probe, caches,
    carrier::{Classifier, Hook},
    seeded, snapshot,
};
use crate::{
    analysis::machine::MachineError, domain::relational::QueryReason, resource::WorkBudget,
};

fn cancelled() -> MachineError {
    MachineError::Relations(QueryReason::Cancelled)
}

#[test]
fn either_cancelled_owner_stops_before_quoting_or_running_any_operation() {
    for cancel_captured in [false, true] {
        let probe = Rc::new(Probe::default());
        let mut index = seeded(&probe);
        let before = snapshot(&index);
        let cache = caches(&index);
        let captured = Cancellation::new();
        let current = Cancellation::new();
        let mut budget = with_cancellation(&captured, || WorkBudget::new(usize::MAX));
        if cancel_captured {
            captured.cancel();
        } else {
            current.cancel();
        }
        with_cancellation(&current, || {
            let key = HeavyKey::new(vec![1; 4096], &probe);
            assert_eq!(clone_key(&key, &mut budget), Err(cancelled()));
            assert_eq!(index.get(&key, &mut budget), Err(cancelled()));
            assert_eq!(
                index.insert_with(key, &mut budget, |_| panic!("cancelled builder")),
                Err(cancelled())
            );
            let policy = Classifier {
                work: 61,
                probe: Rc::clone(&probe),
            };
            assert_eq!(
                classify(&policy, Arrival::default(), &mut budget),
                Err(cancelled())
            );
        });
        assert_eq!(probe.quotes.get(), 0);
        assert_eq!(probe.clones.get(), 0);
        assert_eq!(probe.comparisons.get(), 0);
        assert_eq!(probe.classifications.get(), 0);
        assert_eq!(budget.used(), 0);
        assert_eq!(snapshot(&index), before);
        assert_eq!(caches(&index), cache);
    }
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    Clone,
    Get,
    Insert,
    Classify,
}

#[test]
fn cancellation_in_any_quote_never_pays_or_admits_a_result() {
    for cancel_captured in [false, true] {
        for operation in [
            Operation::Clone,
            Operation::Get,
            Operation::Insert,
            Operation::Classify,
        ] {
            let probe = Rc::new(Probe::default());
            let mut index = seeded(&probe);
            let before = snapshot(&index);
            let cache = caches(&index);
            let captured = Cancellation::new();
            let current = Cancellation::new();
            let mut budget = with_cancellation(&captured, || WorkBudget::new(usize::MAX));
            probe.arm(
                Hook::Quote,
                if cancel_captured {
                    captured
                } else {
                    current.clone()
                },
            );
            let result = with_cancellation(&current, || {
                let key = HeavyKey::new(vec![1; 4096], &probe);
                match operation {
                    Operation::Clone => clone_key(&key, &mut budget).map(|_| ()),
                    Operation::Get => index.get(&key, &mut budget).map(|_| ()),
                    Operation::Insert => index
                        .insert_with(key, &mut budget, |_| panic!("quote-cancelled builder"))
                        .map(|_| ()),
                    Operation::Classify => classify(
                        &Classifier {
                            work: 61,
                            probe: Rc::clone(&probe),
                        },
                        Arrival::default(),
                        &mut budget,
                    )
                    .map(|_| ()),
                }
            });
            assert_eq!(
                result,
                Err(cancelled()),
                "{operation:?}, captured={cancel_captured}"
            );
            assert_eq!(probe.quotes.get(), 1);
            assert_eq!(probe.clones.get(), 0);
            assert_eq!(probe.comparisons.get(), 0);
            assert_eq!(probe.classifications.get(), 0);
            assert_eq!(budget.used(), 0);
            assert_eq!(snapshot(&index), before);
            assert_eq!(caches(&index), cache);
        }
    }
}

#[test]
fn cancellation_in_ord_keeps_payment_and_blocks_hit_or_native_builder() {
    for cancel_captured in [false, true] {
        for insert in [false, true] {
            let probe = Rc::new(Probe::default());
            let mut index = seeded(&probe);
            let before = snapshot(&index);
            let cache = caches(&index);
            let captured = Cancellation::new();
            let current = Cancellation::new();
            let mut budget = with_cancellation(&captured, || WorkBudget::new(usize::MAX));
            probe.arm(
                Hook::Compare,
                if cancel_captured {
                    captured
                } else {
                    current.clone()
                },
            );
            let mut native = vec![17];
            let result = with_cancellation(&current, || {
                if insert {
                    index.insert_with(HeavyKey::new(vec![2; 4096], &probe), &mut budget, |_| {
                        native.push(23);
                        Ok(23)
                    })
                } else {
                    index
                        .get(&HeavyKey::new(vec![1; 4096], &probe), &mut budget)
                        .map(|value| value.unwrap())
                }
            });
            assert_eq!(result, Err(cancelled()));
            assert_eq!(native, vec![17]);
            assert_eq!(probe.quotes.get(), 1);
            assert!(probe.comparisons.get() > 0);
            assert!(probe.compared_bytes.get() > 0);
            assert_eq!(probe.clones.get(), 0);
            assert!(budget.used() > 0);
            assert_eq!(snapshot(&index), before);
            assert_eq!(caches(&index), cache);
        }
    }
}

#[test]
fn cancellation_in_real_clone_or_classification_does_not_publish_the_result() {
    for cancel_captured in [false, true] {
        for hook in [Hook::Clone, Hook::Classify] {
            let probe = Rc::new(Probe::default());
            let key = HeavyKey::new(vec![7; 4096], &probe);
            let work = if hook == Hook::Clone {
                key.key_work()
            } else {
                61
            };
            probe.reset();
            let captured = Cancellation::new();
            let current = Cancellation::new();
            let mut budget = with_cancellation(&captured, || WorkBudget::new(work));
            probe.arm(
                hook,
                if cancel_captured {
                    captured
                } else {
                    current.clone()
                },
            );
            let result = with_cancellation(&current, || {
                if hook == Hook::Clone {
                    clone_key(&key, &mut budget).map(|_| ())
                } else {
                    classify(
                        &Classifier {
                            work,
                            probe: Rc::clone(&probe),
                        },
                        Arrival {
                            resume_status: Some(true),
                        },
                        &mut budget,
                    )
                    .map(|_| ())
                }
            });
            assert_eq!(result, Err(cancelled()));
            assert_eq!(probe.quotes.get(), 1);
            if hook == Hook::Clone {
                assert_eq!(probe.clones.get(), 1);
                assert_eq!(probe.cloned_bytes.get(), key.bytes().len());
                assert_eq!(probe.classifications.get(), 0);
            } else {
                assert_eq!(probe.clones.get(), 0);
                assert_eq!(probe.classifications.get(), 1);
            }
            assert_eq!(budget.used(), work);
        }
    }
}
