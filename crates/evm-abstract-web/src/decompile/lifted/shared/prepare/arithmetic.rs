//! Only admitted arithmetic consumers are cached; ordinary syntax windows
//! retain the lightweight base renderer and exact source selection identities.
use super::super::super::work::Work;
use evm_abstract_protocol::decompile_model::{
    EntityId, ExitKind, Ir, LiftedIfPlan, LiftedProgram, LiftedRenderError, LiftedRenderIndex,
    LiftedType, Terminator,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Prepared {
    expressions: BTreeMap<EntityId, String>,
    conditions: BTreeMap<EntityId, [String; 2]>,
    statements: BTreeMap<EntityId, [String; 2]>,
    transfers: BTreeMap<EntityId, Option<String>>,
    exits: BTreeMap<EntityId, String>,
    require_headers: BTreeMap<EntityId, String>,
    failure_conditions: BTreeMap<EntityId, String>,
    call_rows: BTreeMap<EntityId, BTreeSet<EntityId>>,
}

impl Prepared {
    pub(super) fn new(
        ir: &Ir,
        program: &LiftedProgram,
        renderer: &LiftedRenderIndex<'_>,
        work: &Work,
    ) -> Result<Self, LiftedRenderError> {
        let mut result = Self::default();
        // Cache only consumers whose expression DAG reaches an admitted
        // arithmetic call. Ordinary windows keep their lightweight base index;
        // they never repeat the source-flow proof or retain every statement.
        if !work.reserve(program.invocations.len()) {
            return Err(LiftedRenderError::Limit);
        }
        let mut affected: BTreeSet<_> = program
            .invocations
            .iter()
            .filter(|call| renderer.expression(call.source).is_some())
            .map(|call| call.source)
            .collect();
        if !work.reserve(affected.len()) {
            return Err(LiftedRenderError::Limit);
        }
        if !affected.is_empty() {
            let mut parents: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
            let mut consumers: BTreeMap<EntityId, BTreeSet<EntityId>> = BTreeMap::new();
            for expression in &program.expressions {
                let operands = expression.kind.operands();
                if !work.reserve(operands.len().saturating_add(1)) {
                    return Err(LiftedRenderError::Limit);
                }
                if !affected.contains(&expression.id)
                    && !operands.iter().any(|operand| affected.contains(operand))
                {
                    continue;
                }
                affected.insert(expression.id);
                for operand in operands
                    .into_iter()
                    .filter(|operand| affected.contains(operand))
                {
                    parents.entry(operand).or_default().push(expression.id);
                }
                result.expressions.insert(
                    expression.id,
                    renderer.render_expression(expression.id, |amount| work.reserve(amount))?,
                );
                if expression.ty == LiftedType::Predicate {
                    result.conditions.insert(
                        expression.id,
                        [
                            renderer.render_condition(expression.id, false, |amount| {
                                work.reserve(amount)
                            })?,
                            renderer.render_condition(expression.id, true, |amount| {
                                work.reserve(amount)
                            })?,
                        ],
                    );
                }
            }
            for block in &program.blocks {
                for statement in &block.statements {
                    if !work.reserve(statement.inputs.len().saturating_add(1)) {
                        return Err(LiftedRenderError::Limit);
                    }
                    if statement
                        .inputs
                        .iter()
                        .any(|input| affected.contains(input))
                    {
                        for input in statement
                            .inputs
                            .iter()
                            .filter(|input| affected.contains(input))
                        {
                            consumers.entry(*input).or_default().insert(statement.id);
                        }
                        result.statements.insert(
                            statement.id,
                            [
                                renderer.render_statement_with_dependencies(
                                    statement,
                                    false,
                                    |amount| work.reserve(amount),
                                )?,
                                renderer.render_statement_with_dependencies(
                                    statement,
                                    true,
                                    |amount| work.reserve(amount),
                                )?,
                            ],
                        );
                    }
                }
                if !work.reserve(block.exit_values.len().saturating_add(1)) {
                    return Err(LiftedRenderError::Limit);
                }
                if block
                    .exit_values
                    .iter()
                    .any(|input| affected.contains(input))
                    && let Some(Terminator::Exit { kind, .. }) =
                        ir.block(block.block).map(|block| &block.terminator)
                {
                    let owner = block
                        .statements
                        .iter()
                        .find(|statement| {
                            ir.block(block.block).is_some_and(|raw| {
                                raw.operations.iter().any(|operation| {
                                    operation.id == statement.operation
                                        && operation.effects.terminate
                                })
                            })
                        })
                        .map_or(block.id, |statement| statement.id);
                    for input in block
                        .exit_values
                        .iter()
                        .filter(|input| affected.contains(input))
                    {
                        consumers.entry(*input).or_default().insert(owner);
                    }
                    let data = if *kind == ExitKind::Revert {
                        renderer.render_data_revert(block.block, |amount| work.reserve(amount))?
                    } else {
                        None
                    };
                    let text = match data {
                        Some(text) => text,
                        None => renderer.render_exit(*kind, &block.exit_values, |amount| {
                            work.reserve(amount)
                        })?,
                    };
                    result.exits.insert(block.block, text);
                }
            }
            for transfer in &program.transfers {
                if !work.reserve(transfer.arguments.len().saturating_add(1)) {
                    return Err(LiftedRenderError::Limit);
                }
                if transfer
                    .arguments
                    .iter()
                    .any(|input| affected.contains(input))
                {
                    for input in transfer
                        .arguments
                        .iter()
                        .filter(|input| affected.contains(input))
                    {
                        consumers
                            .entry(*input)
                            .or_default()
                            .extend([transfer.id, transfer.edge]);
                    }
                    result.transfers.insert(
                        transfer.edge,
                        renderer.render_transfer(transfer, |amount| work.reserve(amount))?,
                    );
                }
            }
            if let Some(root) = &ir.structured {
                for control in &program.controls {
                    if !work.reserve(1) {
                        return Err(LiftedRenderError::Limit);
                    }
                    if !affected.contains(&control.condition) {
                        continue;
                    }
                    consumers
                        .entry(control.condition)
                        .or_default()
                        .extend([control.id, control.node]);
                    match renderer.if_plan(control.node, root.root.id) {
                        LiftedIfPlan::Require(plan) => {
                            result.require_headers.insert(
                                control.node,
                                renderer.render_require(plan, |amount| work.reserve(amount))?,
                            );
                        }
                        LiftedIfPlan::FailureFirst(plan) => {
                            result.failure_conditions.insert(
                                control.node,
                                renderer.render_failure_first_condition(plan, |amount| {
                                    work.reserve(amount)
                                })?,
                            );
                        }
                        LiftedIfPlan::Original => {}
                    }
                }
            }
            for call in &program.invocations {
                if renderer.expression(call.source).is_none() {
                    continue;
                }
                let mut pending = vec![call.source];
                let mut seen = BTreeSet::new();
                let mut rows = BTreeSet::new();
                while let Some(expression) = pending.pop() {
                    if !work.reserve(1) {
                        return Err(LiftedRenderError::Limit);
                    }
                    if !seen.insert(expression) {
                        continue;
                    }
                    if let Some(owners) = consumers.get(&expression) {
                        if !work.reserve(owners.len()) {
                            return Err(LiftedRenderError::Limit);
                        }
                        rows.extend(owners);
                    }
                    if let Some(next) = parents.get(&expression) {
                        if !work.reserve(next.len()) {
                            return Err(LiftedRenderError::Limit);
                        }
                        pending.extend(next);
                    }
                }
                result.call_rows.insert(call.id, rows);
            }
        }
        Ok(result)
    }

    pub(super) fn expression(&self, id: EntityId) -> Option<&str> {
        self.expressions.get(&id).map(String::as_str)
    }
    pub(super) fn condition(&self, id: EntityId, invert: bool) -> Option<&str> {
        self.conditions
            .get(&id)
            .map(|text| text[usize::from(invert)].as_str())
    }
    pub(super) fn statement(&self, id: EntityId, state: bool) -> Option<&str> {
        self.statements
            .get(&id)
            .map(|text| text[usize::from(state)].as_str())
    }
    pub(super) fn transfer(&self, edge: EntityId) -> Option<Option<&str>> {
        self.transfers.get(&edge).map(|text| text.as_deref())
    }
    pub(super) fn exit(&self, block: EntityId) -> Option<&str> {
        self.exits.get(&block).map(String::as_str)
    }
    pub(super) fn require_header(&self, node: EntityId) -> Option<&str> {
        self.require_headers.get(&node).map(String::as_str)
    }
    pub(super) fn failure_condition(&self, node: EntityId) -> Option<&str> {
        self.failure_conditions.get(&node).map(String::as_str)
    }
    pub(super) fn row_matches(&self, row: EntityId, selected: EntityId) -> bool {
        row == selected
            || self
                .call_rows
                .get(&selected)
                .is_some_and(|rows| rows.contains(&row))
    }
}
