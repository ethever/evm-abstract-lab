//! Shared semantic formatting for lightweight consumers and CLI export.
use super::{
    LiftedBlock, LiftedControl, LiftedExpression, LiftedExpressionKind, LiftedProgram,
    LiftedRelation, LiftedStatement, LiftedTransfer, MAX_LIFTED_EXPRESSION_TEXT,
};
use crate::{EntityId, Ir};
use std::collections::BTreeMap;

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
            let mut call = |name: &str, args: &[EntityId]| {
                pending.push(Part::Text(")".to_owned()));
                for (i, arg) in args.iter().enumerate().rev() {
                    pending.push(Part::Expression(*arg));
                    if i > 0 {
                        pending.push(Part::Text(", ".to_owned()));
                    }
                }
                pending.push(Part::Text(format!("{name}(")));
            };
            match part {
                Part::Text(text) => out.push_str(&text),
                Part::Expression(next) => {
                    let e = self
                        .expressions
                        .get(&next)
                        .ok_or(LiftedRenderError::MissingExpression(next))?;
                    match &e.kind {
                        LiftedExpressionKind::Word(v) => out.push_str(&format!("0x{v:x}")),
                        LiftedExpressionKind::Predicate(v) => {
                            out.push_str(if *v { "true" } else { "false" })
                        }
                        LiftedExpressionKind::Value(v) => out.push_str(&format!("v{}", v.0)),
                        LiftedExpressionKind::WordFromPredicate(v) => call("word", &[*v]),
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
                                call(symbol, &[*left, *right]);
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
                                call(name, arguments);
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
    /// One retained primitive, formatted from high semantic inputs.
    pub fn render_statement(
        &self,
        statement: &LiftedStatement,
        mut reserve: impl FnMut(usize) -> bool,
    ) -> Result<String, LiftedRenderError> {
        let operation = self
            .operations
            .get(&statement.operation)
            .copied()
            .ok_or(LiftedRenderError::MissingWitness(statement.operation))?;
        let name = operation.name.to_lowercase();
        let lhs = if statement.outputs.is_empty() {
            String::new()
        } else {
            format!(
                "{} = ",
                statement
                    .outputs
                    .iter()
                    .map(|v| format!("v{}", v.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let args = statement
            .inputs
            .iter()
            .map(|id| self.render_expression(*id, &mut reserve))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        let dependencies = {
            let op = operation;
            let memory = match (op.memory_input, op.memory_output) {
                (Some(input), Some(output)) => format!(" // memory m{} → m{}", input.0, output.0),
                (Some(input), None) => format!(" // memory input m{}", input.0),
                _ => String::new(),
            };
            let effect = match (op.effect_input, op.effect_output) {
                (Some(input), Some(output)) => format!(" // effect q{} → q{}", input.0, output.0),
                (Some(input), None) => format!(" // effect input q{}", input.0),
                _ => String::new(),
            };
            format!("{memory}{effect}")
        };
        Ok(format!("{lhs}{name}({args});{dependencies}"))
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
