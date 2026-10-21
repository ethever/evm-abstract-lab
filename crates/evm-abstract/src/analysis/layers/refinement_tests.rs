//! Refusal by a native carrier must not publish an initial-fact revision.

use super::{AnalysisSession, FactKey, Observation, Observed, SessionError};
use crate::{
    Address, Fork, U256,
    analysis::{
        AnalysisEngine, AnalysisInput, EnginePolicies, ExecutionConfig, control::Control,
        cooperation::NoExtension,
    },
    bytecode::Program,
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, FiniteConstantSet, NumericReduction, ValueAlgebra,
        WordElement, WordObservation, facts::FactError,
    },
    world::{Code, EvmAccount, EvmEntry, EvmWorld},
};
use serde::Serialize;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct RefusingWord(FiniteConstantSet);
impl fmt::Display for RefusingWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}
impl WordElement for RefusingWord {
    fn top() -> Self {
        Self(FiniteConstantSet::top())
    }
    fn constant(value: U256) -> Self {
        Self(FiniteConstantSet::constant(value))
    }
    fn from_observation(value: &WordObservation) -> Self {
        Self(FiniteConstantSet::from_observation(value))
    }
    fn observation(&self) -> WordObservation {
        self.0.observation()
    }
    fn is_top(&self) -> bool {
        self.0.is_top()
    }
    fn work_size(&self) -> usize {
        self.0.work_size()
    }
    fn constrain(&self, _: &WordObservation) -> Result<Self, FactError> {
        Err(FactError::Capacity {
            max_atoms: 1,
            required: 2,
        })
    }
    fn constrain_work(&self, _: &WordObservation) -> usize {
        64
    }
}

// Independent conservative semantics; the session must retain this carrier's
// actual refinement refusal instead of reconstructing a builtin numeric value.
#[derive(Debug)]
struct RefusingAlgebra;
impl EvmNumericAlgebra for RefusingAlgebra {
    type Element = RefusingWord;
    fn spec(&self) -> DomainSpec {
        DomainSpec::default()
    }
    fn project_numeric(&self, value: &RefusingWord) -> RefusingWord {
        value.clone()
    }
    fn join_numeric(&self, a: &RefusingWord, b: &RefusingWord) -> RefusingWord {
        if a == b {
            a.clone()
        } else {
            RefusingWord::top()
        }
    }
    fn widen_numeric(&self, a: &RefusingWord, b: &RefusingWord) -> RefusingWord {
        self.join_numeric(a, b)
    }
    fn widen_procedure_numeric(&self, a: &RefusingWord, b: &RefusingWord) -> RefusingWord {
        self.join_numeric(a, b)
    }
    fn reduce_numeric(&self, value: &RefusingWord) -> NumericReduction<RefusingWord> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        _: u8,
        _: impl ExactSizeIterator<Item = &'a RefusingWord>,
    ) -> NumericReduction<RefusingWord> {
        NumericReduction::stable(RefusingWord::top())
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a RefusingWord>) -> usize {
        let (work, count) = args.fold((64usize, 0usize), |(work, count), value| {
            (
                work.saturating_add(value.work_size()),
                count.saturating_add(1),
            )
        });
        // Native Top is cheap, but the facade can evaluate a retained constant
        // expression once even when this independent numeric policy declines.
        if crate::domain::concrete::arity(op) == Some(count) {
            work.saturating_add(crate::domain::concrete::evaluation_work(op))
        } else {
            work
        }
    }
    fn projection_work(&self, value: &RefusingWord) -> usize {
        value.clone_work()
    }
    fn refinement_work(&self, value: &RefusingWord, _: usize) -> usize {
        value.clone_work().saturating_add(64)
    }
}

#[test]
fn refusing_carrier_quote_prepays_retained_max_exp_before_facade_folding() {
    use crate::domain::{
        concrete::{EVALUATIONS, evaluation_work},
        symbolic::ExprId,
    };
    use crate::resource::WorkBudget;
    use revm_bytecode::opcode;

    let _evaluations = EVALUATIONS.scope();
    let algebra = ValueAlgebra::new(RefusingAlgebra);
    let args = [U256::from(3), U256::MAX]
        .map(|word| EvmValue::<RefusingWord>::top().with_expression(ExprId::constant(word)));
    let quote = algebra.operation_work(opcode::EXP, &args);
    assert!(quote >= evaluation_work(opcode::EXP));
    assert_eq!(EVALUATIONS.get(), (0, 0));
    let mut denied = WorkBudget::new(evaluation_work(opcode::EXP) - 1);
    assert!(
        algebra
            .apply_budgeted(opcode::EXP, &args, &mut denied)
            .is_err()
    );
    assert_eq!(denied.used(), 0);
    assert_eq!(EVALUATIONS.get(), (0, 0));
    let result = algebra
        .apply_budgeted(opcode::EXP, &args, &mut WorkBudget::new(quote))
        .unwrap();
    let inverse = (U256::MAX / U256::from(3)) * U256::from(2) + U256::ONE;
    assert_eq!(result.value.singleton(), Some(inverse));
    assert_eq!(EVALUATIONS.get(), (1, evaluation_work(opcode::EXP)));
}

#[test]
fn native_capacity_refusal_retains_cause_and_does_not_publish_facts() {
    let owner = Address::repeat_byte(0x31);
    let mut world = EvmWorld::new(Fork::Osaka, "native refinement refusal");
    let mut account = EvmAccount::<RefusingWord>::unknown();
    account.code = Code::Runtime(Program::from_hex("00").unwrap());
    account.balance = EvmValue::top();
    world.insert(owner, account).unwrap();
    let input = AnalysisInput::new(world, EvmEntry::new(owner)).unwrap();
    let values = ValueAlgebra::new(RefusingAlgebra);
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let engine = AnalysisEngine::new(&values, &policies, &mut extension);
    let mut session = AnalysisSession::new(engine, input, ExecutionConfig::default()).unwrap();
    let snapshot = session.facts().snapshot().clone();
    let revision = session.facts().revision();
    let rejected = session
        .refine(
            &snapshot,
            [Observation::Balance {
                address: owner,
                value: U256::from(7),
            }],
            &Control::default(),
        )
        .unwrap_err();
    assert!(matches!(rejected, SessionError::Refinement {
        key: FactKey::Balance(address), cause: FactError::Capacity { max_atoms: 1, required: 2 },
    } if address == owner));
    assert_eq!(session.facts().revision(), revision);
    assert_eq!(session.facts().balance(owner), Observed::Unobserved);
    assert!(
        session.work_used() > 0,
        "refused native work remains charged"
    );
}

#[test]
fn implicit_absence_and_completeness_reject_before_publishing_or_invalidating_cache() {
    use crate::domain::NumericValue;
    use crate::world::Existence;
    use std::sync::Arc;

    let owner = Address::repeat_byte(0x31);
    let neighbor = Address::repeat_byte(0x32);
    for cached in [false, true] {
        for absent in [false, true] {
            let values = ValueAlgebra::new(crate::domain::ProductAlgebra::default());
            let policies = EnginePolicies::default();
            let mut extension = NoExtension;
            let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "implicit candidate facts");
            world
                .insert(owner, EvmAccount::from_hex("00", Fork::Osaka).unwrap())
                .unwrap();
            let mut account = EvmAccount::unknown();
            assert_eq!(account.existence, Existence::Unknown);
            if absent {
                account.balance = EvmValue::constant(U256::ONE);
            } else {
                account
                    .storage
                    .insert(U256::ZERO, EvmValue::constant(U256::ONE));
            }
            world.insert(neighbor, account).unwrap();
            let input = AnalysisInput::new(world, EvmEntry::new(owner)).unwrap();
            let engine = AnalysisEngine::new(&values, &policies, &mut extension);
            let mut session =
                AnalysisSession::new(engine, input, ExecutionConfig::default()).unwrap();
            let control = Control::default();
            let report = cached.then(|| session.run(&control).unwrap());
            let snapshot = session.facts().snapshot().clone();
            let revision = session.facts().revision();
            let observation = if absent {
                Observation::Existence {
                    address: neighbor,
                    exists: false,
                }
            } else {
                Observation::StorageComplete { address: neighbor }
            };
            let rejected = session
                .refine(&snapshot, [observation], &control)
                .unwrap_err();
            let expected = if absent {
                FactKey::Balance(neighbor)
            } else {
                FactKey::Storage {
                    address: neighbor,
                    slot: U256::ZERO,
                }
            };
            assert!(
                matches!(rejected, SessionError::Facts(super::FactError::Conflict { key }) if key == expected)
            );
            assert_eq!(session.facts().revision(), revision);
            assert_eq!(session.facts().existence(neighbor), Observed::Unobserved);
            assert!(!session.facts().storage_complete(neighbor));
            if let Some(before) = report {
                assert!(Arc::ptr_eq(&before, &session.run(&control).unwrap()));
            }
            if !absent {
                session
                    .refine(
                        &snapshot,
                        [
                            Observation::StorageComplete { address: neighbor },
                            Observation::Storage {
                                address: neighbor,
                                slot: U256::ZERO,
                                value: U256::ONE,
                            },
                        ],
                        &control,
                    )
                    .unwrap();
                assert_ne!(session.facts().revision(), revision);
                assert!(session.facts().storage_complete(neighbor));
                assert_eq!(
                    session.facts().storage(neighbor, U256::ZERO),
                    Observed::Known(U256::ONE)
                );
            }
        }
    }
}
