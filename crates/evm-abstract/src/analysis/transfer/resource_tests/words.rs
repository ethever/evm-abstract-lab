//! Owned representation data for resource tests; this is not an alternate
//! numerical semantics or an independent-analyzer cooperation witness.

#[cfg(test)]
use crate::domain::NumericValue;

use crate::{
    U256,
    domain::{
        DomainSpec, EvmNumericAlgebra, NumericReduction, ProductAlgebra, WordElement,
        WordObservation,
    },
};
use serde::Serialize;
use std::{collections::BTreeSet, fmt};

const COSTLY_BYTE_BYTES: usize = 16_384;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analysis::transfer) struct Copies {
    pub bytes: usize,
    pub quotes: usize,
    pub bytes_at_last_quote: usize,
    pub singleton_queries: usize,
}

pub(in crate::analysis::transfer) static COPIES: crate::test_instrumentation::Counter<Copies> =
    crate::test_instrumentation::Counter::new(Copies {
        bytes: 0,
        quotes: 0,
        bytes_at_last_quote: 0,
        singleton_queries: 0,
    });

pub(in crate::analysis::transfer) fn reset() {
    COPIES.set(Copies::default());
}
pub(in crate::analysis::transfer) fn copies() -> Copies {
    COPIES.get()
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(in crate::analysis::transfer) struct Heavy {
    word: NumericValue,
    pub owned: Vec<u8>,
    clone_multiplier: usize,
}

impl Heavy {
    pub fn payload(word: NumericValue, bytes: usize) -> Self {
        Self {
            word,
            owned: vec![7; bytes],
            clone_multiplier: 1,
        }
    }

    fn costly_byte_bound() -> Self {
        Self {
            word: NumericValue::unknown_byte(),
            owned: vec![7; COSTLY_BYTE_BYTES],
            clone_multiplier: 8,
        }
    }
}

impl Clone for Heavy {
    fn clone(&self) -> Self {
        COPIES.update(|copies| {
            copies.bytes += self.owned.len();
        });
        Self {
            word: self.word.clone(),
            owned: self.owned.clone(),
            clone_multiplier: self.clone_multiplier,
        }
    }
}

impl fmt::Display for Heavy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.word, f)
    }
}

impl WordElement for Heavy {
    fn top() -> Self {
        Self::payload(NumericValue::top(), 0)
    }
    fn constant(value: U256) -> Self {
        Self::payload(NumericValue::constant(value), 0)
    }
    fn from_observation(observation: &WordObservation) -> Self {
        Self::payload(NumericValue::from_observation(observation), 0)
    }
    fn observation(&self) -> WordObservation {
        self.word.observation()
    }
    fn singleton(&self) -> Option<U256> {
        COPIES.update(|copies| copies.singleton_queries += 1);
        self.word.singleton()
    }
    fn contains(&self, word: U256) -> bool {
        self.word.contains(word)
    }
    fn is_top(&self) -> bool {
        self.word.is_top()
    }
    fn work_size(&self) -> usize {
        self.owned
            .len()
            .saturating_add(self.word.work_size())
            .saturating_add(16)
    }
    fn clone_work(&self) -> usize {
        if !self.owned.is_empty() {
            COPIES.update(|copies| {
                copies.quotes += 1;
                copies.bytes_at_last_quote = copies.bytes;
            });
        }
        self.work_size().saturating_mul(self.clone_multiplier)
    }
    fn singleton_work(&self) -> usize {
        self.word.singleton_work()
    }
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        self.word.constants()
    }
    fn finite_constants(&self) -> &crate::domain::FiniteConstantSet {
        self.word.finite_constants()
    }
    fn known_bits(&self) -> &crate::domain::known_bits::KnownBits {
        self.word.known_bits()
    }
    fn interval(&self) -> &crate::domain::interval::Interval {
        self.word.interval()
    }
    fn congruence(&self) -> &crate::domain::congruence::Congruence {
        self.word.congruence()
    }
}

#[derive(Debug)]
pub(in crate::analysis::transfer) struct Policy(pub ProductAlgebra);

impl EvmNumericAlgebra for Policy {
    type Element = Heavy;
    fn spec(&self) -> DomainSpec {
        self.0.spec()
    }
    fn project_numeric(&self, value: &Heavy) -> Heavy {
        value.clone()
    }
    fn join_numeric(&self, left: &Heavy, right: &Heavy) -> Heavy {
        Heavy::payload(self.0.join_numeric(&left.word, &right.word), 0)
    }
    fn widen_numeric(&self, old: &Heavy, next: &Heavy) -> Heavy {
        Heavy::payload(self.0.widen_numeric(&old.word, &next.word), 0)
    }
    fn widen_procedure_numeric(&self, old: &Heavy, next: &Heavy) -> Heavy {
        Heavy::payload(self.0.widen_procedure_numeric(&old.word, &next.word), 0)
    }
    fn reduce_numeric(&self, value: &Heavy) -> NumericReduction<Heavy> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Heavy>,
    ) -> NumericReduction<Heavy> {
        let args: Vec<&Heavy> = args.collect();
        if op == revm_bytecode::opcode::AND
            && args.len() == 2
            && args
                .iter()
                .any(|value| value.singleton() == Some(U256::from(255)))
        {
            return NumericReduction::stable(Heavy::costly_byte_bound());
        }
        let result = self
            .0
            .apply_numeric(op, args.iter().map(|value| &value.word));
        NumericReduction {
            value: Heavy::payload(result.value, 0),
            status: result.status,
            rounds: result.rounds,
            strengthened: result.strengthened,
        }
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Heavy>) -> usize {
        let mut count = 0usize;
        let mut dispatch = 0usize;
        let numeric = self.0.operation_work(
            op,
            args.map(|value| {
                count = count.saturating_add(1);
                if op == revm_bytecode::opcode::AND {
                    dispatch = dispatch.saturating_add(value.singleton_work());
                }
                &value.word
            }),
        );
        // Product consumes the iterator; retain apply_numeric's actual staging
        // fee without allocating another reference vector merely to quote it.
        // Stage/release borrowed argument slots and construct the wrapper's
        // fixed fields. Native work also bounds facade-installed literals.
        let work = numeric
            .saturating_add(count.saturating_mul(2))
            .saturating_add(16);
        if op == revm_bytecode::opcode::AND && count == 2 {
            // Dispatch may query both operands. Reserve the special result's
            // heap construction and release if facade folding replaces it,
            // without running dispatch or quoting a nonexistent operand copy.
            work.saturating_add(dispatch)
                .saturating_add(COSTLY_BYTE_BYTES.saturating_mul(2))
        } else {
            work
        }
    }
    fn projection_work(&self, value: &Heavy) -> usize {
        value.work_size().saturating_mul(4)
    }
    fn refinement_work(&self, value: &Heavy, facts: usize) -> usize {
        value.work_size().saturating_add(facts).saturating_mul(4096)
    }
}

#[test]
fn operation_quote_uses_the_selected_product_capacity() {
    use std::num::NonZeroUsize;

    let _copies = COPIES.scope();
    let capacity = 131_072;
    let policy = Policy(ProductAlgebra::from_spec(DomainSpec::new(
        NonZeroUsize::new(capacity).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        // Leave room for the cover's other retained semantic atoms.
        NonZeroUsize::new(capacity * 2).unwrap(),
    )));
    let args = [Heavy::top(), Heavy::constant(U256::from(capacity - 1))];
    let quote = policy.operation_work(revm_bytecode::opcode::AND, args.iter());
    // The low-17-bit cube can retain all capacity members. NumericValue adds
    // twelve inline units and Heavy adds sixteen; do not enumerate that cube.
    let numeric_size = capacity + 12 + 16;
    assert!(quote < usize::MAX && quote >= numeric_size);
    assert!(
        quote
            > Policy(ProductAlgebra::default())
                .operation_work(revm_bytecode::opcode::AND, args.iter())
    );
    assert!(
        quote
            >= policy
                .0
                .operation_work(revm_bytecode::opcode::AND, args.iter().map(|arg| &arg.word))
    );
    assert_eq!(copies(), Copies::default());
}

#[test]
fn byte_bound_quote_keeps_queries_copies_and_tariffs_independent() {
    use crate::domain::{EvmValue, ValueAlgebra};
    use revm_bytecode::opcode;

    let _copies = COPIES.scope();
    let policy = Policy(ProductAlgebra::default());
    let args = [
        Heavy::payload(NumericValue::top(), 64),
        Heavy::payload(NumericValue::constant(U256::from(255)), 64),
    ];
    let quote = policy.operation_work(opcode::AND, args.iter());
    let add_quote = policy.operation_work(opcode::ADD, args.iter());
    assert!(add_quote < usize::MAX);
    assert_eq!(copies(), Copies::default());

    let result = policy.apply_numeric(opcode::AND, args.iter()).value;
    assert_eq!(copies().singleton_queries, 2);
    assert_eq!(result.owned.len(), COSTLY_BYTE_BYTES);
    assert_eq!(result.work_size(), COSTLY_BYTE_BYTES + 13 + 16);
    assert!(quote >= result.work_size());
    assert_eq!(result.clone_work(), result.work_size() * 8);
    assert_eq!(policy.projection_work(&result), result.work_size() * 4);
    assert_eq!(
        policy.refinement_work(&result, 1),
        (result.work_size() + 1) * 4096
    );
    assert_eq!(result.clone(), result);
    assert_eq!(copies().bytes, COSTLY_BYTE_BYTES);
    assert_eq!(copies().quotes, 1);
    // Operand construction refusal is independent of a complete operation fee.
    assert_eq!(policy.literal_quote_work(U256::from(255)), usize::MAX);

    let facade = ValueAlgebra::new(policy);
    let literals = [
        EvmValue::<Heavy>::constant(U256::from(255)),
        EvmValue::<Heavy>::constant(U256::ONE),
    ];
    reset();
    let quote = facade
        .numeric_algebra()
        .operation_work(opcode::AND, literals.iter().map(EvmValue::numeric));
    assert_eq!(copies(), Copies::default());
    let folded = facade.apply(opcode::AND, &literals);
    assert_eq!(folded.singleton(), Some(U256::ONE));
    assert!(folded.numeric().owned.is_empty());
    assert!(quote >= folded.numeric().work_size());
}

#[test]
fn operation_quote_preserves_product_refusal() {
    use std::num::NonZeroUsize;

    let _copies = COPIES.scope();
    let policy = Policy(ProductAlgebra::from_spec(DomainSpec::new(
        NonZeroUsize::new(usize::MAX).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(usize::MAX).unwrap(),
    )));
    let args = [
        Heavy::payload(NumericValue::top(), 64),
        Heavy::payload(NumericValue::constant(U256::from(255)), 64),
    ];
    assert_eq!(
        policy
            .0
            .operation_work(revm_bytecode::opcode::AND, args.iter().map(|arg| &arg.word)),
        usize::MAX
    );
    assert_eq!(
        policy.operation_work(revm_bytecode::opcode::AND, args.iter()),
        usize::MAX
    );
    assert_eq!(copies(), Copies::default());
}
