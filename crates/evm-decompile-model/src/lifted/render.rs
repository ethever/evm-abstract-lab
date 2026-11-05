//! Shared semantic formatting for lightweight consumers and CLI export.
use super::{
    LiftedBlock, LiftedControl, LiftedExpression, LiftedExpressionKind, LiftedProgram,
    LiftedRelation, LiftedStatement, LiftedTransfer, MAX_LIFTED_EXPRESSION_TEXT,
};
use crate::{EntityId, Ir};
use std::collections::BTreeMap;

mod primitive;
use primitive::append;

/// Reusable indexes for rendering many semantic rows without rescanning arenas.
/// Owners reserve index construction from their own work ledger before creating it.
pub struct LiftedRenderIndex<'a> {
    expressions: BTreeMap<EntityId, &'a LiftedExpression>,
    operations: BTreeMap<EntityId, &'a crate::Operation>,
    edges: BTreeMap<EntityId, &'a crate::Edge>,
    blocks: BTreeMap<EntityId, &'a LiftedBlock>,
    transfers: BTreeMap<EntityId, &'a LiftedTransfer>,
    controls: BTreeMap<EntityId, &'a LiftedControl>,
}
/// Bounded expression export failure, independent of artifact recovery coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiftedRenderError {
    /// Caller refused work or expanded text reached its fixed ceiling.
    Limit,
    /// A referenced semantic expression is absent.
    MissingExpression(EntityId),
    /// A source primitive or edge binding is absent.
    MissingWitness(EntityId),
}
impl std::fmt::Display for LiftedRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lifted rendering refused: {self:?}")
    }
}
impl std::error::Error for LiftedRenderError {}

impl<'a> LiftedRenderIndex<'a> {
    fn new(program: &'a LiftedProgram, ir: Option<&'a Ir>) -> Self {
        Self {
            expressions: program.expressions.iter().map(|e| (e.id, e)).collect(),
            operations: ir
                .into_iter()
                .flat_map(|ir| &ir.blocks)
                .flat_map(|b| &b.operations)
                .map(|o| (o.id, o))
                .collect(),
            edges: ir
                .into_iter()
                .flat_map(|ir| &ir.edges)
                .map(|e| (e.id, e))
                .collect(),
            blocks: program.blocks.iter().map(|b| (b.block, b)).collect(),
            transfers: program.transfers.iter().map(|t| (t.edge, t)).collect(),
            controls: program.controls.iter().map(|c| (c.node, c)).collect(),
        }
    }
    /// Semantic body for a source block without scanning the block arena.
    pub fn block(&self, source: EntityId) -> Option<&'a LiftedBlock> {
        self.blocks.get(&source).copied()
    }
    /// Semantic tuple for a source edge without scanning all transfers.
    pub fn transfer(&self, source: EntityId) -> Option<&'a LiftedTransfer> {
        self.transfers.get(&source).copied()
    }
    /// Semantic predicate binding for a source AST node.
    pub fn control(&self, source: EntityId) -> Option<&'a LiftedControl> {
        self.controls.get(&source).copied()
    }

    /// Render a typed expression with bounded, iterative DAG expansion.
    /// Unsigned word comparisons use ordinary relational symbols; signed
    /// comparisons use explicit s-prefixed primitives.
    pub fn render_expression(
        &self,
        id: EntityId,
        mut reserve: impl FnMut(usize) -> bool,
    ) -> Result<String, LiftedRenderError> {
        enum Part {
            Expression(EntityId),
            Text(String),
        }
        let mut pending = vec![Part::Expression(id)];
        let mut out = String::new();
        let mut work = 0_usize;
        while let Some(part) = pending.pop() {
            work += 1;
            if work > MAX_LIFTED_EXPRESSION_TEXT
                || out.len() > MAX_LIFTED_EXPRESSION_TEXT
                || !reserve(1)
            {
                return Err(LiftedRenderError::Limit);
            }
            let mut call = |name: &str, args: &[EntityId], names: Option<&[&str]>| {
                // Bound pending expansion before allocating per-argument parts.
                let parts = if names.is_some() { 3 } else { 2 };
                if work
                    .saturating_add(pending.len())
                    .saturating_add(args.len().saturating_mul(parts))
                    .saturating_add(2)
                    > MAX_LIFTED_EXPRESSION_TEXT
                {
                    return Err(LiftedRenderError::Limit);
                }
                pending.push(Part::Text(")".to_owned()));
                for (i, arg) in args.iter().enumerate().rev() {
                    pending.push(Part::Expression(*arg));
                    if let Some(names) = names {
                        pending.push(Part::Text(format!("{} = ", names[i])));
                    }
                    if i > 0 {
                        pending.push(Part::Text(", ".to_owned()));
                    }
                }
                pending.push(Part::Text(format!("{name}(")));
                Ok(())
            };
            match part {
                Part::Text(text) => append(&mut out, &text, &mut reserve)?,
                Part::Expression(next) => {
                    let e = self
                        .expressions
                        .get(&next)
                        .ok_or(LiftedRenderError::MissingExpression(next))?;
                    match &e.kind {
                        LiftedExpressionKind::Word(v) => {
                            append(&mut out, &format!("0x{v:x}"), &mut reserve)?;
                        }
                        LiftedExpressionKind::Predicate(v) => {
                            append(&mut out, if *v { "true" } else { "false" }, &mut reserve)?;
                        }
                        LiftedExpressionKind::Value(v) => {
                            append(&mut out, &format!("v{}", v.0), &mut reserve)?;
                        }
                        LiftedExpressionKind::WordFromPredicate(v) => call("word", &[*v], None)?,
                        LiftedExpressionKind::NonZero(v) => {
                            pending.push(Part::Text(" != 0)".to_owned()));
                            pending.push(Part::Expression(*v));
                            pending.push(Part::Text("(".to_owned()));
                        }
                        LiftedExpressionKind::Not(v) => {
                            pending.push(Part::Text(")".to_owned()));
                            pending.push(Part::Expression(*v));
                            pending.push(Part::Text("!(".to_owned()));
                        }
                        LiftedExpressionKind::Compare {
                            relation,
                            left,
                            right,
                        } => {
                            let symbol = match relation {
                                LiftedRelation::Eq => "==",
                                LiftedRelation::Ne => "!=",
                                LiftedRelation::Ult => "<",
                                LiftedRelation::Ule => "<=",
                                LiftedRelation::Ugt => ">",
                                LiftedRelation::Uge => ">=",
                                LiftedRelation::Slt => "slt",
                                LiftedRelation::Sle => "sle",
                                LiftedRelation::Sgt => "sgt",
                                LiftedRelation::Sge => "sge",
                            };
                            if matches!(
                                relation,
                                LiftedRelation::Slt
                                    | LiftedRelation::Sle
                                    | LiftedRelation::Sgt
                                    | LiftedRelation::Sge
                            ) {
                                call(symbol, &[*left, *right], Some(primitive::LEFT_RIGHT))?;
                            } else {
                                pending.push(Part::Text(")".to_owned()));
                                pending.push(Part::Expression(*right));
                                pending.push(Part::Text(format!(" {symbol} ")));
                                pending.push(Part::Expression(*left));
                                pending.push(Part::Text("(".to_owned()));
                            }
                        }
                        LiftedExpressionKind::WordOp { opcode, arguments } => {
                            let symbol = match opcode {
                                0x01 => Some("+"),
                                0x02 => Some("*"),
                                0x03 => Some("-"),
                                0x04 => Some("/"),
                                0x06 => Some("%"),
                                0x16 => Some("&"),
                                0x17 => Some("|"),
                                0x18 => Some("^"),
                                _ => None,
                            };
                            if let Some(symbol) = symbol.filter(|_| arguments.len() == 2) {
                                pending.push(Part::Text(")".to_owned()));
                                pending.push(Part::Expression(arguments[1]));
                                pending.push(Part::Text(format!(" {symbol} ")));
                                pending.push(Part::Expression(arguments[0]));
                                pending.push(Part::Text("(".to_owned()));
                            } else {
                                let name = match opcode {
                                    0x05 => "sdiv",
                                    0x07 => "smod",
                                    0x08 => "addmod",
                                    0x09 => "mulmod",
                                    0x0a => "exp",
                                    0x0b => "signextend",
                                    0x15 => "iszero",
                                    0x19 => "bitnot",
                                    0x1a => "byte",
                                    0x1b => "shl",
                                    0x1c => "shr",
                                    0x1d => "sar",
                                    0x1e => "clz",
                                    _ => "word_op",
                                };
                                call(
                                    name,
                                    arguments,
                                    primitive::operand_names(*opcode, arguments.len()),
                                )?;
                            }
                        }
                    }
                }
            }
        }
        if out.len() > MAX_LIFTED_EXPRESSION_TEXT {
            return Err(LiftedRenderError::Limit);
        }
        Ok(out)
    }
    /// One complete primitive call from semantic inputs, without a semicolon.
    /// Operand labels require an exact opcode/arity match; display spelling does
    /// not select a signature. Names, labels and separators share the expression
    /// byte ceiling and caller's work allowance. Refusal exposes no partial call.
    pub fn render_invocation(
        &self,
        opcode: u8,
        name: &str,
        arguments: &[EntityId],
        mut reserve: impl FnMut(usize) -> bool,
    ) -> Result<String, LiftedRenderError> {
        let names = primitive::operand_names(opcode, arguments.len());
        let mut out = String::new();
        append(&mut out, name, &mut reserve)?;
        append(&mut out, "(", &mut reserve)?;
        for (position, argument) in arguments.iter().enumerate() {
            if position > 0 {
                append(&mut out, ", ", &mut reserve)?;
            }
            if let Some(names) = names {
                append(&mut out, names[position], &mut reserve)?;
                append(&mut out, " = ", &mut reserve)?;
            }
            let argument = self.render_expression(*argument, &mut reserve)?;
            append(&mut out, &argument, &mut reserve)?;
        }
        append(&mut out, ")", &mut reserve)?;
        Ok(out)
    }
    /// One retained primitive, formatted from high semantic inputs.
    pub fn render_statement(
        &self,
        statement: &LiftedStatement,
        reserve: impl FnMut(usize) -> bool,
    ) -> Result<String, LiftedRenderError> {
        self.render_statement_with_dependencies(statement, true, reserve)
    }
    /// Format a retained primitive, optionally including state dependency tokens.
    /// This changes only the suffix; word inputs, outputs and effects are retained.
    pub fn render_statement_with_dependencies(
        &self,
        statement: &LiftedStatement,
        show_state_dependencies: bool,
        mut reserve: impl FnMut(usize) -> bool,
    ) -> Result<String, LiftedRenderError> {
        let operation = self
            .operations
            .get(&statement.operation)
            .copied()
            .ok_or(LiftedRenderError::MissingWitness(statement.operation))?;
        if operation.name.len() > MAX_LIFTED_EXPRESSION_TEXT || !reserve(operation.name.len()) {
            return Err(LiftedRenderError::Limit);
        }
        let name = operation.name.to_lowercase();
        let mut out = String::new();
        for (position, output) in statement.outputs.iter().enumerate() {
            if position > 0 {
                append(&mut out, ", ", &mut reserve)?;
            }
            append(&mut out, &format!("v{}", output.0), &mut reserve)?;
        }
        if !statement.outputs.is_empty() {
            append(&mut out, " = ", &mut reserve)?;
        }
        let invocation =
            self.render_invocation(operation.opcode, &name, &statement.inputs, &mut reserve)?;
        append(&mut out, &invocation, &mut reserve)?;
        append(&mut out, ";", &mut reserve)?;
        let memory = match (operation.memory_input, operation.memory_output) {
            (Some(input), Some(output)) => format!(" // memory m{} → m{}", input.0, output.0),
            (Some(input), None) => format!(" // memory input m{}", input.0),
            _ => String::new(),
        };
        let effect = match (operation.effect_input, operation.effect_output) {
            (Some(input), Some(output)) => format!(" // effect q{} → q{}", input.0, output.0),
            (Some(input), None) => format!(" // effect input q{}", input.0),
            _ => String::new(),
        };
        let dependencies = format!("{memory}{effect}");
        if show_state_dependencies {
            append(&mut out, &dependencies, &mut reserve)?;
        } else if !reserve(dependencies.len()) {
            // Visibility changes only the suffix, not the owner's work ledger.
            return Err(LiftedRenderError::Limit);
        }
        Ok(out)
    }
    /// Logical edge assignment. RHS expressions are evaluated simultaneously.
    pub fn render_transfer(
        &self,
        transfer: &LiftedTransfer,
        mut reserve: impl FnMut(usize) -> bool,
    ) -> Result<Option<String>, LiftedRenderError> {
        if transfer.arguments.is_empty() {
            return Ok(None);
        }
        let raw = self
            .edges
            .get(&transfer.edge)
            .copied()
            .ok_or(LiftedRenderError::MissingWitness(transfer.edge))?;
        let target = self
            .blocks
            .get(&raw.to)
            .copied()
            .ok_or(LiftedRenderError::MissingWitness(raw.to))?;
        Ok(Some(format!(
            "({}) = ({});",
            target
                .parameters
                .iter()
                .map(|v| format!("v{}", v.0))
                .collect::<Vec<_>>()
                .join(", "),
            transfer
                .arguments
                .iter()
                .map(|id| self.render_expression(*id, &mut reserve))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        )))
    }
}
impl LiftedProgram {
    /// Build reusable source/expression indexes for a batch of code rows.
    pub fn render_index<'a>(&'a self, ir: &'a Ir) -> LiftedRenderIndex<'a> {
        LiftedRenderIndex::new(self, Some(ir))
    }
    /// Bounded single-expression convenience; batch consumers use render_index.
    pub fn render_expression(&self, id: EntityId) -> String {
        match LiftedRenderIndex::new(self, None).render_expression(id, |_| true) {
            Ok(text) => text,
            Err(LiftedRenderError::Limit) => format!("expression_limit(e{})", id.0),
            Err(_) => format!("missing_expression(e{})", id.0),
        }
    }
    /// Format one primitive using semantic inputs and source dependency tokens.
    pub fn render_statement(&self, statement: &LiftedStatement, ir: &Ir) -> String {
        self.render_index(ir)
            .render_statement(statement, |_| true)
            .unwrap_or_else(|_| format!("statement_limit(s{});", statement.id.0))
    }
    /// Format one live simultaneous transfer, omitting an empty logical tuple.
    pub fn render_transfer(&self, transfer: &LiftedTransfer, ir: &Ir) -> Option<String> {
        self.render_index(ir)
            .render_transfer(transfer, |_| true)
            .unwrap_or_else(|_| Some(format!("transfer_limit(t{});", transfer.id.0)))
    }
}
