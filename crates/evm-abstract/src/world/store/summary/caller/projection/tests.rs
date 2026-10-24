//! Native discovery and key work remain separately prepaid.
use super::*;
use crate::domain::{NumericValue, WordObservation};
use crate::resource::CloneError;
use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Receipt {
    finite: usize,
    singleton: usize,
    clones: usize,
}
static RECEIPT: crate::test_instrumentation::Counter<Receipt> =
    crate::test_instrumentation::Counter::new(Receipt {
        finite: 0,
        singleton: 0,
        clones: 0,
    });

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Word {
    finite: Option<BTreeSet<U256>>,
    exact: Option<U256>,
    work: usize,
    query: usize,
    private: bool,
}
impl Clone for Word {
    fn clone(&self) -> Self {
        RECEIPT.update(|r| r.clones += 1);
        Self {
            finite: self.finite.clone(),
            exact: self.exact,
            work: self.work,
            query: self.query,
            private: self.private,
        }
    }
}
impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "private candidate view")
    }
}
impl WordElement for Word {
    fn top() -> Self {
        Self {
            finite: None,
            exact: None,
            work: 128,
            query: 64,
            private: false,
        }
    }
    fn constant(value: U256) -> Self {
        Self {
            exact: Some(value),
            ..Self::top()
        }
    }
    fn from_observation(_: &WordObservation) -> Self {
        panic!("candidate work never imports an observation")
    }
    fn observation(&self) -> WordObservation {
        panic!("candidate work never exports private N")
    }
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        RECEIPT.update(|r| r.finite += 1);
        self.finite.as_ref()
    }
    fn singleton(&self) -> Option<U256> {
        RECEIPT.update(|r| r.singleton += 1);
        self.exact
    }
    fn singleton_work(&self) -> usize {
        self.query
    }
    fn is_top(&self) -> bool {
        self.finite.is_none() && self.exact.is_none() && !self.private
    }
    fn work_size(&self) -> usize {
        self.work
    }
}
fn input(finite: Option<BTreeSet<U256>>, exact: Option<U256>) -> EvmValue<Word> {
    EvmValue::from_numeric(Word {
        finite,
        exact,
        private: true,
        ..Word::top()
    })
}

#[test]
fn borrowed_subset_and_staged_copy_match_complete_candidate_views() {
    let _scope = RECEIPT.scope();
    let values = [
        input(None, None),
        input(None, Some(U256::from(7))),
        input(Some(BTreeSet::new()), None),
        input(Some([U256::ZERO, U256::ONE, U256::from(7)].into()), None),
        input(Some((0..257).map(U256::from).collect()), None),
    ];
    for value in &values {
        for keys in [
            BTreeSet::new(),
            [U256::from(7)].into(),
            [U256::ZERO, U256::ONE, U256::from(7)].into(),
            (0..257).map(U256::from).collect(),
        ] {
            let expected = crate::world::candidates(value)
                .map(|view| view.iter().copied().collect::<BTreeSet<_>>());
            let subset = expected.as_ref().is_some_and(|view| view.is_subset(&keys));
            RECEIPT.set(Receipt::default());
            assert_eq!(
                candidates(value, &mut WorkBudget::new(1_000_000_000)).unwrap(),
                expected
            );
            let copied = RECEIPT.get();
            RECEIPT.set(Receipt::default());
            assert_eq!(
                candidate_subset(value, &keys, &mut WorkBudget::new(1_000_000_000)).unwrap(),
                subset
            );
            assert_eq!(
                RECEIPT.get(),
                copied,
                "same native view, no additional native clone/query"
            );
            assert_eq!(copied.clones, 0);
        }
    }
    // The complete real product has the same view as this private test carrier.
    let value = EvmValue::from_numeric(NumericValue::unknown_address());
    assert!(
        candidates(&value, &mut WorkBudget::new(100_000))
            .unwrap()
            .is_none()
    );
}

#[test]
fn no_candidate_pays_native_discovery_without_unperformed_key_copy() {
    let _scope = RECEIPT.scope();
    let value = input(None, None);
    let mut budget = WorkBudget::new(1_000_000);
    assert!(candidates(&value, &mut budget).unwrap().is_none());
    assert_eq!(budget.used(), 16 + 128 + 64);
    assert_eq!(
        RECEIPT.get(),
        Receipt {
            finite: 1,
            singleton: 1,
            clones: 0
        }
    );
    let mut old = WorkBudget::new(1_000_000);
    pay_structure(128, &mut old).unwrap();
    assert!(old.used() > budget.used());
}

#[test]
fn coordinate_copy_and_subset_refuse_at_their_exact_boundaries() {
    let _scope = RECEIPT.scope();
    let value = input(Some([U256::ONE, U256::from(7)].into()), None);
    for subset in [false, true] {
        let keys = [U256::ONE, U256::from(7)].into();
        let run = |budget: &mut WorkBudget| {
            if subset {
                candidate_subset(&value, &keys, budget).map(|_| ())
            } else {
                candidates(&value, budget).map(|_| ())
            }
        };
        let mut full = WorkBudget::new(1_000_000);
        run(&mut full).unwrap();
        let exact = full.used();
        for maximum in [0, exact - 1] {
            RECEIPT.set(Receipt::default());
            let mut denied = WorkBudget::new(maximum);
            assert_eq!(
                run(&mut denied),
                Err(SummaryError::Budget(CloneError::Work))
            );
            let prior = RECEIPT.get();
            assert_eq!(prior.clones, 0);
            assert!(run(&mut denied).is_err());
            assert_eq!(RECEIPT.get(), prior);
        }
        run(&mut WorkBudget::new(exact)).unwrap();
    }
    let value = EvmValue::from_numeric(Word {
        work: usize::MAX,
        ..Word::top()
    });
    RECEIPT.set(Receipt::default());
    assert!(candidates(&value, &mut WorkBudget::new(usize::MAX)).is_err());
    assert_eq!(RECEIPT.get(), Receipt::default());
    let value = EvmValue::from_numeric(Word {
        query: usize::MAX,
        ..Word::top()
    });
    RECEIPT.set(Receipt::default());
    assert!(candidates(&value, &mut WorkBudget::new(usize::MAX)).is_err());
    assert_eq!(
        RECEIPT.get(),
        Receipt {
            finite: 1,
            singleton: 0,
            clones: 0
        }
    );
}

#[test]
fn both_cancellation_owners_precede_native_candidate_discovery() {
    let _scope = RECEIPT.scope();
    let value = input(None, None);
    for captured in [false, true] {
        let token = analysis_control::Cancellation::new();
        let mut budget = if captured {
            analysis_control::with_cancellation(&token, || WorkBudget::new(1_000_000))
        } else {
            WorkBudget::new(1_000_000)
        };
        token.cancel();
        RECEIPT.set(Receipt::default());
        let result = if captured {
            candidates(&value, &mut budget)
        } else {
            analysis_control::with_cancellation(&token, || candidates(&value, &mut budget))
        };
        assert_eq!(result, Err(SummaryError::Budget(CloneError::Cancelled)));
        assert_eq!(budget.used(), 0);
        assert_eq!(RECEIPT.get(), Receipt::default());
    }
}
