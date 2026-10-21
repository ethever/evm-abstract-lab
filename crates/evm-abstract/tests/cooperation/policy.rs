//! Borrowed immutable non-Copy, non-Clone policy executing the independent carrier.
use super::carrier::ExcludingWord;
use alloy_primitives::U256;
use evm_abstract::domain::{DomainSpec, EvmNumericAlgebra, NumericReduction, WordElement};
use revm_bytecode::opcode;
use std::{
    num::NonZeroUsize,
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Debug, Default)]
pub struct Calls {
    pub arithmetic: AtomicUsize,
    pub projections: AtomicUsize,
    pub joins: AtomicUsize,
    pub reductions: AtomicUsize,
}
#[derive(Debug)]
pub struct Policy<'a> {
    pub configuration: Vec<u8>,
    pub calls: &'a Calls,
    pub exclusion_queries: bool,
    pub retain_exclusions: bool,
    pub coarsen_one: bool,
    pub relations: evm_abstract::domain::relational::RelationLimits,
}
impl<'a> Policy<'a> {
    pub fn new(calls: &'a Calls) -> Self {
        Self {
            configuration: vec![17; 257],
            calls,
            exclusion_queries: true,
            retain_exclusions: true,
            coarsen_one: false,
            relations: Default::default(),
        }
    }
    fn transfer(&self, op: u8, args: &[&ExcludingWord]) -> ExcludingWord {
        let first = args.first().copied();
        let second = args.get(1).copied();
        if let Some(a) = first
            && op == opcode::ISZERO
            && !a.contains(U256::ZERO)
        {
            return ExcludingWord::constant(U256::ZERO);
        }
        if let (Some(a), Some(b)) = (first, second) {
            if self.exclusion_queries
                && op == opcode::EQ
                && (a.singleton().is_some_and(|word| !b.contains(word))
                    || b.singleton().is_some_and(|word| !a.contains(word)))
            {
                return ExcludingWord::constant(U256::ZERO);
            }
            if matches!(op, opcode::ADD | opcode::OR | opcode::XOR) {
                if a.singleton() == Some(U256::ZERO) {
                    return b.clone();
                }
                if b.singleton() == Some(U256::ZERO) {
                    return a.clone();
                }
            }
            if op == opcode::SUB && b.singleton() == Some(U256::ZERO) {
                return a.clone();
            }
            if op == opcode::MUL {
                if a.singleton() == Some(U256::ONE) {
                    return b.clone();
                }
                if b.singleton() == Some(U256::ONE) {
                    return a.clone();
                }
                if a.singleton() == Some(U256::ZERO) || b.singleton() == Some(U256::ZERO) {
                    return ExcludingWord::constant(U256::ZERO);
                }
            }
            if op == opcode::AND {
                if a.singleton() == Some(U256::MAX) {
                    return b.clone();
                }
                if b.singleton() == Some(U256::MAX) {
                    return a.clone();
                }
            }
        }
        let constants: Option<Vec<_>> = args.iter().copied().map(WordElement::singleton).collect();
        if let Some(words) = constants {
            let a = words.first().copied().unwrap_or_default();
            let b = words.get(1).copied().unwrap_or_default();
            let truth = |condition: bool| if condition { U256::ONE } else { U256::ZERO };
            let value = match op {
                opcode::ADD => Some(a.wrapping_add(b)),
                opcode::SUB => Some(a.wrapping_sub(b)),
                opcode::MUL => Some(a.wrapping_mul(b)),
                opcode::DIV => Some(if b == U256::ZERO { U256::ZERO } else { a / b }),
                opcode::MOD => Some(if b == U256::ZERO { U256::ZERO } else { a % b }),
                opcode::EQ => Some(truth(a == b)),
                opcode::LT => Some(truth(a < b)),
                opcode::GT => Some(truth(a > b)),
                opcode::ISZERO => Some(truth(a == U256::ZERO)),
                opcode::AND => Some(a & b),
                opcode::OR => Some(a | b),
                opcode::XOR => Some(a ^ b),
                opcode::NOT => Some(!a),
                opcode::SHL => Some(if a >= U256::from(256) {
                    U256::ZERO
                } else {
                    b << a.to::<usize>()
                }),
                opcode::SHR => Some(if a >= U256::from(256) {
                    U256::ZERO
                } else {
                    b >> a.to::<usize>()
                }),
                opcode::BYTE => Some(if a >= U256::from(32) {
                    U256::ZERO
                } else {
                    (b >> ((31 - a.to::<usize>()) * 8)) & U256::from(255)
                }),
                _ => None,
            };
            if let Some(word) = value {
                return ExcludingWord::constant(word);
            }
        }
        if op == opcode::AND
            && let Some(mask) = args.iter().copied().find_map(WordElement::singleton)
        {
            return ExcludingWord::range(U256::ZERO, mask);
        }
        ExcludingWord::top()
    }
}
impl EvmNumericAlgebra for Policy<'_> {
    type Element = ExcludingWord;
    fn spec(&self) -> DomainSpec {
        DomainSpec::new(
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        )
        .with_relations(self.relations)
    }
    fn project_numeric(&self, value: &ExcludingWord) -> ExcludingWord {
        self.calls.projections.fetch_add(1, Ordering::Relaxed);
        if self.retain_exclusions {
            value.clone()
        } else {
            // Explicit sound coarsening for the production ablation. Merely
            // disabling EQ's fast consequence is insufficient: generic relation
            // reasoning may still use the native carrier's contains/constrain.
            ExcludingWord::from_observation(&value.observation())
        }
    }
    fn join_numeric(&self, a: &ExcludingWord, b: &ExcludingWord) -> ExcludingWord {
        self.calls.joins.fetch_add(1, Ordering::Relaxed);
        ExcludingWord::covering(a, b)
    }
    fn widen_numeric(&self, old: &ExcludingWord, next: &ExcludingWord) -> ExcludingWord {
        let mut joined = ExcludingWord::covering(old, next);
        if joined.lower < old.lower {
            joined.lower = U256::ZERO;
        }
        if joined.upper > old.upper {
            joined.upper = U256::MAX;
        }
        joined
    }
    fn widen_procedure_numeric(&self, old: &ExcludingWord, next: &ExcludingWord) -> ExcludingWord {
        self.widen_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &ExcludingWord) -> NumericReduction<ExcludingWord> {
        self.calls.reductions.fetch_add(1, Ordering::Relaxed);
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a ExcludingWord>,
    ) -> NumericReduction<ExcludingWord>
    where
        ExcludingWord: 'a,
    {
        let args: Vec<_> = args.collect();
        self.calls.arithmetic.fetch_add(1, Ordering::Relaxed);
        // Sound precision loss for the source-receipt regression: OR([0,1],1)
        // is one, but this configured carrier intentionally retains [1,2].
        if self.coarsen_one
            && op == opcode::OR
            && args.len() == 2
            && (0..2).any(|i| {
                args[i].singleton() == Some(U256::ONE)
                    && args[1 - i].lower == U256::ZERO
                    && args[1 - i].upper == U256::ONE
            })
        {
            return NumericReduction::stable(ExcludingWord::range(U256::ONE, U256::from(2)));
        }
        NumericReduction::stable(self.transfer(op, &args))
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a ExcludingWord>) -> usize
    where
        ExcludingWord: 'a,
    {
        // Even an opcode this carrier leaves at Top can fold an exact retained
        // expression in ValueAlgebra. Its quote must cover that opcode's real
        // concrete evaluator as well as this policy's own data copies/queries.
        let evaluation = match op {
            opcode::EXP => 16_384,
            opcode::ADDMOD | opcode::MULMOD => 4096,
            opcode::DIV | opcode::SDIV | opcode::MOD | opcode::SMOD => 1024,
            _ => 512,
        };
        args.fold(
            self.configuration.len().saturating_add(evaluation),
            |cost, value| cost.saturating_add(value.work_size().saturating_mul(8)),
        )
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        // Fixed literal fields/empty payload and common metadata, including drop.
        64
    }
    fn projection_work(&self, value: &ExcludingWord) -> usize {
        value.work_size()
    }
    fn refinement_work(&self, value: &ExcludingWord, facts: usize) -> usize {
        value.work_size().saturating_add(facts.saturating_mul(512))
    }
    fn pack_word_numeric(&self, bytes: &[&ExcludingWord; 32]) -> Option<ExcludingWord> {
        let mut packed = [0u8; 32];
        for (byte, slot) in bytes.iter().zip(&mut packed) {
            let value = byte.singleton()?;
            if value > U256::from(255) {
                return None;
            }
            *slot = value.to::<u8>();
        }
        Some(ExcludingWord::constant(U256::from_be_bytes(packed)))
    }
}
