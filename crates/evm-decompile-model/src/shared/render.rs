//! Parameterized canonical body rows shared by native and browser consumers.
use super::{MAX_SHARED_REGION_ENTITIES, SharedInvocation, SharedProcedure, SharedProcedureKind};
use crate::{EntityId, LiftedIfPlan};
use crate::{
    LiftedRenderError as E, LiftedRenderIndex, StructuredKind as K, Terminator as T, ValueKind as V,
};

#[cfg(test)]
mod tests;

/// One complete semantic row with its original source-navigation entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedRenderRow {
    /// Original statement or syntax entity; its raw witness remains inspectable.
    pub entity: EntityId,
    /// Complete row text, excluding indentation/newline.
    pub text: String,
    /// Indentation relative to the procedure body.
    pub depth: usize,
    /// Optional structural annotation, controlled by the consumer's toggle.
    pub is_structure_annotation: bool,
}
fn paid(pay: &mut impl FnMut(usize) -> bool, n: usize) -> Result<(), E> {
    if pay(n) { Ok(()) } else { Err(E::Limit) }
}

#[derive(Default)]
struct BodyRows {
    rows: Vec<SharedRenderRow>,
    text_bytes: usize,
    emitted_rows: usize,
}

impl BodyRows {
    fn emit(
        &mut self,
        entity: EntityId,
        text: String,
        depth: usize,
        annotation: bool,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(), E> {
        paid(pay, text.len().saturating_add(1))?;
        self.text_bytes = self.text_bytes.saturating_add(text.len());
        if self.text_bytes > 4_194_304 || self.emitted_rows >= MAX_SHARED_REGION_ENTITIES {
            return Err(E::Limit);
        }
        self.emitted_rows += 1;
        self.rows.push(SharedRenderRow {
            entity,
            text,
            depth,
            is_structure_annotation: annotation,
        });
        Ok(())
    }
}

fn tuple(ids: &[EntityId], prefix: &str) -> String {
    ids.iter()
        .map(|id| format!("{prefix}{}", id.0))
        .collect::<Vec<_>>()
        .join(", ")
}

fn name(p: &SharedProcedure) -> String {
    format!(
        "{}_{}",
        match p.kind {
            SharedProcedureKind::Legacy | SharedProcedureKind::Normal => "shared",
            SharedProcedureKind::PureArithmetic => "pure_arithmetic",
        },
        p.id.0
    )
}

impl<'a> LiftedRenderIndex<'a> {
    fn binding_tuple(&self, ids: &[EntityId]) -> String {
        ids.iter()
            .map(|id| self.binding_name(*id))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Expand exact original occurrences in this presentation without weakening
    /// source admission or changing the artifact. Unknown identities are rejected.
    pub fn expand_shared_sources(
        &mut self,
        sources: &std::collections::BTreeSet<EntityId>,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<(), E> {
        self.validate_shared_sources()?;
        paid(&mut pay, sources.len().saturating_mul(64).saturating_add(1))?;
        for source in sources {
            if !self.shared_calls.contains_key(source) {
                return Err(E::InvalidSharedSource(*source));
            }
        }
        self.expanded_shared_sources = sources.clone();
        paid(&mut pay, self.program.invocations.len().saturating_mul(32))?;
        let substituted: std::collections::BTreeSet<_> = self
            .program
            .invocations
            .iter()
            .filter(|i| self.shared_invocation(i.source).is_some())
            .map(|i| i.source)
            .collect();
        self.set_call_result_projection(|id| substituted.contains(&id), &mut pay)?;
        Ok(())
    }
    /// Direct admission independent of stored snapshot validation status.
    pub fn validate_shared_sources(&self) -> Result<(), E> {
        self.shared_error.map_or(Ok(()), Err)
    }
    /// Only certified definitions are visible; rejected plans retain source code.
    pub fn procedures(&self) -> impl Iterator<Item = &'a SharedProcedure> {
        self.program
            .procedures
            .iter()
            .filter(|_| self.shared_error.is_none() && self.shared_source.is_some())
    }
    /// One certified reusable definition.
    pub fn procedure(&self, id: EntityId) -> Option<&'a SharedProcedure> {
        if self.shared_error.is_none() && self.shared_source.is_some() {
            self.shared_definitions.get(&id).copied()
        } else {
            None
        }
    }
    /// Replace only this certified occurrence, never a matching-looking block.
    pub fn shared_invocation(&self, source: EntityId) -> Option<&'a SharedInvocation> {
        if self.shared_error.is_some()
            || self.shared_source.is_none()
            || self.expanded_shared_sources.contains(&source)
        {
            return None;
        }
        self.shared_calls.get(&source).copied()
    }
    /// Shared definition spelling used unchanged by CLI/Web.
    pub fn render_procedure_header(
        &self,
        p: &SharedProcedure,
        state: bool,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<String, E> {
        self.validate_shared_sources()?;
        if self.procedure(p.id) != Some(p) {
            return Err(E::InvalidSharedSource(p.id));
        }
        paid(&mut pay, p.interface.record_count().saturating_mul(32))?;
        let mut text = if let Some(expression) = self.expression(p.body) {
            format!(
                "procedure {}({}) -> {} {{",
                name(p),
                tuple(&p.interface.inputs, "x"),
                match expression.ty {
                    crate::LiftedType::Word256 => "word256",
                    crate::LiftedType::Predicate => "predicate",
                }
            )
        } else {
            format!(
                "procedure {}({}) -> ({}) {{",
                name(p),
                self.binding_tuple(&p.interface.inputs),
                self.binding_tuple(&p.interface.outputs)
            )
        };
        if state {
            text.push_str(&format!(
                " // memory ({}) -> ({}); effect ({}) -> ({})",
                tuple(&p.interface.memory_inputs, "m"),
                tuple(&p.interface.memory_outputs, "m"),
                tuple(&p.interface.effect_inputs, "q"),
                tuple(&p.interface.effect_outputs, "q")
            ));
        }
        paid(&mut pay, text.len())?;
        Ok(text)
    }
    /// Each call binds fresh actual words/results in the same EVM frame/state.
    pub fn render_shared_invocation(
        &self,
        i: &SharedInvocation,
        state: bool,
        pay: impl FnMut(usize) -> bool,
    ) -> Result<String, E> {
        self.render_shared_invocation_scoped(i, state, None, pay)
    }

    fn render_shared_invocation_scoped(
        &self,
        i: &SharedInvocation,
        state: bool,
        scope: Option<&SharedProcedure>,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<String, E> {
        self.validate_shared_sources()?;
        if self.shared_invocation(i.source) != Some(i) {
            return Err(E::InvalidSharedSource(i.id));
        }
        let p = self
            .procedure(i.procedure)
            .ok_or(E::MissingWitness(i.procedure))?;
        let source = self
            .shared_source
            .as_ref()
            .ok_or(E::InvalidSharedSource(i.id))?;
        paid(&mut pay, p.interface.record_count().saturating_mul(32))?;
        let actual = |id: EntityId| i.actual(id).ok_or(E::MissingWitness(id));
        if self.expression(p.body).is_some() {
            let mut text = format!("{}(", name(p));
            for (position, id) in p.interface.inputs.iter().enumerate() {
                if position > 0 {
                    text.push_str(", ");
                }
                let argument = self.render_expression_scoped(actual(*id)?, scope, &mut pay)?;
                text.push_str(&argument);
                if text.len() > crate::MAX_LIFTED_EXPRESSION_TEXT {
                    return Err(E::Limit);
                }
            }
            text.push(')');
            paid(&mut pay, text.len())?;
            return Ok(text);
        }
        let args = p
            .interface
            .inputs
            .iter()
            .map(|id| {
                let id = actual(*id)?;
                Ok(match source.values.get(&id).map(|v| &v.kind) {
                    _ if scope
                        .is_some_and(|scope| scope.interface.inputs.binary_search(&id).is_ok()) =>
                    {
                        self.binding_name(id)
                    }
                    Some(V::Constant(w)) => format!("0x{w:x}"),
                    _ => self.binding_name(id),
                })
            })
            .collect::<Result<Vec<_>, E>>()?
            .join(", ");
        let outputs = p
            .interface
            .outputs
            .iter()
            .map(|id| actual(*id))
            .collect::<Result<Vec<_>, E>>()?;
        let mut text = String::new();
        if !outputs.is_empty() {
            text.push_str(&format!("({}) = ", self.binding_tuple(&outputs)));
        }
        text.push_str(&format!("{}({args});", name(p)));
        if state {
            for (label, prefix, inputs, outputs) in [
                (
                    "memory",
                    "m",
                    &p.interface.memory_inputs,
                    &p.interface.memory_outputs,
                ),
                (
                    "effect",
                    "q",
                    &p.interface.effect_inputs,
                    &p.interface.effect_outputs,
                ),
            ] {
                let inputs = inputs
                    .iter()
                    .map(|id| actual(*id))
                    .collect::<Result<Vec<_>, E>>()?;
                let outputs = outputs
                    .iter()
                    .map(|id| actual(*id))
                    .collect::<Result<Vec<_>, E>>()?;
                text.push_str(&format!(
                    " // {label} ({}) -> ({})",
                    tuple(&inputs, prefix),
                    tuple(&outputs, prefix)
                ));
            }
        }
        paid(&mut pay, text.len())?;
        Ok(text)
    }
    /// Complete canonical executable rows. Parameter words never inherit a
    /// canonical caller's constant folding. Every body executes independently.
    pub fn render_procedure_body(
        &self,
        p: &SharedProcedure,
        state: bool,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<Vec<SharedRenderRow>, E> {
        self.validate_shared_sources()?;
        if self.procedure(p.id) != Some(p) {
            return Err(E::InvalidSharedSource(p.id));
        }
        if self.expression(p.body).is_some() {
            let mut output = BodyRows::default();
            let mut pending = vec![(p.body, false)];
            let mut locals = std::collections::BTreeSet::new();
            let mut order = Vec::new();
            while let Some((id, complete)) = pending.pop() {
                paid(&mut pay, 1)?;
                if complete {
                    order.push(id);
                    continue;
                }
                if p.interface.inputs.binary_search(&id).is_ok() || !locals.insert(id) {
                    continue;
                }
                if locals.len() > MAX_SHARED_REGION_ENTITIES {
                    return Err(E::Limit);
                }
                if id != p.body && self.shared_invocation(id).is_some() {
                    order.push(id);
                    continue;
                }
                pending.push((id, true));
                let expression = self.expression(id).ok_or(E::MissingExpression(id))?;
                let operands = expression.kind.operands();
                paid(&mut pay, operands.len())?;
                pending.extend(operands.into_iter().rev().map(|operand| (operand, false)));
            }
            // The source-checked DAG's iterative postorder visits only this
            // body. Each node is bound once, independent of other definitions.
            for id in order {
                paid(&mut pay, 1)?;
                let text = self.render_expression_bound(id, Some(p), Some(&locals), &mut pay)?;
                let entity = self
                    .shared_invocation(id)
                    .filter(|_| id != p.body)
                    .map_or(id, |call| call.id);
                output.emit(entity, format!("x{} = {text};", id.0), 0, false, &mut pay)?;
            }
            output.emit(p.body, format!("yield x{};", p.body.0), 0, false, &mut pay)?;
            return Ok(output.rows);
        }
        let source = self
            .shared_source
            .as_ref()
            .ok_or(E::InvalidSharedSource(p.id))?;
        let layouts = self.plan_branch_layouts(
            p.body,
            |id| id != p.body && self.shared_invocation(id).is_some(),
            &mut pay,
        )?;
        enum Task {
            Node(EntityId, usize, Option<EntityId>),
            Sequence(EntityId, usize, usize, usize, Option<EntityId>),
            Text(EntityId, String, usize),
            Transfer(EntityId, usize),
            Else(EntityId, EntityId, usize, Option<EntityId>),
        }
        let mut todo = vec![Task::Node(p.body, 0, None)];
        let mut output = BodyRows::default();
        while let Some(task) = todo.pop() {
            paid(&mut pay, 1)?;
            match task {
                Task::Sequence(id, start, end, depth, loop_id) => {
                    let node = source.nodes.get(&id).ok_or(E::MissingWitness(id))?;
                    let K::Sequence(children) = &node.kind else {
                        return Err(E::MissingWitness(id));
                    };
                    let mut pos = start;
                    let mut selected = Vec::new();
                    while pos < end {
                        paid(&mut pay, 1)?;
                        if pos > start
                            && let Some(next) = self.expanded_selected_sequence_end(id, pos, p.body)
                        {
                            pos = next;
                            continue;
                        }
                        selected.push(children.get(pos).ok_or(E::MissingWitness(id))?.id);
                        pos += 1;
                    }
                    todo.extend(
                        selected
                            .into_iter()
                            .rev()
                            .map(|id| Task::Node(id, depth, loop_id)),
                    );
                }
                Task::Text(id, text, depth) => output.emit(id, text, depth, false, &mut pay)?,
                Task::Else(owner, body, depth, loop_id) => {
                    output.emit(owner, "} else {".to_owned(), depth, false, &mut pay)?;
                    todo.push(Task::Node(body, depth + 1, loop_id));
                }
                Task::Transfer(id, depth) => {
                    let transfer = self.transfer(id).ok_or(E::MissingWitness(id))?;
                    if let Some(text) = self.render_transfer(transfer, &mut pay)? {
                        output.emit(id, text, depth, false, &mut pay)?;
                    }
                    let edge = source.edges.get(&id).ok_or(E::MissingWitness(id))?;
                    let target = source
                        .blocks
                        .get(&edge.to)
                        .ok_or(E::MissingWitness(edge.to))?;
                    paid(&mut pay, 256)?;
                    for text in [
                        format!(
                            "m{} = m{}; // memory token on e{}",
                            target.memory_parameter.0, edge.memory.0, id.0
                        ),
                        format!(
                            "q{} = q{}; // effect token on e{}",
                            target.effect_parameter.0, edge.effect.0, id.0
                        ),
                    ] {
                        if state {
                            output.emit(id, text, depth, false, &mut pay)?;
                        } else {
                            paid(&mut pay, text.len().saturating_add(1))?;
                        }
                    }
                }
                Task::Node(id, depth, loop_id) => {
                    if id != p.body
                        && let Some(call) = self.shared_invocation(id)
                    {
                        let text =
                            self.render_shared_invocation_scoped(call, state, Some(p), &mut pay)?;
                        output.emit(call.id, text, depth, false, &mut pay)?;
                        continue;
                    }
                    let node = source.nodes.get(&id).ok_or(E::MissingWitness(id))?;
                    match &node.kind {
                        K::Sequence(children) => {
                            todo.push(Task::Sequence(id, 0, children.len(), depth, loop_id));
                        }
                        K::Block { block } => {
                            let b = source.blocks.get(block).ok_or(E::MissingWitness(*block))?;
                            output.emit(
                                id,
                                format!("// block b{}", block.0),
                                depth,
                                true,
                                &mut pay,
                            )?;
                            let high = self.block(*block).ok_or(E::MissingWitness(*block))?;
                            for statement in &high.statements {
                                paid(&mut pay, 1)?;
                                if matches!(b.terminator, T::Trap { .. })
                                    && b.operations
                                        .last()
                                        .is_some_and(|o| o.id == statement.operation)
                                {
                                    continue;
                                }
                                if self.scoped_statement_has_row(statement, Some(p)) {
                                    let text = self.render_statement_scoped(
                                        statement,
                                        state,
                                        Some(p),
                                        &mut pay,
                                    )?;
                                    output.emit(statement.id, text, depth, false, &mut pay)?;
                                }
                            }
                        }
                        K::Transfer { edge } => todo.push(Task::Transfer(*edge, depth)),
                        K::If { .. } => {
                            let control = self.control(id).ok_or(E::MissingWitness(id))?;
                            if let LiftedIfPlan::Require(plan) = self.expanded_if_plan(id, p.body) {
                                output.emit(
                                    id,
                                    self.render_require(plan, &mut pay)?,
                                    depth,
                                    false,
                                    &mut pay,
                                )?;
                                todo.push(Task::Node(plan.success_node(), depth, loop_id));
                            } else if let LiftedIfPlan::FailureFirst(plan) =
                                self.expanded_if_plan(id, p.body)
                            {
                                let condition =
                                    self.render_failure_first_condition(plan, &mut pay)?;
                                output.emit(
                                    id,
                                    format!("if ({condition}) {{"),
                                    depth,
                                    false,
                                    &mut pay,
                                )?;
                                todo.push(Task::Node(plan.success_node(), depth, loop_id));
                                todo.push(Task::Text(id, "}".to_owned(), depth));
                                if let Some((sequence, range)) =
                                    self.failure_first_suffix_range(plan)
                                {
                                    todo.push(Task::Sequence(
                                        sequence,
                                        range.start,
                                        range.end,
                                        depth + 1,
                                        loop_id,
                                    ));
                                }
                                todo.push(Task::Node(plan.failure_node(), depth + 1, loop_id));
                            } else {
                                let layout = layouts.get(id).ok_or(E::MissingWitness(id))?;
                                output.emit(
                                    id,
                                    format!(
                                        "if ({}) {{",
                                        self.render_condition(
                                            control.condition,
                                            layout.invert,
                                            &mut pay
                                        )?
                                    ),
                                    depth,
                                    false,
                                    &mut pay,
                                )?;
                                todo.push(Task::Text(id, "}".to_owned(), depth));
                                if let Some(otherwise) = layout.else_node {
                                    todo.push(Task::Else(id, otherwise, depth, loop_id));
                                }
                                todo.push(Task::Node(layout.then_node, depth + 1, loop_id));
                            }
                        }
                        K::While {
                            header,
                            condition,
                            condition_nonzero,
                            body,
                            exit_edge,
                        } => {
                            output.emit(
                                id,
                                format!("'loop{}: while (true) {{", id.0),
                                depth,
                                false,
                                &mut pay,
                            )?;
                            todo.push(Task::Text(id, "}".to_owned(), depth));
                            todo.push(Task::Node(body.id, depth + 1, Some(id)));
                            if condition.is_some() {
                                let control = self.control(id).ok_or(E::MissingWitness(id))?;
                                let condition = self.render_condition(
                                    control.condition,
                                    *condition_nonzero,
                                    &mut pay,
                                )?;
                                todo.push(Task::Text(id, "}".to_owned(), depth + 1));
                                todo.push(Task::Text(
                                    id,
                                    format!("break 'loop{};", id.0),
                                    depth + 2,
                                ));
                                if let Some(edge) = exit_edge {
                                    todo.push(Task::Transfer(*edge, depth + 2));
                                }
                                todo.push(Task::Text(
                                    id,
                                    format!("if ({condition}) {{"),
                                    depth + 1,
                                ));
                            }
                            todo.push(Task::Node(header.id, depth + 1, Some(id)));
                        }
                        K::Break | K::Continue => {
                            let loop_id = loop_id.ok_or(E::InvalidSharedSource(id))?;
                            output.emit(
                                id,
                                format!(
                                    "{} 'loop{};",
                                    if matches!(node.kind, K::Break) {
                                        "break"
                                    } else {
                                        "continue"
                                    },
                                    loop_id.0
                                ),
                                depth,
                                false,
                                &mut pay,
                            )?;
                        }
                        K::Exit { block } => {
                            let b = source.blocks[block];
                            match &b.terminator {
                                T::Trap { reason } => output.emit(
                                    self.block(*block)
                                        .and_then(|b| b.statements.last())
                                        .map_or(id, |s| s.id),
                                    crate::render_explicit_invalid(b)
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| format!("trap({reason:?});")),
                                    depth,
                                    false,
                                    &mut pay,
                                )?,
                                T::Exit {
                                    kind: crate::ExitKind::EndOfCode,
                                    ..
                                } => {
                                    let text = self.render_exit(
                                        crate::ExitKind::EndOfCode,
                                        &[],
                                        &mut pay,
                                    )?;
                                    output.emit(id, text, depth, false, &mut pay)?
                                }
                                T::Exit { .. } => {}
                                _ => return Err(E::InvalidSharedSource(id)),
                            }
                        }
                        _ => return Err(E::InvalidSharedSource(id)),
                    }
                }
            }
        }
        if !p.interface.outputs.is_empty() {
            output.emit(
                p.body,
                format!("yield ({});", self.binding_tuple(&p.interface.outputs)),
                0,
                false,
                &mut pay,
            )?;
        }
        Ok(output.rows)
    }
}
