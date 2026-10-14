//! Real branch projection, followed by ordinary instruction and return consumers.

use super::{
    AbstractValue, Domain, Execution, ExecutionConfig, ExprId, RelationState, SmtProvider,
    TransferContext, U256, WorkBudget, opcode,
};
use crate::{
    Fork,
    analysis::{MachinePayload, OutcomeKind, transfer::StorageReadPolicy},
    domain::{identity::Symbol, provenance::Origin, symbolic::SlotStateId},
    world::{Account, Entry, World},
};
use alloy_primitives::Address;

const PC: usize = 7;

fn operation(code: u8, args: &[ExprId]) -> ExprId {
    ExprId::operation(code, args, Default::default()).unwrap()
}

fn context<'a>(config: &'a ExecutionConfig, budget: &'a mut WorkBudget) -> TransferContext<'a> {
    TransferContext {
        config,
        domain: Domain::default(),
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    }
}

fn execution(payload: MachinePayload) -> Execution {
    super::super::super::empty_execution(payload)
}

fn read(key: &ExprId, index: &ExprId, word: u64, definition: u32) -> AbstractValue {
    let state = SlotStateId::zero()
        .store(
            key.clone(),
            ExprId::constant(U256::from(word)),
            Default::default(),
        )
        .unwrap();
    let expression = ExprId::slot_read(state, index.clone(), Default::default()).unwrap();
    let mut value = AbstractValue::top()
        .with_symbol(Symbol::CalldataWord(U256::from(definition)), Some(501))
        .with_identity(502, definition)
        .with_expression(expression);
    value.set_origin(Origin::Storage);
    value
}

struct Fixture {
    world: World,
    entry: Entry,
    before: MachinePayload,
    condition: AbstractValue,
    write_key: ExprId,
    read_key: ExprId,
}

impl Fixture {
    fn new(count: usize) -> Self {
        assert!((2..=16).contains(&count));
        let address = Address::repeat_byte(0x11);
        let mut world = World::offline(Fork::Osaka, "joint branch", "symbolic storage alias");
        // ADD consumes a projected stack value. RETURN exposes its bytes as a
        // real outcome, so equal branch values alone cannot satisfy the test.
        world
            .insert(
                address,
                Account::from_hex("60000160005260206000f3", world.fork()).unwrap(),
            )
            .unwrap();
        let entry = Entry::new(address);
        let mut before = super::super::super::initial(&world, &entry, Domain::default()).unwrap();
        let write_key = ExprId::fresh().unwrap();
        let read_key = ExprId::fresh().unwrap();
        let difference = operation(opcode::XOR, &[write_key.clone(), read_key.clone()]);
        let shifted = operation(
            opcode::SHL,
            &[ExprId::constant(U256::ONE), difference.clone()],
        );
        let condition = operation(
            opcode::ISZERO,
            &[operation(opcode::OR, &[difference, shifted])],
        );
        let mut values = (0..count)
            .map(|index| read(&write_key, &read_key, 100 + index as u64, index as u32))
            .collect::<Vec<_>>();
        before.active_mut().call_value = values.pop().unwrap();
        before.active_mut().stack = values;
        Self {
            world,
            entry,
            before,
            condition: AbstractValue::top().with_expression(condition),
            write_key,
            read_key,
        }
    }

    fn multiple(count: usize) -> Self {
        let mut fixture = Self::new(count);
        let difference = operation(
            opcode::XOR,
            &[fixture.write_key.clone(), fixture.read_key.clone()],
        );
        fixture.condition = AbstractValue::top().with_expression(operation(
            opcode::ISZERO,
            &[operation(
                opcode::AND,
                &[difference, ExprId::constant(U256::ONE)],
            )],
        ));
        let mut values = (0..count)
            .map(|index| {
                read(
                    &fixture.write_key,
                    &ExprId::fresh().unwrap(),
                    100 + index as u64,
                    index as u32,
                )
            })
            .collect::<Vec<_>>();
        fixture.before.active_mut().call_value = values.pop().unwrap();
        fixture.before.active_mut().stack = values;
        fixture
    }

    /// Retain the guard through branch itself while leaving no scalar targets
    /// there. The reference then calls the unchanged individual projection.
    fn frozen(&self, context: &mut TransferContext<'_>) -> RelationState {
        let mut guard = execution(self.before.clone());
        guard.payload.active_mut().stack.clear();
        guard.payload.active_mut().call_value = AbstractValue::constant(U256::ZERO);
        let successor =
            super::super::branch(&mut guard, &self.condition, true, context, PC).unwrap();
        assert!(guard.frontiers.is_empty());
        assert!(guard.diagnostics.is_empty());
        successor.relations
    }

    fn branch(&self, context: &mut TransferContext<'_>) -> MachinePayload {
        let mut result = execution(self.before.clone());
        let successor =
            super::super::branch(&mut result, &self.condition, true, context, PC).unwrap();
        assert_eq!(
            result.payload, self.before,
            "the predecessor stays unchanged"
        );
        assert!(result.frontiers.is_empty());
        assert!(result.diagnostics.is_empty());
        successor
    }
}

fn values(payload: &MachinePayload) -> impl Iterator<Item = &AbstractValue> {
    payload
        .active()
        .stack
        .iter()
        .chain([&payload.active().call_value])
}

fn individual(
    before: &MachinePayload,
    frozen: &RelationState,
    context: &mut TransferContext<'_>,
) -> MachinePayload {
    let mut reference = before.clone();
    reference.relations = frozen.clone();
    let mut diagnostics = Vec::new();
    assert_eq!(
        super::super::joint::individual(&mut reference, frozen, context, &mut diagnostics, PC),
        Ok(true)
    );
    assert_eq!(
        super::super::state::refine(&mut reference, frozen, context, &mut diagnostics, PC),
        Ok(true)
    );
    assert!(diagnostics.is_empty());
    reference
}

fn assert_values(actual: &MachinePayload, reference: &MachinePayload, before: &MachinePayload) {
    assert_eq!(actual.active().stack.len(), before.active().stack.len());
    assert_eq!(reference.active().stack.len(), before.active().stack.len());
    for ((actual, reference), before) in values(actual).zip(values(reference)).zip(values(before)) {
        assert_eq!(actual.numeric(), reference.numeric());
        assert_eq!(actual.expression(), before.expression());
        assert_eq!(reference.expression(), before.expression());
        assert_eq!(actual.provenance(), before.provenance());
        assert_eq!(reference.provenance(), before.provenance());
        assert_eq!(actual.identity().input(), before.identity().input());
        assert_eq!(reference.identity().input(), before.identity().input());
        // ValueIdentity equality excludes runtime definitions; check the
        // original must-alias relationship as well as persistent input IDs.
        assert!(actual.same_identity(before));
        assert!(reference.same_identity(before));
    }
}

fn assert_pending(before: &MachinePayload, frozen: &RelationState, config: &ExecutionConfig) {
    let mut budget = WorkBudget::new(config.max_work);
    let mut context = context(config, &mut budget);
    let mut expressions = Vec::new();
    for original in values(before) {
        assert!(original.singleton().is_none());
        assert!(original.constants().is_none(), "no finite pair shortcut");
        let mut value = original.clone();
        let prepared = super::super::prepare_refine(&mut value, frozen, &mut context).unwrap();
        let super::super::PreparedRefinement::Pending(expression) = prepared else {
            panic!("the fixture must survive the original local and transfer-cache paths");
        };
        assert_eq!(value, *original);
        assert!(expression.has_state_semantics());
        assert!(!expressions.contains(&expression));
        expressions.push(expression);
    }
    assert!(expressions.len() >= 2);
    assert_eq!(context.budget.smt_cache_stats().entries, 0);
}

fn continue_execution(
    fixture: &Fixture,
    payload: MachinePayload,
    config: &ExecutionConfig,
) -> Execution {
    super::super::super::execute(
        &fixture.world,
        &fixture.entry,
        payload,
        config,
        Domain::default(),
        &mut WorkBudget::new(config.max_work),
        StorageReadPolicy::Abstract,
    )
}

#[test]
fn actual_branch_cold_joint_fixture() {
    let config = ExecutionConfig::default();
    let fixture = Fixture::new(16);
    let mut budget = WorkBudget::new(config.max_work);
    let successor = fixture.branch(&mut context(&config, &mut budget));
    assert_pending(&fixture.before, &successor.relations, &config);
    for (index, value) in values(&successor).enumerate() {
        assert_eq!(value.singleton(), Some(U256::from(100 + index)));
    }
    assert!(!budget.exhausted());
}

#[test]
fn actual_branch_cold_oracle_fixture() {
    let config = ExecutionConfig::default();
    let fixture = Fixture::new(16);
    let mut budget = WorkBudget::new(config.max_work);
    let mut context = context(&config, &mut budget);
    let frozen = fixture.frozen(&mut context);
    assert_pending(&fixture.before, &frozen, &config);
    let reference = individual(&fixture.before, &frozen, &mut context);
    for (index, value) in values(&reference).enumerate() {
        assert_eq!(value.singleton(), Some(U256::from(100 + index)));
    }
    assert!(!budget.exhausted());
}

#[test]
fn actual_branch_multiple_joint_fixture() {
    let config = ExecutionConfig::default();
    let fixture = Fixture::multiple(16);
    let mut budget = WorkBudget::new(config.max_work);
    let successor = fixture.branch(&mut context(&config, &mut budget));
    assert_pending(&fixture.before, &successor.relations, &config);
    assert_values(&successor, &fixture.before, &fixture.before);
    assert!(!budget.exhausted());
}

#[test]
fn actual_branch_multiple_oracle_fixture() {
    let config = ExecutionConfig::default();
    let fixture = Fixture::multiple(16);
    let mut budget = WorkBudget::new(config.max_work);
    let mut context = context(&config, &mut budget);
    let frozen = fixture.frozen(&mut context);
    assert_pending(&fixture.before, &frozen, &config);
    let reference = individual(&fixture.before, &frozen, &mut context);
    assert_values(&reference, &fixture.before, &fixture.before);
    assert!(!budget.exhausted());
}

#[test]
fn original_single_value_proofs_still_win_in_a_new_transfer_context() {
    let config = ExecutionConfig::default();
    let fixture = Fixture::new(3);
    let mut budget = WorkBudget::new(config.max_work);
    let frozen = fixture.frozen(&mut context(&config, &mut budget));
    let reference = individual(&fixture.before, &frozen, &mut context(&config, &mut budget));
    let before = budget.smt_cache_stats();
    assert!(before.insertions >= 3);
    // The next transfer owns a fresh local QueryCache, while its analysis
    // retains exactly the old complete single-target mathematical queries.
    let successor = fixture.branch(&mut context(&config, &mut budget));
    assert_values(&successor, &reference, &fixture.before);
    assert_eq!(budget.smt_cache_stats().unique_hits - before.unique_hits, 3);
    assert!(!budget.exhausted());
}

#[test]
fn actual_branch_matches_individual_values_instruction_receipts_and_return_outcomes() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let mut config = ExecutionConfig::default();
        config.analysis.relations.provider = provider;
        let fixture = Fixture::new(3);
        let mut budget = WorkBudget::new(config.max_work);
        let successor = fixture.branch(&mut context(&config, &mut budget));
        let frozen = successor.relations.clone();
        assert_pending(&fixture.before, &frozen, &config);
        let reference = individual(
            &fixture.before,
            &frozen,
            &mut context(&config, &mut WorkBudget::new(config.max_work)),
        );
        assert_values(&successor, &reference, &fixture.before);
        for (index, value) in values(&successor).enumerate() {
            assert_eq!(value.singleton(), Some(U256::from(100 + index)));
        }
        let actual = continue_execution(&fixture, successor, &config);
        let expected = continue_execution(&fixture, reference, &config);
        assert!(!actual.instruction_values.is_empty());
        assert_eq!(actual.instruction_values, expected.instruction_values);
        assert_eq!(actual.executed_pcs, expected.executed_pcs);
        assert_eq!(actual.instruction_progress, expected.instruction_progress);
        let add = actual
            .instruction_values
            .iter()
            .find(|value| value.instruction_index() == 1)
            .expect("ADD must emit an actual normal-result receipt");
        assert_eq!(add.numeric().singleton(), Some(U256::from(101)));
        assert!(actual.frontiers.is_empty());
        assert!(expected.frontiers.is_empty());
        assert!(actual.diagnostics.is_empty());
        assert!(expected.diagnostics.is_empty());
        assert_eq!(actual.outcomes.len(), 1);
        assert_eq!(expected.outcomes.len(), 1);
        assert_eq!(actual.outcomes[0].kind, OutcomeKind::Return);
        assert_eq!(actual.outcomes[0].kind, expected.outcomes[0].kind);
        assert_eq!(actual.outcomes[0].data, expected.outcomes[0].data);
        assert_eq!(actual.outcomes[0].store, expected.outcomes[0].store);
        assert_eq!(
            actual.outcomes[0]
                .data
                .read_word(&AbstractValue::constant(U256::ZERO), Domain::default())
                .singleton(),
            Some(U256::from(101))
        );
    }
}
