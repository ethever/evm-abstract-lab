//! Source-checked, local RHS views for retained pure word bindings.
//! A view changes no statement record, opcode, evaluation position or output.
//! Unsupported correspondence (including foreign aliases and predicate DAGs)
//! retains all primitive operands instead of guessing a definition.
use super::{
    LiftedRenderError as Error, LiftedRenderIndex, MAX_LIFTED_EXPRESSION_TEXT, primitive::append,
};
use crate::{
    Block, EntityId, Ir, LiftedExpressionKind as Kind, LiftedProgram, LiftedStatement, LiftedType,
    Operation, Terminator, Value, ValueKind, WordExpr, WordRewrite, WordSimplifyError,
    simplify_word,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
struct SourceOp<'a> {
    block: &'a Block,
    position: usize,
    operation: &'a Operation,
}
#[derive(Clone, Copy)]
struct Definition {
    block: EntityId,
    position: Option<usize>,
    operation: Option<EntityId>,
    parameter: Option<usize>,
}

/// Linear source indexes are built once under the render index's reservation.
#[derive(Default)]
pub(super) struct WordSources<'a> {
    operations: BTreeMap<EntityId, SourceOp<'a>>,
    definitions: BTreeMap<EntityId, Definition>,
    values: BTreeMap<EntityId, &'a Value>,
    statements: BTreeMap<EntityId, (EntityId, &'a LiftedStatement)>,
    expression_positions: BTreeMap<EntityId, usize>,
    bound: BTreeSet<EntityId>,
    valid: bool,
}
impl<'a> WordSources<'a> {
    pub(super) fn new(program: &'a LiftedProgram, ir: Option<&'a Ir>) -> Self {
        let Some(ir) = ir else {
            return Self::default();
        };
        let mut source = Self {
            valid: true,
            ..Self::default()
        };
        for value in &ir.values {
            if source.values.insert(value.id, value).is_some() {
                source.valid = false;
            }
        }
        for (position, expression) in program.expressions.iter().enumerate() {
            if source
                .expression_positions
                .insert(expression.id, position)
                .is_some()
            {
                source.valid = false;
            }
        }
        let mut blocks = BTreeSet::new();
        for block in &ir.blocks {
            if !blocks.insert(block.id) {
                source.valid = false;
            }
            for (index, value) in block.parameters.iter().enumerate() {
                if source
                    .definitions
                    .insert(
                        *value,
                        Definition {
                            block: block.id,
                            position: None,
                            operation: None,
                            parameter: Some(index),
                        },
                    )
                    .is_some()
                {
                    source.valid = false;
                }
            }
            for (position, operation) in block.operations.iter().enumerate() {
                if source
                    .operations
                    .insert(
                        operation.id,
                        SourceOp {
                            block,
                            position,
                            operation,
                        },
                    )
                    .is_some()
                {
                    source.valid = false;
                }
                for value in &operation.outputs {
                    if source
                        .definitions
                        .insert(
                            *value,
                            Definition {
                                block: block.id,
                                position: Some(position),
                                operation: Some(operation.id),
                                parameter: None,
                            },
                        )
                        .is_some()
                    {
                        source.valid = false;
                    }
                }
            }
        }
        let mut high_blocks = BTreeSet::new();
        for block in &program.blocks {
            if !blocks.contains(&block.block) || !high_blocks.insert(block.block) {
                source.valid = false;
            }
            for parameter in &block.parameters {
                if source
                    .definitions
                    .get(parameter)
                    .is_some_and(|d| d.block == block.block && d.parameter.is_some())
                {
                    source.bound.insert(*parameter);
                }
            }
            let mut previous = None;
            for statement in &block.statements {
                if source
                    .statements
                    .insert(statement.id, (block.block, statement))
                    .is_some()
                {
                    source.valid = false;
                }
                let Some(raw) = source.operations.get(&statement.operation) else {
                    source.valid = false;
                    continue;
                };
                if raw.block.id != block.block
                    || previous.is_some_and(|position| position >= raw.position)
                    || statement.inputs.len() != raw.operation.inputs.len()
                {
                    source.valid = false;
                }
                previous = Some(raw.position);
                let mut outputs = raw.operation.outputs.iter();
                for output in &statement.outputs {
                    if outputs.by_ref().any(|raw| raw == output) {
                        source.bound.insert(*output);
                    } else {
                        source.valid = false;
                    }
                }
            }
        }
        source
    }
    fn available(&self, value: EntityId, block: EntityId, before: usize) -> bool {
        let Some(definition) = self.definitions.get(&value) else {
            return false;
        };
        if definition.block != block
            || definition
                .position
                .is_some_and(|position| position >= before)
        {
            return false;
        }
        match self.values.get(&value).map(|v| &v.kind) {
            Some(ValueKind::Constant(word)) => definition
                .operation
                .and_then(|id| self.operations.get(&id))
                .is_some_and(|producer| {
                    let op = producer.operation;
                    let literal_source = match op.opcode {
                        0x5f => op.inputs.is_empty() && *word == crate::U256::ZERO,
                        0x60..=0x7f => op.inputs.is_empty(),
                        _ => {
                            word_operation(*producer)
                                && op.replacement == Some(*word)
                                && op.inputs.iter().all(|input| {
                                    self.definitions.get(input).is_some_and(|d| {
                                        d.block == block
                                            && d.position
                                                .is_none_or(|position| position < producer.position)
                                    })
                                })
                        }
                    };
                    literal_source
                        && op.effects.is_pure()
                        && op.outputs.as_slice() == [value]
                        && [
                            op.memory_input,
                            op.memory_output,
                            op.effect_input,
                            op.effect_output,
                        ]
                        .iter()
                        .all(Option::is_none)
                        && !matches!(producer.block.terminator, Terminator::Pending)
                        && !(producer.position + 1 == producer.block.operations.len()
                            && matches!(producer.block.terminator, Terminator::Trap { .. }))
                }),
            Some(ValueKind::Operation { operation, index }) => {
                definition.operation == Some(*operation)
                    && self
                        .operations
                        .get(operation)
                        .is_some_and(|o| o.operation.outputs.get(*index) == Some(&value))
            }
            Some(ValueKind::Parameter {
                block: owner,
                index,
            }) => *owner == block && definition.parameter == Some(*index),
            _ => false,
        }
    }
    fn producer(&self, value: EntityId, block: EntityId, before: usize) -> Option<SourceOp<'a>> {
        if !self.available(value, block, before) {
            return None;
        }
        let operation = self.definitions.get(&value)?.operation?;
        let producer = *self.operations.get(&operation)?;
        word_operation(producer).then_some(producer)
    }
    fn raw_view(&self, value: EntityId, block: EntityId, before: usize) -> Option<WordExpr> {
        if !self.available(value, block, before) {
            return None;
        }
        if let ValueKind::Constant(word) = self.values.get(&value)?.kind {
            return Some(WordExpr::Literal(word));
        }
        if let Some(producer) = self.producer(value, block, before)
            && producer.operation.opcode == 0x16
        {
            if !producer
                .operation
                .inputs
                .iter()
                .all(|input| self.available(*input, block, producer.position))
            {
                return None;
            }
            return Some(WordExpr::And {
                left: producer.operation.inputs[0],
                right: producer.operation.inputs[1],
            });
        }
        Some(WordExpr::Opaque)
    }
}
fn word_operation(source: SourceOp<'_>) -> bool {
    let op = source.operation;
    op.effects.is_pure()
        && op.outputs.len() == 1
        && !matches!(op.opcode, 0x10..=0x15)
        && evm_bytecode::word::arity(op.opcode) == Some(op.inputs.len())
        && [
            op.memory_input,
            op.memory_output,
            op.effect_input,
            op.effect_output,
        ]
        .iter()
        .all(Option::is_none)
        && !matches!(source.block.terminator, Terminator::Pending)
        && !(source.position + 1 == source.block.operations.len()
            && matches!(source.block.terminator, Terminator::Trap { .. }))
}
fn paid(pay: &mut impl FnMut(usize) -> bool, amount: usize) -> Result<(), Error> {
    if pay(amount) {
        Ok(())
    } else {
        Err(Error::Limit)
    }
}

impl LiftedRenderIndex<'_> {
    /// Exact local word recipe actually used by retained statement rendering.
    /// None preserves the original primitive operands. Source correspondence is
    /// deliberately conservative: no foreign alias, predicate reconstruction,
    /// approximate value fact, or stateful producer is used to remove an input.
    pub fn statement_word_rhs(
        &self,
        statement: &LiftedStatement,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<Option<WordRewrite>, Error> {
        paid(&mut pay, 1)?;
        if statement.outputs.len() != 1 || statement.inputs.len() > 3 {
            return Ok(None);
        }
        let source = &self.word_sources;
        if !source.valid {
            return Ok(None);
        }
        let Some(&(block, registered)) = source.statements.get(&statement.id) else {
            return Ok(None);
        };
        let Some(&raw) = source.operations.get(&statement.operation) else {
            return Ok(None);
        };
        if raw.block.id != block
            || !word_operation(raw)
            || statement.outputs != raw.operation.outputs
            || statement.inputs.len() != raw.operation.inputs.len()
        {
            return Ok(None);
        }
        if registered != statement {
            return Ok(None);
        }
        // Reject non-candidates before walking their DAGs. This prevents a
        // retained FullMath MUL/DIV from replaying a large input merely to keep
        // the same primitive operands. A candidate still proves every input below,
        // including the one its algebraic recipe would otherwise eliminate.
        let recipe = match simplify_word(
            raw.operation.opcode,
            &statement.inputs,
            |id| {
                let expression = self.expressions.get(&id)?;
                if expression.block != block
                    || expression.ty != LiftedType::Word256
                    || expression.kind.ty() != LiftedType::Word256
                {
                    return None;
                }
                Some(match &expression.kind {
                    Kind::Word(word) => WordExpr::Literal(*word),
                    Kind::WordOp {
                        opcode: 0x16,
                        arguments,
                    } if arguments.len() == 2 => WordExpr::And {
                        left: arguments[0],
                        right: arguments[1],
                    },
                    _ => WordExpr::Opaque,
                })
            },
            &mut pay,
        ) {
            Ok(recipe) => recipe,
            Err(WordSimplifyError::Limit) => return Err(Error::Limit),
            Err(_) => return Ok(None),
        };
        if recipe == WordRewrite::Unchanged {
            return Ok(None);
        }
        for (expression, value) in statement.inputs.iter().zip(&raw.operation.inputs) {
            if !self.corresponds(*expression, *value, block, raw.position, &mut pay)? {
                return Ok(None);
            }
        }
        Ok(Some(recipe))
    }
    fn corresponds(
        &self,
        expression: EntityId,
        value: EntityId,
        block: EntityId,
        before: usize,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<bool, Error> {
        let source = &self.word_sources;
        let mut pending = vec![(expression, value, before)];
        let mut seen = BTreeSet::new();
        while let Some((id, value, before)) = pending.pop() {
            paid(pay, 1)?;
            if !seen.insert((id, value, before)) {
                continue;
            }
            if seen.len() > MAX_LIFTED_EXPRESSION_TEXT {
                return Err(Error::Limit);
            }
            let Some(expression) = self.expressions.get(&id) else {
                return Ok(false);
            };
            if expression.block != block
                || expression.ty != LiftedType::Word256
                || expression.kind.ty() != LiftedType::Word256
                || !source.available(value, block, before)
            {
                return Ok(false);
            }
            if let Kind::Value(leaf) = expression.kind
                && leaf == value
                && source.bound.contains(&leaf)
            {
                continue;
            }
            if let Kind::Word(word) = expression.kind
                && matches!(source.values.get(&value).map(|v|&v.kind),Some(ValueKind::Constant(actual)) if *actual==word)
            {
                continue;
            }
            let Some(producer) = source.producer(value, block, before) else {
                return Ok(false);
            };
            let recipe = match simplify_word(
                producer.operation.opcode,
                &producer.operation.inputs,
                |raw| source.raw_view(raw, block, producer.position),
                &mut *pay,
            ) {
                Ok(recipe) => recipe,
                Err(WordSimplifyError::Limit) => return Err(Error::Limit),
                Err(_) => return Ok(false),
            };
            match recipe {
                WordRewrite::Existing(value) => pending.push((id, value, producer.position)),
                WordRewrite::Literal(word) => {
                    if expression.kind != Kind::Word(word) {
                        return Ok(false);
                    }
                }
                WordRewrite::Masked { value, mask } => {
                    let Kind::WordOp {
                        opcode: 0x16,
                        arguments,
                    } = &expression.kind
                    else {
                        return Ok(false);
                    };
                    if arguments.len() != 2 {
                        return Ok(false);
                    }
                    if !arguments.iter().all(|child| {
                        source
                            .expression_positions
                            .get(child)
                            .zip(source.expression_positions.get(&id))
                            .is_some_and(|(child, parent)| child < parent)
                    }) {
                        return Ok(false);
                    }
                    let pair = arguments.iter().enumerate().find_map(|(index, child)| {
                        let child = self.expressions.get(child)?;
                        (child.block == block
                            && child.ty == LiftedType::Word256
                            && child.kind == Kind::Word(mask))
                        .then_some(arguments[1 - index])
                    });
                    let Some(child) = pair else {
                        return Ok(false);
                    };
                    if !source
                        .expression_positions
                        .get(&child)
                        .zip(source.expression_positions.get(&id))
                        .is_some_and(|(child, parent)| child < parent)
                    {
                        return Ok(false);
                    }
                    pending.push((child, value, producer.position));
                }
                WordRewrite::Unchanged => {
                    let Kind::WordOp { opcode, arguments } = &expression.kind else {
                        return Ok(false);
                    };
                    if *opcode != producer.operation.opcode
                        || arguments.len() != producer.operation.inputs.len()
                    {
                        return Ok(false);
                    }
                    for (child, raw) in arguments.iter().zip(&producer.operation.inputs) {
                        if !source
                            .expression_positions
                            .get(child)
                            .zip(source.expression_positions.get(&id))
                            .is_some_and(|(child, parent)| child < parent)
                        {
                            return Ok(false);
                        }
                        pending.push((*child, *raw, producer.position));
                    }
                }
            }
        }
        Ok(true)
    }
    pub(super) fn render_word_rhs(
        &self,
        recipe: WordRewrite,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<String, Error> {
        match recipe {
            WordRewrite::Existing(id) => self.render_expression(id, pay),
            WordRewrite::Literal(word) => {
                let mut out = String::new();
                append(&mut out, &format!("0x{word:x}"), pay)?;
                Ok(out)
            }
            WordRewrite::Masked { value, mask } => {
                let value = self.render_expression(value, &mut *pay)?;
                let mut out = String::new();
                append(&mut out, "(", pay)?;
                append(&mut out, &value, pay)?;
                append(&mut out, &format!(" & 0x{mask:x})"), pay)?;
                Ok(out)
            }
            WordRewrite::Unchanged => Err(Error::MissingWitness(EntityId(0))),
        }
    }
}
