//! Shared semantic formatting for lightweight consumers and CLI export.
use super::{
    LiftedBlock, LiftedControl, LiftedExpression, LiftedExpressionKind, LiftedFailure, LiftedGuard,
    LiftedProgram, LiftedRelation, LiftedStatement, LiftedTransfer, MAX_LIFTED_EXPRESSION_TEXT,
};
use super::{LiftedModelError, sources::GuardSources};
use crate::{EntityId, ExitKind, Ir, StructuredNode};
use std::collections::BTreeMap;

mod failure_first;
pub(crate) mod primitive;
pub use failure_first::FailureFirstIf;
use failure_first::FailureFirstIndex;
use primitive::{CallName, append};

/// Reusable indexes for rendering many semantic rows without rescanning arenas.
/// Owners reserve index construction from their own work ledger before creating it.
pub struct LiftedRenderIndex<'a> {
    pub(crate) program: &'a LiftedProgram,
    pub(crate) shared_source: Option<crate::SharedSourceIndex<'a>>,
    pub(crate) shared_error: Option<LiftedRenderError>,
    pub(crate) shared_calls: BTreeMap<EntityId, &'a crate::SharedInvocation>,
    pub(crate) shared_definitions: BTreeMap<EntityId, &'a crate::SharedProcedure>,
    expressions: BTreeMap<EntityId, &'a LiftedExpression>,
    operations: BTreeMap<EntityId, &'a crate::Operation>,
    edges: BTreeMap<EntityId, &'a crate::Edge>,
    blocks: BTreeMap<EntityId, &'a LiftedBlock>,
    transfers: BTreeMap<EntityId, &'a LiftedTransfer>,
    controls: BTreeMap<EntityId, &'a LiftedControl>,
    nodes: BTreeMap<EntityId, &'a StructuredNode>,
    failures: BTreeMap<EntityId, &'a LiftedFailure>,
    omissions: BTreeMap<(EntityId, usize), usize>,
    payloads: BTreeMap<EntityId, &'a super::LiftedErrorPayload>,
    reverts: BTreeMap<EntityId, &'a super::LiftedRevert>,
    constructions: BTreeMap<EntityId, &'a super::LiftedConstruction>,
    guard_source_error: Option<LiftedRenderError>,
    failure_first: Result<FailureFirstIndex, LiftedRenderError>,
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
    /// Guard metadata would escape its owned source arm or forge a suffix cut.
    InvalidGuardSource(EntityId),
    /// Sharing certificate failed direct source validation; omission is quarantined.
    InvalidSharedSource(EntityId),
}
impl std::fmt::Display for LiftedRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lifted rendering refused: {self:?}")
    }
}
impl std::error::Error for LiftedRenderError {}

impl<'a> LiftedRenderIndex<'a> {
    fn new(
        program: &'a LiftedProgram,
        ir: Option<&'a Ir>,
        mut pay: impl FnMut(usize) -> bool,
        include_shared: bool,
    ) -> Self {
        let mut basic = program.record_count();
        if let Some(ir) = ir {
            basic = basic
                .saturating_add(ir.values.len())
                .saturating_add(ir.edges.len());
            for b in &ir.blocks {
                basic = basic
                    .saturating_add(b.operations.len())
                    .saturating_add(b.parameters.len())
                    .saturating_add(4);
            }
        }
        if !pay(basic.saturating_mul(4)) {
            return Self::refused(program);
        }
        let expressions = program.expressions.iter().map(|e| (e.id, e)).collect();
        let edges = ir
            .into_iter()
            .flat_map(|ir| &ir.edges)
            .map(|e| (e.id, e))
            .collect();
        let blocks = program.blocks.iter().map(|b| (b.block, b)).collect();
        let sources = ir.map(|ir| GuardSources::new(program, ir, &expressions, &blocks, &edges));
        let (nodes, omissions, guard_source_error) = match sources {
            Some(sources) => (
                sources.nodes,
                sources.omissions,
                sources.error.map(|error| match error {
                    LiftedModelError::GuardSource(id) | LiftedModelError::Payload(id) => {
                        LiftedRenderError::InvalidGuardSource(id)
                    }
                    _ => LiftedRenderError::Limit,
                }),
            ),
            None => (
                BTreeMap::new(),
                BTreeMap::new(),
                program
                    .controls
                    .iter()
                    .find(|c| c.guard.is_some())
                    .map(|c| LiftedRenderError::MissingWitness(c.node)),
            ),
        };
        let shared = ir
            .filter(|_| include_shared && program.has_procedures())
            .map(|ir| {
                crate::SharedSourceIndex::for_program(ir, Some(program), &mut pay).and_then(
                    |index| {
                        index.validate(program, &mut pay)?;
                        Ok(index)
                    },
                )
            });
        let (shared_source, shared_error) = match shared {
            Some(Ok(index)) => (Some(index), None),
            Some(Err(crate::SharedProcedureError::Limit)) => (None, Some(LiftedRenderError::Limit)),
            Some(Err(_)) => (
                None,
                Some(LiftedRenderError::InvalidSharedSource(EntityId(0))),
            ),
            None => (None, None),
        };
        let mut result = Self {
            program,
            shared_source,
            shared_error,
            shared_calls: program.invocations.iter().map(|i| (i.source, i)).collect(),
            shared_definitions: program.procedures.iter().map(|p| (p.id, p)).collect(),
            expressions,
            operations: ir
                .into_iter()
                .flat_map(|ir| &ir.blocks)
                .flat_map(|b| &b.operations)
                .map(|o| (o.id, o))
                .collect(),
            edges,
            blocks,
            transfers: program.transfers.iter().map(|t| (t.edge, t)).collect(),
            controls: program.controls.iter().map(|c| (c.node, c)).collect(),
            nodes,
            failures: program.failures.iter().map(|f| (f.block, f)).collect(),
            omissions,
            payloads: program.payloads.iter().map(|p| (p.id, p)).collect(),
            reverts: program.reverts.iter().map(|r| (r.block, r)).collect(),
            constructions: program.constructions.iter().map(|c| (c.block, c)).collect(),
            guard_source_error,
            failure_first: Ok(FailureFirstIndex::default()),
        };
        result.failure_first = if let Some(error) = guard_source_error {
            Err(error)
        } else if let Some(ir) = ir {
            FailureFirstIndex::new(&result, ir, &mut pay)
        } else {
            Ok(FailureFirstIndex::default())
        };
        result
    }
    fn refused(program: &'a LiftedProgram) -> Self {
        Self {
            program,
            shared_source: None,
            shared_error: Some(LiftedRenderError::Limit),
            shared_calls: BTreeMap::new(),
            shared_definitions: BTreeMap::new(),
            expressions: BTreeMap::new(),
            operations: BTreeMap::new(),
            edges: BTreeMap::new(),
            blocks: BTreeMap::new(),
            transfers: BTreeMap::new(),
            controls: BTreeMap::new(),
            nodes: BTreeMap::new(),
            failures: BTreeMap::new(),
            omissions: BTreeMap::new(),
            payloads: BTreeMap::new(),
            reverts: BTreeMap::new(),
            constructions: BTreeMap::new(),
            guard_source_error: Some(LiftedRenderError::Limit),
            failure_first: Err(LiftedRenderError::Limit),
        }
    }
    /// Structural guard admission for this index, independent of stored Passed
    /// status or core's semantic failure proof. Invalid overlays are quarantined.
    pub fn validate_guard_sources(&self) -> Result<(), LiftedRenderError> {
        self.guard_source_error.map_or(Ok(()), Err)
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
        self.controls
            .get(&source)
            .copied()
            .filter(|c| c.guard.is_none() || self.guard_source_error.is_none())
    }
    /// Certified guard at this exact source If occurrence.
    pub fn guard(&self, source: EntityId) -> Option<&'a LiftedGuard> {
        self.control(source)?.guard.as_ref()
    }
    /// Raw source occurrence retained for executing the certified successful arm.
    pub fn node(&self, source: EntityId) -> Option<&'a StructuredNode> {
        self.nodes.get(&source).copied()
    }
    /// Shared failure proof fact without rescanning all facts.
    pub fn failure(&self, source: EntityId) -> Option<&'a LiftedFailure> {
        self.failures.get(&source).copied()
    }
    /// Exclusive end of a certified, unreachable following failure suffix.
    pub fn sequence_end(&self, sequence: EntityId, position: usize) -> Option<usize> {
        self.omissions.get(&(sequence, position)).copied()
    }
    /// Shared byte identity, quarantined when source admission fails.
    pub fn payload(&self, id: EntityId) -> Option<&'a super::LiftedErrorPayload> {
        self.guard_source_error
            .is_none()
            .then(|| self.payloads.get(&id).copied())
            .flatten()
    }
    /// Terminal proof over the original block's memory version.
    pub fn revert(&self, block: EntityId) -> Option<&'a super::LiftedRevert> {
        self.guard_source_error
            .is_none()
            .then(|| self.reverts.get(&block).copied())
            .flatten()
    }
    /// Exact lazy source statement interval; no global primitive deletion.
    pub fn construction(&self, block: EntityId) -> Option<&'a super::LiftedConstruction> {
        self.guard_source_error
            .is_none()
            .then(|| self.constructions.get(&block).copied())
            .flatten()
    }
    /// Compact statement representing the original lazy construction and faults.
    pub fn render_revert(
        &self,
        block: EntityId,
        mut reserve: impl FnMut(usize) -> bool,
    ) -> Result<Option<String>, LiftedRenderError> {
        self.validate_guard_sources()?;
        if self.construction(block).is_none() {
            return Ok(None);
        }
        let proof = self
            .revert(block)
            .ok_or(LiftedRenderError::MissingWitness(block))?;
        let payload = self
            .payload(proof.payload)
            .ok_or(LiftedRenderError::MissingWitness(proof.payload))?;
        let mut out = String::new();
        CallName::Evm("revert").append(&mut out, &mut reserve)?;
        append(&mut out, "(", &mut reserve)?;
        quoted(&mut out, &payload.message, &mut reserve)?;
        append(&mut out, ");", &mut reserve)?;
        Ok(Some(out))
    }
    /// Compact guard statement; pass transfers remain in the raw pass arm.
    pub fn render_guard(
        &self,
        control: &LiftedControl,
        mut reserve: impl FnMut(usize) -> bool,
    ) -> Result<Option<String>, LiftedRenderError> {
        let Some(guard) = &control.guard else {
            return Ok(None);
        };
        self.validate_guard_sources()?;
        if self.controls.get(&control.node).copied() != Some(control) {
            return Err(LiftedRenderError::InvalidGuardSource(control.node));
        }
        let condition = self.render_expression(control.condition, &mut reserve)?;
        let mut out = String::new();
        append(&mut out, "require(", &mut reserve)?;
        if !guard.pass_nonzero {
            append(&mut out, "!(", &mut reserve)?;
        }
        append(&mut out, &condition, &mut reserve)?;
        if !guard.pass_nonzero {
            append(&mut out, ")", &mut reserve)?;
        }
        if let super::LiftedGuardPayload::ErrorString(id) = guard.payload {
            let payload = self
                .payload(id)
                .ok_or(LiftedRenderError::MissingWitness(id))?;
            append(&mut out, ", ", &mut reserve)?;
            quoted(&mut out, &payload.message, &mut reserve)?;
        }
        append(&mut out, ");", &mut reserve)?;
        Ok(Some(out))
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
            let mut call = |name: CallName<'_>, args: &[EntityId], names: Option<&[&str]>| {
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
                pending.push(Part::Text(name.opening()));
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
                        LiftedExpressionKind::WordFromPredicate(v) => {
                            call(CallName::High("word"), &[*v], None)?
                        }
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
                                call(
                                    CallName::Evm(symbol),
                                    &[*left, *right],
                                    Some(primitive::LEFT_RIGHT),
                                )?;
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
                                    0x19 => "not",
                                    0x1a => "byte",
                                    0x1b => "shl",
                                    0x1c => "shr",
                                    0x1d => "sar",
                                    0x1e => "clz",
                                    _ => "word_op",
                                };
                                call(
                                    CallName::Evm(name),
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
    /// One qualified EVM call from semantic inputs, without a semicolon.
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
        CallName::Evm(name).append(&mut out, &mut reserve)?;
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

    /// One terminal primitive with the same spelling in every code consumer.
    /// Compact error constructions are handled separately by `render_revert`.
    pub fn render_exit(
        &self,
        kind: ExitKind,
        arguments: &[EntityId],
        mut reserve: impl FnMut(usize) -> bool,
    ) -> Result<String, LiftedRenderError> {
        let (opcode, name) = match kind {
            ExitKind::Stop | ExitKind::EndOfCode => (0x00, "stop"),
            ExitKind::Return => (0xf3, "return"),
            ExitKind::Revert => (0xfd, "revert"),
            ExitKind::SelfDestruct => (0xff, "selfdestruct"),
        };
        let mut out = self.render_invocation(opcode, name, arguments, &mut reserve)?;
        append(&mut out, ";", &mut reserve)?;
        if kind == ExitKind::EndOfCode {
            append(&mut out, " // end of code", &mut reserve)?;
        }
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
        let mut out = String::new();
        append(&mut out, "(", &mut reserve)?;
        for (position, id) in target.parameters.iter().enumerate() {
            if position > 0 {
                append(&mut out, ", ", &mut reserve)?;
            }
            append(&mut out, &format!("v{}", id.0), &mut reserve)?;
        }
        append(&mut out, ") = (", &mut reserve)?;
        for (position, id) in transfer.arguments.iter().enumerate() {
            if position > 0 {
                append(&mut out, ", ", &mut reserve)?;
            }
            let value = self.render_expression(*id, &mut reserve)?;
            append(&mut out, &value, &mut reserve)?;
        }
        append(&mut out, ");", &mut reserve)?;
        Ok(Some(out))
    }
}
impl LiftedProgram {
    /// Build reusable source/expression indexes for a batch of code rows.
    pub fn render_index<'a>(&'a self, ir: &'a Ir) -> LiftedRenderIndex<'a> {
        {
            let mut remaining = super::MAX_LIFTED_RECORDS.saturating_mul(256);
            LiftedRenderIndex::new(
                self,
                Some(ir),
                |n| {
                    if n > remaining {
                        false
                    } else {
                        remaining -= n;
                        true
                    }
                },
                true,
            )
        }
    }
    /// Prepare one reusable index under the owner's cancellation/work ledger.
    /// Invalid sharing is quarantined; refusal is reported by validation APIs.
    pub fn render_index_with_budget<'a>(
        &'a self,
        ir: &'a Ir,
        reserve: impl FnMut(usize) -> bool,
    ) -> LiftedRenderIndex<'a> {
        LiftedRenderIndex::new(self, Some(ir), reserve, true)
    }
    /// Base expressions/guards/capsules only; performs no shared Flow proof and
    /// exposes no shared definitions/call omissions. Immutable owners can cache
    /// the full index's formatted shared rows and use this for normal windows.
    pub fn render_base_index_with_budget<'a>(
        &'a self,
        ir: &'a Ir,
        reserve: impl FnMut(usize) -> bool,
    ) -> LiftedRenderIndex<'a> {
        LiftedRenderIndex::new(self, Some(ir), reserve, false)
    }
    /// Bounded single-expression convenience; batch consumers use render_index.
    pub fn render_expression(&self, id: EntityId) -> String {
        match LiftedRenderIndex::new(self, None, |_| true, false).render_expression(id, |_| true) {
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

fn quoted(
    out: &mut String,
    message: &str,
    reserve: &mut impl FnMut(usize) -> bool,
) -> Result<(), LiftedRenderError> {
    append(out, "\"", reserve)?;
    for c in message.chars() {
        let text = match c {
            '"' => "\\\"".to_owned(),
            '\\' => "\\\\".to_owned(),
            '\n' => "\\n".to_owned(),
            '\r' => "\\r".to_owned(),
            '\t' => "\\t".to_owned(),
            c if c.is_control() => format!("\\u{:04x}", u32::from(c)),
            c => c.to_string(),
        };
        append(out, &text, reserve)?;
    }
    append(out, "\"", reserve)
}
