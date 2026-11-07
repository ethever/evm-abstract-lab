//! Parameterized canonical body rows shared by native and browser consumers.
use super::{MAX_SHARED_REGION_ENTITIES, SharedInvocation, SharedProcedure};
use crate::EntityId;
use crate::{
    LiftedRenderError as E, LiftedRenderIndex, StructuredKind as K, Terminator as T, ValueKind as V,
};

/// One complete semantic row with its original source-navigation entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedRenderRow {
    /// Source operation or syntax node; the raw body remains inspectable.
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
fn tuple(ids: &[EntityId], prefix: &str) -> String {
    ids.iter()
        .map(|id| format!("{prefix}{}", id.0))
        .collect::<Vec<_>>()
        .join(", ")
}
impl<'a> LiftedRenderIndex<'a> {
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
        if self.shared_error.is_some() || self.shared_source.is_none() {
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
        let mut text = format!(
            "procedure shared_{}({}) -> ({}) {{",
            p.id.0,
            tuple(&p.interface.inputs, "v"),
            tuple(&p.interface.outputs, "v")
        );
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
        let args = p
            .interface
            .inputs
            .iter()
            .map(|id| {
                let id = actual(*id)?;
                Ok(match source.values.get(&id).map(|v| &v.kind) {
                    Some(V::Constant(w)) => format!("0x{w:x}"),
                    _ => format!("v{}", id.0),
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
            text.push_str(&format!("({}) = ", tuple(&outputs, "v")));
        }
        text.push_str(&format!("shared_{}({args});", p.id.0));
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
        let source = self
            .shared_source
            .as_ref()
            .ok_or(E::InvalidSharedSource(p.id))?;
        enum Task {
            Node(EntityId, usize, Option<EntityId>),
            Text(EntityId, String, usize),
            Transfer(EntityId, usize),
        }
        let mut todo = vec![Task::Node(p.body, 0, None)];
        let mut rows = Vec::new();
        let mut text_bytes = 0_usize;
        let mut emit = |entity, text: String, depth, annotation, pay: &mut _| -> Result<(), E> {
            paid(pay, text.len().saturating_add(1))?;
            text_bytes = text_bytes.saturating_add(text.len());
            if text_bytes > 4_194_304 {
                return Err(E::Limit);
            }
            if rows.len() >= MAX_SHARED_REGION_ENTITIES {
                return Err(E::Limit);
            }
            rows.push(SharedRenderRow {
                entity,
                text,
                depth,
                is_structure_annotation: annotation,
            });
            Ok(())
        };
        while let Some(task) = todo.pop() {
            paid(&mut pay, 1)?;
            match task {
                Task::Text(id, text, depth) => emit(id, text, depth, false, &mut pay)?,
                Task::Transfer(id, depth) => {
                    let transfer = self.transfer(id).ok_or(E::MissingWitness(id))?;
                    if let Some(text) = self.render_transfer(transfer, &mut pay)? {
                        emit(id, text, depth, false, &mut pay)?;
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
                            emit(id, text, depth, false, &mut pay)?;
                        } else {
                            paid(&mut pay, text.len().saturating_add(1))?;
                        }
                    }
                }
                Task::Node(id, depth, loop_id) => {
                    let node = source.nodes.get(&id).ok_or(E::MissingWitness(id))?;
                    match &node.kind {
                        K::Sequence(children) => {
                            let mut pos = 0;
                            let mut selected = Vec::new();
                            while let Some(n) = children.get(pos) {
                                paid(&mut pay, 1)?;
                                if let Some(end) = self.sequence_end(id, pos) {
                                    pos = end;
                                    continue;
                                }
                                selected.push(n.id);
                                pos += 1;
                            }
                            todo.extend(
                                selected
                                    .into_iter()
                                    .rev()
                                    .map(|id| Task::Node(id, depth, loop_id)),
                            );
                        }
                        K::Block { block } => {
                            let b = source.blocks.get(block).ok_or(E::MissingWitness(*block))?;
                            emit(id, format!("// block b{}", block.0), depth, true, &mut pay)?;
                            let high = self.block(*block).ok_or(E::MissingWitness(*block))?;
                            let construction = self.construction(*block);
                            let mut compact = false;
                            for (position, statement) in high.statements.iter().enumerate() {
                                paid(&mut pay, 1)?;
                                if let Some(c) = construction {
                                    if position == c.start
                                        && let Some(text) = self.render_revert(*block, &mut pay)?
                                    {
                                        emit(statement.operation, text, depth, false, &mut pay)?;
                                        compact = true;
                                    }
                                    if compact && (c.start..c.end).contains(&position) {
                                        continue;
                                    }
                                }
                                if matches!(b.terminator, T::Trap { .. })
                                    && b.operations
                                        .last()
                                        .is_some_and(|o| o.id == statement.operation)
                                {
                                    continue;
                                }
                                let text = self.render_statement_with_dependencies(
                                    statement, state, &mut pay,
                                )?;
                                emit(statement.operation, text, depth, false, &mut pay)?;
                            }
                        }
                        K::Transfer { edge } => todo.push(Task::Transfer(*edge, depth)),
                        K::If {
                            then_branch,
                            else_branch,
                            ..
                        } => {
                            let control = self.control(id).ok_or(E::MissingWitness(id))?;
                            if let Some(g) = self.guard(id) {
                                let text = self
                                    .render_guard(control, &mut pay)?
                                    .ok_or(E::MissingWitness(id))?;
                                emit(id, text, depth, false, &mut pay)?;
                                todo.push(Task::Node(g.pass_node, depth, loop_id));
                            } else {
                                emit(
                                    id,
                                    format!(
                                        "if ({}) {{",
                                        self.render_expression(control.condition, &mut pay)?
                                    ),
                                    depth,
                                    false,
                                    &mut pay,
                                )?;
                                todo.push(Task::Text(id, "}".to_owned(), depth));
                                todo.push(Task::Node(else_branch.id, depth + 1, loop_id));
                                todo.push(Task::Text(id, "} else {".to_owned(), depth));
                                todo.push(Task::Node(then_branch.id, depth + 1, loop_id));
                            }
                        }
                        K::While {
                            header,
                            condition,
                            condition_nonzero,
                            body,
                            exit_edge,
                        } => {
                            emit(
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
                                let condition =
                                    self.render_expression(control.condition, &mut pay)?;
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
                                    format!(
                                        "if ({}{}) {{",
                                        if *condition_nonzero { "!" } else { "" },
                                        format_args!("({condition})")
                                    ),
                                    depth + 1,
                                ));
                            }
                            todo.push(Task::Node(header.id, depth + 1, Some(id)));
                        }
                        K::Break | K::Continue => {
                            let loop_id = loop_id.ok_or(E::InvalidSharedSource(id))?;
                            emit(
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
                                T::Trap { reason } => {
                                    emit(id, format!("trap({reason:?});"), depth, false, &mut pay)?
                                }
                                T::Exit {
                                    kind: crate::ExitKind::EndOfCode,
                                    ..
                                } => emit(id, "stop();".to_owned(), depth, false, &mut pay)?,
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
            emit(
                p.body,
                format!("yield ({});", tuple(&p.interface.outputs, "v")),
                0,
                false,
                &mut pay,
            )?;
        }
        Ok(rows)
    }
}
