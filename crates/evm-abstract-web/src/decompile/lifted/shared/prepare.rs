//! Owned display data from one source-certified model index. No source witness
//! or proof algorithm is copied. The publication owns cache invalidation.
use super::super::work::Work;
use evm_abstract_protocol::decompile_model::{
    EntityId, Ir, LiftedIfPlan, LiftedIfSuffix, LiftedRenderError, SharedRenderRow,
};
use std::collections::{BTreeMap, BTreeSet};

mod arithmetic;

pub(in crate::decompile) struct Definition {
    pub id: EntityId,
    headers: [String; 2],
    bodies: [Vec<SharedRenderRow>; 2],
    invocations: Vec<EntityId>,
}

impl Definition {
    pub fn header(&self, state: bool) -> &str {
        &self.headers[usize::from(state)]
    }
    pub fn body(&self, state: bool) -> &[SharedRenderRow] {
        &self.bodies[usize::from(state)]
    }
}

struct Invocation {
    id: EntityId,
    text: [String; 2],
}

pub(in crate::decompile) struct ControlPlan {
    pub require: bool,
    pub header: Option<String>,
    pub suffix: LiftedIfSuffix,
    pub references: Vec<EntityId>,
}

#[derive(Default)]
pub(in crate::decompile) struct Prepared {
    definitions: Vec<Definition>,
    definition_ids: BTreeMap<EntityId, usize>,
    control_plans: BTreeMap<EntityId, ControlPlan>,
    source_controls: BTreeMap<EntityId, EntityId>,
    invocations: BTreeMap<EntityId, Invocation>,
    arithmetic: arithmetic::Prepared,
    error: Option<LiftedRenderError>,
}

impl Prepared {
    pub fn new(ir: &Ir, work: &Work) -> Self {
        Self::with_expanded(ir, work, &BTreeSet::new())
    }

    pub fn with_expanded(ir: &Ir, work: &Work, sources: &BTreeSet<EntityId>) -> Self {
        Self::prepare(ir, work, sources).unwrap_or_else(|error| Self {
            error: Some(error),
            ..Self::default()
        })
    }

    fn prepare(
        ir: &Ir,
        work: &Work,
        sources: &BTreeSet<EntityId>,
    ) -> Result<Self, LiftedRenderError> {
        let Some(program) = ir
            .lifted
            .as_ref()
            .filter(|program| program.has_procedures())
        else {
            return Ok(Self::default());
        };
        let mut renderer = program.render_index_with_budget(ir, |amount| work.reserve(amount));
        renderer.validate_shared_sources()?;
        renderer.validate_guard_sources()?;
        if !sources.is_empty() {
            renderer.expand_shared_sources(sources, |amount| work.reserve(amount))?;
        }
        if !work.reserve(
            program
                .procedures
                .len()
                .saturating_add(program.invocations.len())
                .saturating_mul(8),
        ) {
            return Err(LiftedRenderError::Limit);
        }
        let mut result = Self::default();
        if !work.reserve(program.controls.len()) {
            return Err(LiftedRenderError::Limit);
        }
        let controls: BTreeMap<_, _> = program
            .controls
            .iter()
            .flat_map(|control| [(control.id, control), (control.node, control)])
            .collect();
        for procedure in renderer.procedures() {
            let headers = [
                renderer
                    .render_procedure_header(procedure, false, |amount| work.reserve(amount))?,
                renderer.render_procedure_header(procedure, true, |amount| work.reserve(amount))?,
            ];
            let bodies = [
                renderer.render_procedure_body(procedure, false, |amount| work.reserve(amount))?,
                renderer.render_procedure_body(procedure, true, |amount| work.reserve(amount))?,
            ];
            if !work.reserve(bodies[1].len()) {
                return Err(LiftedRenderError::Limit);
            }
            for row in &bodies[1] {
                let Some(control) = controls.get(&row.entity) else {
                    continue;
                };
                if result.control_plans.contains_key(&control.id) {
                    continue;
                }
                // Inspector consumes the same scoped choice used to prepare
                // this canonical body, without repeating shared proof work.
                let selected = renderer.procedure_if_plan(control.node, procedure);
                let mut references = Vec::new();
                let header = match selected {
                    LiftedIfPlan::Require(plan) => {
                        references.extend([plan.failure_node(), plan.success_node()]);
                        Some(renderer.render_require(plan, |amount| work.reserve(amount))?)
                    }
                    LiftedIfPlan::FailureFirst(plan) => {
                        references.extend([plan.failure_node(), plan.success_node()]);
                        if let Some(suffix) = renderer.failure_first_suffix(plan) {
                            references.extend(suffix.iter().map(|node| node.id));
                        }
                        Some(format!(
                            "if ({}) {{",
                            renderer.render_failure_first_condition(plan, |amount| work
                                .reserve(amount))?
                        ))
                    }
                    LiftedIfPlan::Original => None,
                };
                if !work.reserve(references.len()) {
                    return Err(LiftedRenderError::Limit);
                }
                result.control_plans.insert(
                    control.id,
                    ControlPlan {
                        require: matches!(selected, LiftedIfPlan::Require(_)),
                        header,
                        suffix: selected.suffix(),
                        references,
                    },
                );
                result.source_controls.insert(control.node, control.id);
            }
            result
                .definition_ids
                .insert(procedure.id, result.definitions.len());
            result.definitions.push(Definition {
                id: procedure.id,
                headers,
                bodies,
                invocations: Vec::new(),
            });
        }
        for call in &program.invocations {
            if sources.contains(&call.source) {
                continue;
            }
            let text = [
                renderer.render_shared_invocation(call, false, |amount| work.reserve(amount))?,
                renderer.render_shared_invocation(call, true, |amount| work.reserve(amount))?,
            ];
            let definition = *result
                .definition_ids
                .get(&call.procedure)
                .ok_or(LiftedRenderError::InvalidSharedSource(call.procedure))?;
            result
                .definitions
                .get_mut(definition)
                .ok_or(LiftedRenderError::InvalidSharedSource(call.procedure))?
                .invocations
                .push(call.id);
            result
                .invocations
                .insert(call.source, Invocation { id: call.id, text });
        }
        result.arithmetic = arithmetic::Prepared::new(ir, program, &renderer, work)?;
        Ok(result)
    }

    #[cfg(test)]
    pub fn refused(&self) -> bool {
        self.error == Some(LiftedRenderError::Limit)
    }
    pub fn error(&self) -> Option<LiftedRenderError> {
        self.error
    }
    pub fn definitions(&self) -> &[Definition] {
        &self.definitions
    }
    pub fn control_plan(&self, entity: EntityId) -> Option<&ControlPlan> {
        self.control_plans.get(&entity)
    }
    pub fn require_owner(&self, source: EntityId) -> Option<EntityId> {
        let control = *self.source_controls.get(&source)?;
        self.control_plans.get(&control)?.require.then_some(control)
    }
    pub fn header(&self, id: EntityId, state: bool) -> Option<&str> {
        Some(
            self.definitions
                .get(*self.definition_ids.get(&id)?)?
                .header(state),
        )
    }
    pub fn invocations(&self, id: EntityId) -> &[EntityId] {
        self.definition_ids
            .get(&id)
            .and_then(|position| self.definitions.get(*position))
            .map_or(&[], |definition| definition.invocations.as_slice())
    }
    pub fn invocation(&self, source: EntityId, state: bool) -> Option<(EntityId, &str)> {
        let call = self.invocations.get(&source)?;
        Some((call.id, &call.text[usize::from(state)]))
    }
    pub fn expression(&self, id: EntityId) -> Option<&str> {
        self.arithmetic.expression(id)
    }
    pub fn condition(&self, id: EntityId, invert: bool) -> Option<&str> {
        self.arithmetic.condition(id, invert)
    }
    pub fn statement(&self, id: EntityId, state: bool) -> Option<&str> {
        self.arithmetic.statement(id, state)
    }
    pub fn transfer(&self, edge: EntityId) -> Option<Option<&str>> {
        self.arithmetic.transfer(edge)
    }
    pub fn exit(&self, block: EntityId) -> Option<&str> {
        self.arithmetic.exit(block)
    }
    pub fn require_header(&self, node: EntityId) -> Option<&str> {
        self.arithmetic.require_header(node)
    }
    pub fn failure_condition(&self, node: EntityId) -> Option<&str> {
        self.arithmetic.failure_condition(node)
    }
    pub fn row_matches(&self, row: EntityId, selected: EntityId) -> bool {
        self.arithmetic.row_matches(row, selected)
    }
}
