//! Real production assembly with independent mask/range removal controls.
use alloy_primitives::{Address, U256, hex};
use analysis_kernel::Reduction;
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisEngine, AnalysisInput};
use evm_abstract::{
    Fork,
    analysis::{EnginePolicies, EvmWorldAnalysis, ExecutionConfig, OutcomeKind, Status},
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, NumericValue, ValueAlgebra, WordElement,
        WordObservation,
        cooperation::{
            BoundsToMask, KeepBounds, KeepMasks, MaskToBounds, ProductAlgebra, RuleWork,
        },
        known_bits::KnownBits,
        relational::RelationLimits,
    },
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use revm_bytecode::opcode;
use std::{cell::Cell, num::NonZeroUsize};

#[derive(Clone, Copy, Debug)]
struct Counted<'a, R> {
    calls: &'a Cell<usize>,
    rule: R,
}
impl<R: RuleWork> RuleWork for Counted<'_, R> {
    fn evaluation_work(&self, spec: DomainSpec, finite: usize) -> usize {
        self.rule.evaluation_work(spec, finite)
    }
}
impl<Cx, R: Reduction<Cx>> Reduction<Cx> for Counted<'_, R> {
    type Task = R::Task;
    type Error = R::Error;
    fn reduce(
        &mut self,
        task: &R::Task,
        cx: &mut Cx,
    ) -> Result<analysis_kernel::LocalClosure, R::Error> {
        self.calls.set(self.calls.get() + 1);
        self.rule.reduce(task, cx)
    }
}
fn spec() -> DomainSpec {
    DomainSpec::new(
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(16).unwrap(),
        NonZeroUsize::new(256).unwrap(),
    )
    .with_relations(RelationLimits {
        enabled: false,
        ..RelationLimits::default()
    })
}

fn run<D: EvmNumericAlgebra<Element = NumericValue>>(
    numeric: D,
    initial: NumericValue,
    operations: &[u8],
) -> (EvmWorldAnalysis<NumericValue>, Vec<EvmValue<NumericValue>>) {
    let address = Address::from_word(U256::from(0x101).into());
    let mut code = vec![opcode::PUSH0, opcode::SLOAD];
    code.extend(operations);
    code.extend([
        opcode::DUP1,
        opcode::PUSH1,
        1,
        opcode::SSTORE,
        opcode::PUSH0,
        opcode::MSTORE,
        opcode::PUSH1,
        32,
        opcode::PUSH0,
        opcode::RETURN,
    ]);
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "production bit-range ablation");
    let mut account =
        EvmAccount::<NumericValue>::from_hex(&hex::encode(code), Fork::Osaka).unwrap();
    account.storage_unknown = false;
    account
        .storage
        .insert(U256::ZERO, EvmValue::<NumericValue>::from_numeric(initial));
    world.insert(address, account).unwrap();
    let algebra = ValueAlgebra::new(numeric);
    let limits = ExecutionConfig {
        max_work: 1_000_000_000,
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    };
    let result = {
        let mut extension = evm_abstract::analysis::cooperation::NoExtension;
        AnalysisEngine::new(&algebra, &EnginePolicies::default(), &mut extension).run(
            AnalysisInput::new(
                world,
                EvmEntry::<NumericValue>::concrete(
                    address,
                    address,
                    EvmValue::<NumericValue>::constant(U256::ZERO),
                    EvmByteArray::<NumericValue>::empty(),
                ),
            )
            .unwrap(),
            limits,
            &Control::default(),
        )
    }
    .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    let values: Vec<_> = result
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .map(|outcome| {
            outcome.store.read(
                address,
                &EvmValue::<NumericValue>::constant(U256::ONE),
                algebra.borrowed(),
            )
        })
        .collect();
    assert!(!values.is_empty());
    (result, values)
}

#[test]
fn removing_bounds_to_mask_loses_a_real_returned_word_guarantee() {
    let mask_calls = Cell::new(0);
    let bounds_calls = Cell::new(0);
    let initial = NumericValue::from_observation(
        &WordObservation::unsigned_range(U256::from(7), U256::from(14)).unwrap(),
    );
    // ADD creates interval 8..15 with unconstrained bits; masks must learn its
    // prefix before the following AND. Eight candidates exceed capacity one.
    let operations = [opcode::PUSH1, 1, opcode::ADD, opcode::PUSH1, 8, opcode::AND];
    let (_, full) = run(
        ProductAlgebra::new(
            spec(),
            Counted {
                calls: &mask_calls,
                rule: BoundsToMask,
            },
            Counted {
                calls: &bounds_calls,
                rule: MaskToBounds,
            },
        ),
        initial.clone(),
        &operations,
    );
    assert!(
        mask_calls.get() > 0 && bounds_calls.get() > 0,
        "both real production rules must evaluate"
    );
    assert!(
        full.iter()
            .all(|value| value.singleton() == Some(U256::from(8)))
    );
    let (_, removed) = run(
        ProductAlgebra::new(spec(), KeepMasks, MaskToBounds),
        initial,
        &operations,
    );
    assert!(
        removed.iter().any(|value| value.contains(U256::ZERO)),
        "the removal control must actually lose output precision"
    );
    for input in 7..=14u64 {
        let concrete = (input + 1) & 8;
        assert!(
            full.iter()
                .any(|value| value.contains(U256::from(concrete)))
        );
    }
}

#[test]
fn removing_mask_to_bounds_loses_an_arithmetic_range_after_the_next_operation() {
    let mask_calls = Cell::new(0);
    let bounds_calls = Cell::new(0);
    let initial = NumericValue::from_observation(&WordObservation::bits(
        KnownBits::new(!U256::from(7), U256::ZERO).unwrap(),
    ));
    // OR creates masks for 8..15 but the raw interval is 8..MAX. DIV by 3
    // keeps only a 0..7 mask approximation, so it needs the imported 8..15 range
    // to exclude 6 and 7 after the second transfer.
    let operations = [
        opcode::PUSH1,
        8,
        opcode::OR,
        opcode::PUSH1,
        3,
        opcode::SWAP1,
        opcode::DIV,
    ];
    let (_, full) = run(
        ProductAlgebra::new(
            spec(),
            Counted {
                calls: &mask_calls,
                rule: BoundsToMask,
            },
            Counted {
                calls: &bounds_calls,
                rule: MaskToBounds,
            },
        ),
        initial.clone(),
        &operations,
    );
    assert!(mask_calls.get() > 0 && bounds_calls.get() > 0);
    assert!(full.iter().all(|value| !value.contains(U256::from(6))));
    let (_, removed) = run(
        ProductAlgebra::new(spec(), BoundsToMask, KeepBounds),
        initial,
        &operations,
    );
    assert!(
        removed.iter().any(|value| value.contains(U256::from(6))),
        "range ablation must produce a strictly weaker final arithmetic answer"
    );
    for input in 0..=7u64 {
        let concrete = (input | 8) / 3;
        assert!(
            full.iter()
                .any(|value| value.contains(U256::from(concrete)))
        );
    }
}
