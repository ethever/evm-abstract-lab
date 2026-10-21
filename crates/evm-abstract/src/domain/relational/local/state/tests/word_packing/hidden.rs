//! Independent finite carrier with an intentionally hidden interior exclusion.

use super::*;
use crate::domain::{WordObservation, interval::Interval};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Hidden {
    members: Option<BTreeSet<U256>>,
    bounds: Interval,
}
impl std::fmt::Display for Hidden {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.members)
    }
}
impl Hidden {
    fn finite(members: BTreeSet<U256>) -> Self {
        let bounds = Interval::from_values(&members);
        Self {
            members: Some(members),
            bounds,
        }
    }
}
impl WordElement for Hidden {
    fn top() -> Self {
        Self {
            members: None,
            bounds: Interval::top(),
        }
    }
    fn constant(value: U256) -> Self {
        Self::finite(BTreeSet::from([value]))
    }
    fn unknown_byte() -> Self {
        Self::finite((0..=255u64).map(U256::from).collect())
    }
    fn from_observation(o: &WordObservation) -> Self {
        o.singleton().map_or(
            Self {
                members: None,
                bounds: *o.interval(),
            },
            Self::constant,
        )
    }
    fn observation(&self) -> WordObservation {
        // Only the hull crosses this boundary. The interior hole stays in N.
        WordObservation::unsigned_range(
            self.bounds.unsigned_bounds().0,
            self.bounds.unsigned_bounds().1,
        )
        .unwrap()
    }
    fn interval(&self) -> &Interval {
        &self.bounds
    }
    fn contains(&self, value: U256) -> bool {
        self.bounds.contains(value) && self.members.as_ref().is_none_or(|v| v.contains(&value))
    }
    fn singleton(&self) -> Option<U256> {
        self.members
            .as_ref()
            .filter(|v| v.len() == 1)
            .and_then(|v| v.first().copied())
    }
    fn is_top(&self) -> bool {
        self.members.is_none() && self.bounds.is_top()
    }
    fn work_size(&self) -> usize {
        16 + self.members.as_ref().map_or(0, BTreeSet::len)
    }
}

#[derive(Debug, Default)]
struct HiddenPolicy {
    calls: Cell<usize>,
    packs: Cell<usize>,
}
impl EvmNumericAlgebra for HiddenPolicy {
    type Element = Hidden;
    fn spec(&self) -> DomainSpec {
        captured_domain().spec()
    }
    fn project_numeric(&self, v: &Hidden) -> Hidden {
        v.clone()
    }
    fn join_numeric(&self, a: &Hidden, b: &Hidden) -> Hidden {
        match (&a.members, &b.members) {
            (Some(a), Some(b)) if a.len() + b.len() <= 512 => {
                Hidden::finite(a.union(b).copied().collect())
            }
            _ => Hidden::top(),
        }
    }
    fn widen_numeric(&self, a: &Hidden, b: &Hidden) -> Hidden {
        self.join_numeric(a, b)
    }
    fn widen_procedure_numeric(&self, a: &Hidden, b: &Hidden) -> Hidden {
        self.join_numeric(a, b)
    }
    fn reduce_numeric(&self, v: &Hidden) -> NumericReduction<Hidden> {
        NumericReduction::stable(v.clone())
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        mut args: impl ExactSizeIterator<Item = &'a Hidden>,
    ) -> NumericReduction<Hidden> {
        self.calls.set(self.calls.get() + 1);
        if args.len() != 2 {
            return NumericReduction::stable(Hidden::top());
        }
        let a = args.next().unwrap();
        let b = args.next().unwrap();
        let value = match (&a.members, &b.members) {
            (Some(a), Some(b)) if a.len() * b.len() <= 512 => Hidden::finite(
                a.iter()
                    .flat_map(|a| {
                        b.iter().map(move |b| match op {
                            opcode::SHL if *a >= U256::from(256) => U256::ZERO,
                            opcode::SHL => *b << a.to::<usize>(),
                            opcode::OR => *a | *b,
                            _ => panic!("word fold used an unexpected opcode"),
                        })
                    })
                    .collect(),
            ),
            _ => Hidden::top(),
        };
        NumericReduction::stable(value)
    }
    fn operation_work<'a>(&self, _op: u8, args: impl Iterator<Item = &'a Hidden>) -> usize {
        args.fold(1024usize, |work, v| {
            work.saturating_add(v.work_size().saturating_mul(4096))
        })
    }
    fn literal_quote_work(&self, _: U256) -> usize {
        128
    }
    fn projection_work(&self, v: &Hidden) -> usize {
        v.work_size()
    }
    fn refinement_work(&self, v: &Hidden, _: usize) -> usize {
        v.work_size().saturating_mul(4096)
    }
    // Deliberately leave pack_word_work at its conservative default. Even an
    // existing custom hook must not run until that policy provides a paid quote.
    fn pack_word_numeric(&self, _: &[&Hidden; 32]) -> Option<Hidden> {
        self.packs.set(self.packs.get() + 1);
        panic!("unquoted custom hook must never run")
    }
}

#[test]
fn alternate_carrier_hidden_exclusion_survives_the_injected_word_fold() {
    let policy = HiddenPolicy::default();
    let algebra = ValueAlgebra::new(&policy);
    let mut bytes = std::array::from_fn(|_| Hidden::constant(U256::ZERO));
    let members = (0..=255u64).filter(|v| *v != 128).map(U256::from).collect();
    bytes[31] = Hidden::finite(members);
    assert_eq!(
        bytes[31].observation(),
        Hidden::unknown_byte().observation()
    );
    assert!(bytes[31].requires_retained_binding());
    assert!(!bytes[31].contains(U256::from(128)));
    assert_eq!(
        algebra
            .borrowed()
            .pack_word_work(&std::array::from_fn(|i| &bytes[i])),
        None
    );
    let expected = ordinary(&algebra, &bytes);
    policy.calls.set(0);
    let mut budget = WorkBudget::new(20_000_000);
    let result = pack(&algebra, bytes, &mut budget).unwrap();
    assert_eq!(result, expected);
    assert!(!result.contains(U256::from(128)));
    assert!(result.contains(U256::from(127)));
    assert!(result.contains(U256::from(129)));
    assert_eq!(policy.calls.get(), 64);
    assert_eq!(policy.packs.get(), 0);
    assert!(!budget.exhausted());
}
