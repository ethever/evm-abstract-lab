//! Format admitted data in its own source scope. A guard may use only bindings
//! already available at its decision, including its exact failed-edge tuple.
use super::{
    LiftedExpressionKind as X, LiftedRenderError as E, LiftedRenderIndex, primitive::append,
};
use crate::{
    EntityId, LiftedByteSegment as S, LiftedByteSource as Source, LiftedControl,
    LiftedDataWord as W, LiftedRevertData as Data, LiftedRevertDataKind as Kind,
    LiftedRevertFormat as F,
};
#[cfg(test)]
mod tests;

impl<'a> LiftedRenderIndex<'a> {
    /// Source-admitted occurrence data, quarantined with invalid overlays.
    pub fn revert_data(&self, id: EntityId) -> Option<&'a Data> {
        self.guard_source_error
            .is_none()
            .then(|| self.revert_data.get(&id).copied())
            .flatten()
    }
    /// Source-admitted data for one original terminal block.
    pub fn revert_data_for_block(&self, block: EntityId) -> Option<&'a Data> {
        self.guard_source_error
            .is_none()
            .then(|| self.data_reverts.get(&block).copied())
            .flatten()
    }
    fn data_expression(
        &self,
        data: &Data,
        value: EntityId,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<EntityId, E> {
        if !pay(data.writes.len().saturating_mul(8).saturating_add(1)) {
            return Err(E::Limit);
        }
        for write in &data.writes {
            let Some(op) = self.operations.get(write) else {
                continue;
            };
            let Some(statement) = self.source_statements.get(write) else {
                continue;
            };
            for (position, raw) in op.inputs.iter().enumerate() {
                if *raw == value
                    && let Some(expression) = statement.inputs.get(position)
                {
                    let Some(expr) = self.expression(*expression) else {
                        continue;
                    };
                    // An opaque source identity is exact even when copy
                    // projection placed its leaf in the consumer's scope.
                    if let X::Value(leaf) = expr.kind
                        && self.word_sources.copy_identity(value, leaf, pay)?
                    {
                        return Ok(*expression);
                    }
                    let Some((block, before)) = self.word_sources.source_position(*write) else {
                        continue;
                    };
                    if self.corresponds(*expression, value, block, before, pay)? {
                        return Ok(*expression);
                    }
                }
            }
        }
        Err(E::MissingWitness(value))
    }
    fn guard_expression(
        &self,
        expression: EntityId,
        control: &LiftedControl,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<Option<EntityId>, E> {
        let Some(guard) = control.guard.as_ref() else {
            return Ok(None);
        };
        let Some(expr) = self.expression(expression) else {
            return Ok(None);
        };
        if self.available_data_expression(expression, guard.block, pay)? {
            return Ok(Some(expression));
        }
        let X::Value(value) = expr.kind else {
            return Ok(None);
        };
        let Some(edge) = self.edges.get(&guard.failure_edge) else {
            return Ok(None);
        };
        if edge.from != guard.block || edge.to != expr.block {
            return Ok(None);
        }
        let Some(body) = self.block(expr.block) else {
            return Ok(None);
        };
        let Some(index) = body.parameters.iter().position(|param| *param == value) else {
            return Ok(None);
        };
        let Some(bound) = self
            .transfer(guard.failure_edge)
            .and_then(|t| t.arguments.get(index))
            .copied()
        else {
            return Ok(None);
        };
        let Some(raw_slot) = self.word_sources.parameter_slot(value, expr.block) else {
            return Ok(None);
        };
        let Some(raw_value) = edge.arguments.get(raw_slot).copied() else {
            return Ok(None);
        };
        let Some(before) = self.word_sources.decision_position(guard.block) else {
            return Ok(None);
        };
        let correspondence = if let Some(X::Value(leaf)) = self.expression(bound).map(|e| &e.kind) {
            self.word_sources.copy_identity(raw_value, *leaf, pay)?
        } else {
            self.corresponds(bound, raw_value, guard.block, before, pay)?
        };
        if !correspondence {
            return Ok(None);
        }
        Ok(self
            .available_data_expression(bound, guard.block, pay)?
            .then_some(bound))
    }
    fn available_data_expression(
        &self,
        expression: EntityId,
        block: EntityId,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<bool, E> {
        if !pay(32) {
            return Err(E::Limit);
        }
        let mut pending = vec![expression];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !pay(32) || seen.len() >= crate::MAX_LIFTED_RECORDS {
                return Err(E::Limit);
            }
            if !seen.insert(id) {
                continue;
            }
            let Some(expr) = self.expression(id) else {
                return Ok(false);
            };
            if let X::Value(value) = expr.kind {
                if !self.word_sources.bound_at(value, block) {
                    return Ok(false);
                }
            } else {
                pending.extend(expr.kind.operands());
            }
        }
        Ok(true)
    }
    fn data_word(
        &self,
        data: &Data,
        word: W,
        guard: Option<&LiftedControl>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<String, E> {
        if !pay(80) {
            return Err(E::Limit);
        }
        match word {
            W::Constant(value) => Ok(format!("0x{value:x}")),
            W::SourceValue(value) => {
                let expr = self.data_expression(data, value, pay)?;
                let expr = if let Some(guard) = guard {
                    self.guard_expression(expr, guard, pay)?
                        .ok_or(E::MissingWitness(value))?
                } else {
                    expr
                };
                self.render_expression(expr, pay)
            }
        }
    }
    pub(super) fn guard_data_bindable(
        &self,
        control: &LiftedControl,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<bool, E> {
        let Some(crate::LiftedGuardPayload::Data(id)) = control.guard.as_ref().map(|g| g.payload)
        else {
            return Ok(true);
        };
        let data = self.revert_data(id).ok_or(E::MissingWitness(id))?;
        let words = match data.decode_format(&mut *pay)? {
            F::Empty | F::ErrorString(_) => return Ok(true),
            F::Panic(word) => vec![word],
            F::SelectorWords { words, .. } => words,
            F::Forwarded {
                source: Source::ReturnData(call),
                offset,
                size,
            } => {
                let Some(guard) = control.guard.as_ref() else {
                    return Ok(false);
                };
                if !self.constructions.get(&data.block).is_some_and(|p| {
                    p.revert == data.id && p.kind == crate::LiftedConstructionKind::PreparedConsumer
                }) || !self
                    .call_results
                    .as_ref()
                    .is_some_and(|p| p.data_source(data, call))
                    || self
                        .returndata_name(call, guard.block, &mut *pay)?
                        .is_none()
                {
                    return Ok(false);
                }
                let plan = self.constructions[&data.block];
                let body = self
                    .block(data.block)
                    .ok_or(E::MissingWitness(data.block))?;
                if !pay(plan.end.saturating_sub(plan.start).saturating_mul(8)) {
                    return Err(E::Limit);
                }
                for statement in &body.statements[plan.start..plan.end] {
                    if self
                        .operations
                        .get(&statement.operation)
                        .is_some_and(|op| op.opcode == 0x3d)
                        && self.returndata_source(statement.operation) != Some(call)
                    {
                        return Ok(false);
                    }
                }
                vec![offset, size]
            }
            _ => return Ok(false),
        };
        for word in words {
            if let W::SourceValue(value) = word {
                let expr = self.data_expression(data, value, pay)?;
                if self.guard_expression(expr, control, pay)?.is_none() {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
    /// Format only the data recipe in its actual failed source scope.
    pub fn render_revert_data(
        &self,
        id: EntityId,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<String, E> {
        self.render_data(id, None, &mut pay)
    }
    pub(super) fn render_guard_data(
        &self,
        id: EntityId,
        guard: &LiftedControl,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<String, E> {
        self.render_data(id, Some(guard), pay)
    }
    fn render_data(
        &self,
        id: EntityId,
        guard: Option<&LiftedControl>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<String, E> {
        let mut remaining = crate::MAX_LIFTED_EXPRESSION_TEXT;
        let mut bounded = |amount| {
            if amount > remaining || !pay(amount) {
                false
            } else {
                remaining -= amount;
                true
            }
        };
        if !bounded(256) {
            return Err(E::Limit);
        }
        self.render_data_inner(id, guard, &mut bounded)
    }
    fn render_data_inner(
        &self,
        id: EntityId,
        guard: Option<&LiftedControl>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<String, E> {
        self.validate_guard_sources()?;
        let data = self.revert_data(id).ok_or(E::MissingWitness(id))?;
        let format = data.decode_format(&mut *pay)?;
        let text = match format {
            F::Empty => "Revert(bytes = 0x)".to_owned(),
            F::ErrorString(message) => {
                let mut text = String::from("Error(");
                super::quoted(&mut text, &message, &mut *pay)?;
                text.push(')');
                text
            }
            F::Panic(word) => format!("Panic({})", self.data_word(data, word, guard, pay)?),
            F::SelectorWords { selector, words } => {
                let mut text = format!(
                    "Revert(selector = 0x{:02x}{:02x}{:02x}{:02x}, words = [",
                    selector[0], selector[1], selector[2], selector[3]
                );
                for (i, word) in words.into_iter().enumerate() {
                    if i > 0 {
                        append(&mut text, ", ", &mut *pay)?;
                    }
                    let value = self.data_word(data, word, guard, pay)?;
                    append(&mut text, &value, &mut *pay)?;
                }
                text.push_str("])");
                text
            }
            F::Forwarded {
                source,
                offset,
                size,
            } => format!(
                "Revert(data = {})",
                self.data_slice(data, source, offset, size, guard, pay)?
            ),
            F::Raw => match &data.content {
                Kind::Raw => {
                    let body = self
                        .block(data.block)
                        .ok_or(E::MissingWitness(data.block))?;
                    let offset = *body
                        .exit_values
                        .first()
                        .ok_or(E::MissingWitness(data.offset))?;
                    let size = *body
                        .exit_values
                        .get(1)
                        .ok_or(E::MissingWitness(data.size))?;
                    format!(
                        "Revert(data = memory_slice(m{}, {}, {}))",
                        data.memory.0,
                        self.render_expression(offset, &mut *pay)?,
                        self.render_expression(size, &mut *pay)?
                    )
                }
                Kind::Segments(parts) => {
                    let mut text = String::from("Revert(data = concat(");
                    for (i, part) in parts.iter().enumerate() {
                        if i > 0 {
                            append(&mut text, ", ", &mut *pay)?;
                        }
                        let value = match part {
                            S::Bytes(bytes) => {
                                if !pay(bytes.len().saturating_mul(2).saturating_add(2)) {
                                    return Err(E::Limit);
                                }
                                let mut value = String::from("0x");
                                for byte in bytes {
                                    use std::fmt::Write;
                                    let _ = write!(value, "{byte:02x}");
                                }
                                value
                            }
                            S::Word {
                                value,
                                offset,
                                size,
                            } => {
                                if !pay(64) {
                                    return Err(E::Limit);
                                }
                                format!(
                                    "word_bytes({}, {}, {})",
                                    self.data_word(data, *value, guard, pay)?,
                                    offset,
                                    size
                                )
                            }
                            S::Slice {
                                source,
                                offset,
                                size,
                            } => self.data_slice(data, *source, *offset, *size, guard, pay)?,
                        };
                        append(&mut text, &value, &mut *pay)?;
                    }
                    text.push_str("))");
                    text
                }
            },
        };
        if !pay(text.len()) {
            return Err(E::Limit);
        }
        Ok(text)
    }
    fn data_slice(
        &self,
        data: &Data,
        source: Source,
        offset: W,
        size: W,
        guard: Option<&LiftedControl>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<String, E> {
        let source = match source {
            Source::Calldata => "calldata".to_owned(),
            Source::ReturnData(call) => {
                if !self
                    .call_results
                    .as_ref()
                    .is_some_and(|p| p.data_source(data, call))
                {
                    return Err(E::MissingWitness(call));
                }
                let block = guard
                    .and_then(|c| c.guard.as_ref())
                    .map_or(data.block, |g| g.block);
                let name = self
                    .returndata_name(call, block, &mut *pay)?
                    .ok_or(E::MissingWitness(call))?;
                let whole = if offset == W::Constant(crate::U256::ZERO) {
                    match size {
                        W::SourceValue(value) => self.whole_buffer_size(call, value, pay)?,
                        _ => false,
                    }
                } else {
                    false
                };
                if whole {
                    return Ok(name);
                }
                name
            }
        };
        Ok(format!(
            "slice({source}, {}, {})",
            self.data_word(data, offset, guard, pay)?,
            self.data_word(data, size, guard, pay)?
        ))
    }
    fn whole_buffer_size(
        &self,
        call: EntityId,
        value: EntityId,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<bool, E> {
        let Some(plan) = &self.call_results else {
            return Ok(false);
        };
        if plan.word_source(value) == Some(call) {
            return Ok(true);
        }
        let Some(binding) = self.call_result(call) else {
            return Ok(false);
        };
        if !pay(binding.readers.len().saturating_mul(8)) {
            return Err(E::Limit);
        }
        for reader in &binding.readers {
            if let Some(op) = self.operations.get(reader).filter(|o| o.opcode == 0x3d)
                && let Some(raw) = op.outputs.first()
                && self.word_sources.copy_identity(value, *raw, pay)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    /// New occurrence data only. Existing string certificates keep their
    /// standalone primitive spelling; only their selected require absorbs it.
    /// Unrecognized formats also retain the original terminal; their recovered
    /// segments remain available through the separate data/Inspector formatter.
    pub fn render_data_revert(
        &self,
        block: EntityId,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<Option<String>, E> {
        if self.revert(block).is_some() {
            return Ok(None);
        }
        let Some(data) = self.revert_data_for_block(block) else {
            return Ok(None);
        };
        if matches!(&data.content, Kind::Raw)
            || matches!(&data.content, Kind::Segments(parts) if parts.is_empty())
        {
            return Ok(None);
        }
        let format = data.decode_format(&mut pay)?;
        if matches!(format, F::Raw) {
            return Ok(None);
        }
        let payload = match self.render_revert_data(data.id, &mut pay) {
            Ok(payload) => payload,
            Err(E::MissingWitness(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut text = format!("evm.revert({payload});");
        if let Some(note) = format.panic_explanation() {
            append(&mut text, " // ", &mut pay)?;
            append(&mut text, note, &mut pay)?;
        }
        if !pay(text.len()) {
            return Err(E::Limit);
        }
        Ok(Some(text))
    }
}
