//! Paid arithmetic answers must reach the real stack without rediscovery.

use super::query::tests::{LITERALS, Policy, RECEIPT, Word};
use super::*;
use crate::domain::ProductAlgebra;
use crate::{
    Fork,
    domain::{
        DomainSpec, NumericReduction, ValueAlgebra,
        provenance::{OriginSet, Provenance},
        relational::QueryReason,
        symbolic::ExprId,
    },
    world::EvmAccount,
};

#[derive(Debug)]
struct Arithmetic {
    spec: DomainSpec,
    cancel_result: Option<analysis_control::Cancellation>,
}

impl EvmNumericAlgebra for Arithmetic {
    type Element = Word;

    fn spec(&self) -> DomainSpec {
        self.spec
    }
    fn project_numeric(&self, value: &Word) -> Word {
        Policy.project_numeric(value)
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        Policy.join_numeric(left, right)
    }
    fn widen_numeric(&self, old: &Word, next: &Word) -> Word {
        Policy.widen_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &Word, next: &Word) -> Word {
        Policy.widen_procedure_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        Policy.reduce_numeric(value)
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        let args: Vec<&Word> = args.collect();
        assert_eq!(op, opcode::ADD);
        NumericReduction::stable(Word {
            lower: args[0].lower.checked_add(args[1].lower).unwrap(),
            upper: args[0].upper.checked_add(args[1].upper).unwrap(),
            // A harmless native-only predicate witnesses that installing the
            // result certificate never reconstructs N from a literal/export.
            excluded: Some(U256::from(99)),
            query_tag: Some(138),
            cancel_query_owner: self.cancel_result.clone(),
            ..Word::top()
        })
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Word>) -> usize {
        Policy.operation_work(op, args)
    }
    fn literal_quote_work(&self, word: U256) -> usize {
        Policy.literal_quote_work(word)
    }
    fn projection_work(&self, value: &Word) -> usize {
        Policy.projection_work(value)
    }
    fn refinement_work(&self, value: &Word, facts: usize) -> usize {
        Policy.refinement_work(value, facts)
    }
}

struct Fixture {
    world: EvmWorld<Word>,
    entry: EvmEntry<Word>,
    input: EvmMachinePayload<Word>,
}

fn fixture(policy: &Arithmetic) -> Fixture {
    let owner = Address::repeat_byte(0x41);
    let mut world = EvmWorld::new(Fork::Osaka, "arithmetic result proof");
    // The successor lets us observe the actual stack definition, before STOP.
    world
        .insert(owner, EvmAccount::from_hex("015b00", Fork::Osaka).unwrap())
        .unwrap();
    let entry = EvmEntry::concrete(
        owner,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let mut input = initial(&world, &entry, ValueAlgebra::new(policy)).unwrap();
    input.active_mut().stack = [(4, Origin::Memory), (3, Origin::TransientStorage)]
        .into_iter()
        .enumerate()
        .map(|(index, (word, origin))| {
            EvmValue::constant(U256::from(word))
                .with_origin(origin)
                .with_identity(91, index.try_into().unwrap())
                .with_expression(ExprId::constant(U256::from(word)))
        })
        .collect();
    Fixture {
        world,
        entry,
        input,
    }
}

fn run(
    fixture: &Fixture,
    policy: &Arithmetic,
    config: &ExecutionConfig,
    budget: &mut WorkBudget,
) -> EvmExecution<Word> {
    execute(
        &fixture.world,
        &fixture.entry,
        fixture.input.clone(),
        config,
        ValueAlgebra::new(policy),
        &crate::analysis::RecentDistinctJumps,
        budget,
        StorageReadPolicy::Abstract,
    )
}

fn reset_receipt() {
    RECEIPT.update(|receipt| {
        receipt.watched = Some(U256::from(7));
        receipt.query_tag = Some(138);
        receipt.quotes = 0;
        receipt.queries = 0;
    });
    LITERALS.update(|count| *count = 0);
}

fn assert_result(result: &EvmExecution<Word>) {
    assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
    assert_eq!(
        result.instruction_progress,
        [InstructionProgress::Completed]
    );
    assert_eq!(result.successors.len(), 1);
    let value = &result.payload.active().stack[0];
    assert!(
        value.constants().is_none(),
        "the carrier has no finite view"
    );
    assert_eq!(value.numeric().lower, U256::from(7));
    assert_eq!(value.numeric().upper, U256::from(7));
    assert_eq!(value.numeric().excluded, Some(U256::from(99)));
    assert_eq!(
        value.expression().and_then(ExprId::as_constant),
        Some(U256::from(7))
    );
    assert_eq!(
        value.provenance(),
        &Provenance::from_origins(
            OriginSet::from_sources([
                Origin::Constant,
                Origin::Memory,
                Origin::TransientStorage,
                Origin::Arithmetic,
            ])
            .unwrap()
        )
    );
    assert!(value.identity().input().is_none());
    assert!(!value.symbolic_limit_reached());
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 1));
    assert_eq!(LITERALS.get(), 0);
}

#[test]
fn actual_arithmetic_reuses_one_paid_answer_and_requires_the_full_native_fee() {
    let config = ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let policy = Arithmetic {
        spec: ProductAlgebra::from_spec(config_numeric.spec()).spec(),
        cancel_result: None,
    };
    let fixture = fixture(&policy);
    let _receipt = RECEIPT.scope();
    let _literals = LITERALS.scope();
    reset_receipt();
    let mut probe = WorkBudget::new(config.max_work);
    assert_result(&run(&fixture, &policy, &config, &mut probe));
    let fee = probe.used();
    reset_receipt();
    let mut exact = WorkBudget::new(fee);
    assert_result(&run(&fixture, &policy, &config, &mut exact));
    assert_eq!(exact.used(), fee);
    reset_receipt();
    let mut short = WorkBudget::new(fee - 1);
    let refused = run(&fixture, &policy, &config, &mut short);
    assert!(
        refused
            .frontiers
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::Work)
    );
    assert!(
        !refused
            .instruction_progress
            .contains(&InstructionProgress::Completed)
    );
    assert!(refused.instruction_values.is_empty());
    assert!(refused.payload.active().stack.is_empty());
    assert!(short.exhausted());
}

#[test]
fn cancellation_during_the_paid_result_query_cannot_publish_a_literal() {
    let config = ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    for captured in [false, true] {
        let owner = analysis_control::Cancellation::new();
        let active = analysis_control::Cancellation::new();
        let policy = Arithmetic {
            spec: ProductAlgebra::from_spec(config_numeric.spec()).spec(),
            cancel_result: captured.then(|| owner.clone()),
        };
        let fixture = fixture(&policy);
        let _receipt = RECEIPT.scope();
        let _literals = LITERALS.scope();
        reset_receipt();
        RECEIPT.update(|receipt| receipt.cancel_on_query = !captured);
        let mut budget =
            analysis_control::with_cancellation(&owner, || WorkBudget::new(config.max_work));
        let result = analysis_control::with_cancellation(&active, || {
            run(&fixture, &policy, &config, &mut budget)
        });
        assert!(
            result.frontiers.iter().any(
                |frontier| frontier.reason == FrontierReason::Relations(QueryReason::Cancelled)
            )
        );
        assert!(
            !result
                .instruction_progress
                .contains(&InstructionProgress::Completed)
        );
        assert!(result.instruction_values.is_empty());
        assert!(result.payload.active().stack.is_empty());
        assert!(
            budget.used()
                >= ValueAlgebra::new(&policy)
                    .operation_work(opcode::ADD, &fixture.input.active().stack)
        );
        assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 1));
        assert_eq!(LITERALS.get(), 0);
        assert_eq!(owner.is_cancelled(), captured);
        assert_eq!(active.is_cancelled(), !captured);
    }
}

#[test]
fn relations_disabled_execution_never_discovers_or_retains_the_unqueried_result() {
    let config = ExecutionConfig::default();
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config_numeric.relations.enabled = false;
    let policy = Arithmetic {
        spec: ProductAlgebra::from_spec(config_numeric.spec()).spec(),
        cancel_result: None,
    };
    let fixture = fixture(&policy);
    let _receipt = RECEIPT.scope();
    let _literals = LITERALS.scope();
    reset_receipt();
    let mut budget = WorkBudget::new(config.max_work);
    let result = run(&fixture, &policy, &config, &mut budget);
    assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
    assert_eq!(
        result.instruction_progress,
        [InstructionProgress::Completed]
    );
    let value = &result.payload.active().stack[0];
    assert_eq!(value.numeric().lower, U256::from(7));
    assert_eq!(value.numeric().excluded, Some(U256::from(99)));
    assert!(value.expression().is_none());
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    assert_eq!(LITERALS.get(), 0);
}

#[test]
fn a_nonexact_result_keeps_its_actual_parametric_input_expression() {
    use crate::domain::{identity::Symbol, symbolic::ExprKind};

    let config = ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let policy = Arithmetic {
        spec: ProductAlgebra::from_spec(config_numeric.spec()).spec(),
        cancel_result: None,
    };
    let mut fixture = fixture(&policy);
    let input = EvmValue::from_numeric(Word {
        lower: U256::from(3),
        upper: U256::from(4),
        ..Word::top()
    })
    .with_symbol(Symbol::CallValue, Some(93));
    let original_expression = input.expression().unwrap().clone();
    let original_identity = *input.identity();
    fixture.input.active_mut().stack[1] = input;
    let _receipt = RECEIPT.scope();
    let _literals = LITERALS.scope();
    reset_receipt();
    let mut budget = WorkBudget::new(config.max_work);
    let result = run(&fixture, &policy, &config, &mut budget);
    assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
    let value = &result.payload.active().stack[0];
    assert_eq!(value.numeric().lower, U256::from(7));
    assert_eq!(value.numeric().upper, U256::from(8));
    let ExprKind::Operation { opcode: op, args } = value.expression().unwrap().kind() else {
        panic!("lost the parametric arithmetic expression");
    };
    assert_eq!(*op, opcode::ADD);
    assert!(args.contains(&original_expression));
    assert_eq!(
        fixture.input.active().stack[1].identity(),
        &original_identity
    );
}
