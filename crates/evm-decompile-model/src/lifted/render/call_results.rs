//! Source-derived dual call results. Names are a presentation plan, not new
//! serialized values; every consumed size observation remains in its witness.
use super::LiftedRenderError as E;
use super::{LiftedRenderIndex, control::ControlPlanIndex};
use crate::{
    EntityId, Ir, LiftedByteSegment, LiftedByteSource, LiftedExpressionKind as X, LiftedProgram,
    MAX_LIFTED_RECORDS, StructuredKind as K, StructuredNode, ValueKind,
};
use std::collections::{BTreeMap, BTreeSet};
mod source;
#[cfg(test)]
mod tests;

/// A source call with an actually used, versioned returndata result.
#[derive(Clone, Debug)]
pub struct LiftedCallResult {
    /// Original CALL-family operation.
    pub operation: EntityId,
    /// Retained raw status word; None prints a discarded first result.
    pub scalar: Option<EntityId>,
    /// Stable ordinal among related source calls, independent of view toggles.
    pub ordinal: usize,
    /// Source RETURNDATASIZE and RETURNDATACOPY observations.
    pub readers: Vec<EntityId>,
    /// Additional terminal consumers in occurrence-local data records.
    pub consumers: Vec<EntityId>,
}

impl<'a> LiftedRenderIndex<'a> {
    /// Admitted source call role; names remain presentation-only.
    pub fn call_result(&self, operation: EntityId) -> Option<&LiftedCallResult> {
        self.call_results.as_ref()?.call(operation)
    }
    /// The independently derived source epoch for a size/copy observation.
    pub fn returndata_source(&self, operation: EntityId) -> Option<EntityId> {
        self.call_results.as_ref()?.origin(operation)
    }
    /// Configure actual substituted AST occurrences before any formatting or
    /// branch planning. Recompute optional require/cut selection atomically.
    pub fn set_call_result_projection(
        &mut self,
        substituted: impl FnMut(EntityId) -> bool,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<(), E> {
        let Some(ir) = self.ir else { return Ok(()) };
        self.control_plans = ControlPlanIndex::default();
        if let (Some(plan), Some(root)) = (
            self.call_results.as_mut(),
            ir.structured.as_ref().map(|s| &s.root),
        ) && let Err(error) = plan.project(root, substituted, &mut pay)
        {
            // No previously selected cut or half-built name map survives
            // a refusal, even if a diagnostic owner reuses this index.
            self.call_results = None;
            return Err(error);
        }
        self.control_plans = ControlPlanIndex::new(self, ir, &mut pay);
        Ok(())
    }
    /// Bound object at this source use in the main rendering scope. Unknown or
    /// outlined-only objects have no external handle and retain raw expressions.
    pub fn returndata_name(
        &self,
        call: EntityId,
        use_block: EntityId,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<Option<String>, E> {
        paid(&mut pay, 64)?;
        Ok(self
            .call_buffer(call, use_block, None)
            .map(|b| format!("returndata{}", b.ordinal)))
    }
    pub(super) fn call_buffer(
        &self,
        call: EntityId,
        block: EntityId,
        scope: Option<&crate::SharedProcedure>,
    ) -> Option<&LiftedCallResult> {
        let plan = self.call_results.as_ref()?;
        plan.buffer(call, block, plan.scope(scope)?)
    }
    pub(crate) fn binding_name(&self, value: EntityId) -> String {
        self.call_results
            .as_ref()
            .and_then(|p| p.scalar_name(value))
            .unwrap_or_else(|| format!("v{}", value.0))
    }
    pub(super) fn expression_name(
        &self,
        value: EntityId,
        block: EntityId,
        scope: Option<&crate::SharedProcedure>,
    ) -> String {
        if !scope.is_some_and(|p| p.interface.inputs.contains(&value))
            && let Some(plan) = &self.call_results
            && let Some(scope) = plan.scope(scope)
            && let Some(name) = plan.size_alias(value, block, scope)
        {
            return name;
        }
        self.binding_name(value)
    }
    pub(super) fn statement_has_row(
        &self,
        statement: &crate::LiftedStatement,
        boundary: EntityId,
    ) -> bool {
        self.call_results
            .as_ref()
            .is_none_or(|p| !p.hide(statement.operation, boundary))
    }
    /// Typed optional row. A consumed RDS still executes at its original source
    /// position through the unchanged primitive/lifted witness.
    pub fn render_statement_row_with_dependencies(
        &self,
        statement: &crate::LiftedStatement,
        show_state_dependencies: bool,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<Option<String>, E> {
        paid(&mut pay, 16)?;
        if let Some(plan) = &self.call_results
            && let Some(scope) = plan.scope(None)
            && !self.statement_has_row(statement, scope)
        {
            return Ok(None);
        }
        self.render_statement_with_dependencies(statement, show_state_dependencies, pay)
            .map(Some)
    }
    pub(crate) fn scoped_statement_has_row(
        &self,
        statement: &crate::LiftedStatement,
        scope: Option<&crate::SharedProcedure>,
    ) -> bool {
        self.call_results
            .as_ref()
            .and_then(|p| p.scope(scope))
            .is_none_or(|boundary| self.statement_has_row(statement, boundary))
    }
}
impl LiftedCallResult {
    /// Inspectable raw identities; the result object itself is not a wire entity.
    pub fn source_references(&self) -> impl Iterator<Item = EntityId> + '_ {
        std::iter::once(self.operation)
            .chain(self.scalar)
            .chain(self.readers.iter().copied())
            .chain(self.consumers.iter().copied())
    }
}
#[derive(Clone, Copy)]
struct Site {
    start: usize,
    end: usize,
    blocked: Option<EntityId>,
}
#[derive(Default)]
struct Projection {
    sites: BTreeMap<EntityId, Site>,
    blocks: BTreeMap<EntityId, Vec<EntityId>>,
}
#[derive(Clone, Copy)]
struct Read {
    call: EntityId,
    block: EntityId,
    position: usize,
    value: Option<EntityId>,
    is_size: bool,
}

pub(super) struct CallResults<'a> {
    facts: source::CallSources<'a>,
    calls: BTreeMap<EntityId, LiftedCallResult>,
    call_blocks: BTreeMap<EntityId, EntityId>,
    reads: BTreeMap<EntityId, Read>,
    sizes: BTreeMap<EntityId, EntityId>,
    scalar_calls: BTreeMap<EntityId, EntityId>,
    terminal_uses: Vec<(EntityId, EntityId)>,
    scopes: Vec<EntityId>,
    main: Option<EntityId>,
    exposed: BTreeSet<EntityId>,
    unsafe_sizes: BTreeSet<EntityId>,
    projection: Projection,
    needed: BTreeSet<(EntityId, EntityId)>,
    named: BTreeSet<EntityId>,
}
fn paid(pay: &mut impl FnMut(usize) -> bool, n: usize) -> Result<(), E> {
    if pay(n) { Ok(()) } else { Err(E::Limit) }
}
impl<'a> CallResults<'a> {
    pub(super) fn new(
        ir: &'a Ir,
        program: &LiftedProgram,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<Self, E> {
        paid(
            pay,
            program
                .record_count()
                .saturating_add(ir.values.len())
                .saturating_add(ir.edges.len())
                .saturating_mul(16)
                .saturating_add(64),
        )?;
        let facts = source::analyze(ir, pay)?;
        let high: BTreeMap<_, _> = program
            .blocks
            .iter()
            .flat_map(|b| {
                b.statements
                    .iter()
                    .map(move |s| (s.operation, (b.block, s)))
            })
            .collect();
        let values: BTreeMap<_, _> = ir.values.iter().map(|v| (v.id, &v.kind)).collect();
        let mut related = BTreeSet::new();
        let mut reads = BTreeMap::new();
        let mut sizes = BTreeMap::new();
        for block in &ir.blocks {
            for op in &block.operations {
                paid(pay, 8)?;
                let Some(call) = facts.origin(op.id) else {
                    continue;
                };
                let Some((owner, stmt)) = high.get(&op.id) else {
                    continue;
                };
                if *owner != block.id || stmt.inputs.len() != op.inputs.len() {
                    continue;
                }
                let Some((_, position)) = facts.position(op.id) else {
                    continue;
                };
                if !facts.available(call, block.id, position) {
                    continue;
                }
                let is_size = op.opcode == 0x3d;
                let value = if is_size {
                    op.outputs.first().copied().filter(|v| {
                        stmt.outputs.as_slice() == [*v] && facts.word_origin(*v) == Some(call)
                    })
                } else {
                    None
                };
                if !is_size || value.is_some() {
                    related.insert(call);
                }
                if let Some(value) = value {
                    sizes.insert(value, op.id);
                }
                reads.insert(
                    op.id,
                    Read {
                        call,
                        block: block.id,
                        position,
                        value,
                        is_size,
                    },
                );
            }
        }
        let mut terminal_uses = Vec::new();
        for data in &program.revert_data {
            for part in data.content.segments() {
                if let LiftedByteSegment::Slice {
                    source: LiftedByteSource::ReturnData(call),
                    ..
                } = part
                    && data.writes.len() == 1
                    && facts.origin(data.writes[0]) == Some(*call)
                {
                    related.insert(*call);
                    terminal_uses.push((*call, data.block));
                }
            }
        }
        let mut ordered = Vec::new();
        for block in &ir.blocks {
            for op in &block.operations {
                let Some(op) = facts.operation(op.id) else {
                    continue;
                };
                if !related.contains(&op.id) || !source::call_shape(op) {
                    continue;
                }
                let Some((owner, stmt)) = high.get(&op.id) else {
                    continue;
                };
                let Some(raw) = op.outputs.first().copied() else {
                    continue;
                };
                if *owner != block.id
                    || stmt.inputs.len() != op.inputs.len()
                    || !matches!(values.get(&raw),Some(ValueKind::Operation {operation,index:0}) if *operation==op.id)
                    || !(stmt.outputs.is_empty() || stmt.outputs.as_slice() == [raw])
                {
                    continue;
                }
                ordered.push((
                    op.source.map_or(usize::MAX, |span| span.pc),
                    op.id,
                    block.id,
                    (!stmt.outputs.is_empty()).then_some(raw),
                ));
            }
        }
        paid(
            pay,
            ordered
                .len()
                .saturating_mul(ordered.len().max(1).ilog2() as usize + 1)
                .saturating_mul(8),
        )?;
        ordered.sort_unstable();
        let mut calls = BTreeMap::new();
        let mut call_blocks = BTreeMap::new();
        let mut scalar_calls = BTreeMap::new();
        for (ordinal, (_, op, block, scalar)) in ordered.into_iter().enumerate() {
            if let Some(scalar) = scalar {
                scalar_calls.insert(scalar, op);
            }
            call_blocks.insert(op, block);
            calls.insert(
                op,
                LiftedCallResult {
                    operation: op,
                    scalar,
                    ordinal: ordinal + 1,
                    readers: Vec::new(),
                    consumers: Vec::new(),
                },
            );
        }
        for (op, read) in &reads {
            if let Some(call) = calls.get_mut(&read.call) {
                call.readers.push(*op);
            }
        }
        for (op, block) in &terminal_uses {
            if let Some(call) = calls.get_mut(op) {
                call.consumers.push(*block);
            }
        }
        let main = ir.structured.as_ref().map(|s| s.root.id);
        let mut scopes: Vec<_> = main
            .into_iter()
            .chain(program.procedures.iter().map(|p| p.body))
            .collect();
        scopes.sort_unstable();
        scopes.dedup();
        let mut exposed = BTreeSet::new();
        let procedures: BTreeMap<_, _> = program.procedures.iter().map(|p| (p.id, p)).collect();
        for p in &program.procedures {
            if p.kind != crate::SharedProcedureKind::PureArithmetic {
                exposed.extend(
                    p.interface
                        .inputs
                        .iter()
                        .chain(&p.interface.outputs)
                        .copied(),
                );
            }
        }
        for invocation in &program.invocations {
            if let Some(p) = procedures
                .get(&invocation.procedure)
                .filter(|p| p.kind != crate::SharedProcedureKind::PureArithmetic)
            {
                exposed.extend(
                    p.interface
                        .inputs
                        .iter()
                        .chain(&p.interface.outputs)
                        .filter_map(|v| invocation.actual(*v)),
                );
            }
        }
        // Propagate the earliest real consumer position backwards through the
        // topological arena. This is linear-space even for heavily shared DAGs.
        let mut unsafe_sizes = BTreeSet::new();
        let terminal_positions: BTreeMap<_, _> = ir
            .blocks
            .iter()
            .map(|b| (b.id, b.operations.len().saturating_sub(1)))
            .collect();
        let expression_positions: BTreeMap<_, _> = program
            .expressions
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id, (i, e.block)))
            .collect();
        let raw_edges: BTreeMap<_, _> = ir.edges.iter().map(|e| (e.id, e)).collect();
        let mut earliest = BTreeMap::<EntityId, usize>::new();
        let mut supported = true;
        let mut use_at = |expression: EntityId, block: EntityId, before: usize| {
            if expression_positions
                .get(&expression)
                .is_none_or(|(_, owner)| *owner != block)
            {
                supported = false;
                return;
            }
            earliest
                .entry(expression)
                .and_modify(|old| *old = (*old).min(before))
                .or_insert(before);
        };
        for block in &program.blocks {
            for statement in &block.statements {
                paid(pay, statement.inputs.len().saturating_mul(8))?;
                let Some((owner, before)) = facts.position(statement.operation) else {
                    continue;
                };
                if owner != block.block {
                    use_at(EntityId(0), block.block, before);
                    continue;
                }
                for input in &statement.inputs {
                    use_at(*input, owner, before);
                }
            }
            let before = terminal_positions.get(&block.block).copied().unwrap_or(0);
            paid(
                pay,
                block.exit_values.len().saturating_add(1).saturating_mul(8),
            )?;
            for expr in block.condition.iter().chain(block.exit_values.iter()) {
                use_at(*expr, block.block, before);
            }
        }
        for transfer in &program.transfers {
            paid(pay, transfer.arguments.len().saturating_mul(8))?;
            if let Some(edge) = raw_edges.get(&transfer.edge) {
                let before = terminal_positions.get(&edge.from).copied().unwrap_or(0);
                for expr in &transfer.arguments {
                    use_at(*expr, edge.from, before);
                }
            }
        }
        for control in &program.controls {
            paid(pay, 8)?;
            if let Some((_, block)) = expression_positions.get(&control.condition) {
                use_at(
                    control.condition,
                    *block,
                    terminal_positions.get(block).copied().unwrap_or(0),
                );
            }
        }
        for (position, expression) in program.expressions.iter().enumerate().rev() {
            let Some(before) = earliest.get(&expression.id).copied() else {
                continue;
            };
            let count = match &expression.kind {
                X::WordOp { arguments, .. } => arguments.len(),
                X::Compare { .. } => 2,
                X::Not(_) | X::NonZero(_) | X::WordFromPredicate(_) => 1,
                _ => 0,
            };
            paid(pay, count.saturating_mul(16).saturating_add(8))?;
            for operand in expression.kind.operands() {
                if expression_positions
                    .get(&operand)
                    .is_none_or(|(child, owner)| *child >= position || *owner != expression.block)
                {
                    supported = false;
                    continue;
                }
                earliest
                    .entry(operand)
                    .and_modify(|old| *old = (*old).min(before))
                    .or_insert(before);
            }
            if let X::Value(value) = expression.kind
                && let Some(read) = sizes.get(&value).and_then(|op| reads.get(op))
                && !facts.available(read.call, expression.block, before)
            {
                unsafe_sizes.insert(value);
            }
        }
        if !supported {
            unsafe_sizes.extend(sizes.keys().copied());
        }
        Ok(Self {
            facts,
            calls,
            call_blocks,
            reads,
            sizes,
            scalar_calls,
            terminal_uses,
            scopes,
            main,
            exposed,
            unsafe_sizes,
            projection: Projection::default(),
            needed: BTreeSet::new(),
            named: BTreeSet::new(),
        })
    }
    pub(super) fn project(
        &mut self,
        root: &StructuredNode,
        mut substitute: impl FnMut(EntityId) -> bool,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(), E> {
        enum Task<'a> {
            Enter(&'a StructuredNode, Option<EntityId>),
            Exit(EntityId),
        }
        paid(pay, 64)?;
        let mut todo = vec![Task::Enter(root, None)];
        let mut projection = Projection::default();
        let mut order = 0_usize;
        while let Some(task) = todo.pop() {
            paid(pay, 32)?;
            match task {
                Task::Enter(node, blocked) => {
                    if projection.sites.len() >= MAX_LIFTED_RECORDS
                        || projection.sites.contains_key(&node.id)
                    {
                        return Err(E::Limit);
                    }
                    let blocked = if substitute(node.id) {
                        Some(node.id)
                    } else {
                        blocked
                    };
                    projection.sites.insert(
                        node.id,
                        Site {
                            start: order,
                            end: order,
                            blocked,
                        },
                    );
                    order += 1;
                    todo.push(Task::Exit(node.id));
                    match &node.kind {
                        K::Block { block } => {
                            projection.blocks.entry(*block).or_default().push(node.id)
                        }
                        K::Dispatcher { cases, .. } => {
                            paid(pay, cases.len().saturating_mul(8))?;
                            for case in cases {
                                projection
                                    .blocks
                                    .entry(case.block)
                                    .or_default()
                                    .push(node.id);
                            }
                        }
                        K::Sequence(children) => {
                            paid(pay, children.len().saturating_mul(8))?;
                            todo.extend(children.iter().rev().map(|c| Task::Enter(c, blocked)));
                        }
                        K::If {
                            then_branch,
                            else_branch,
                            ..
                        } => {
                            todo.push(Task::Enter(else_branch, blocked));
                            todo.push(Task::Enter(then_branch, blocked));
                        }
                        K::While { header, body, .. } => {
                            todo.push(Task::Enter(body, blocked));
                            todo.push(Task::Enter(header, blocked));
                        }
                        K::Indirect { arms, .. } => {
                            paid(pay, arms.len().saturating_mul(8))?;
                            todo.extend(arms.iter().rev().map(|a| Task::Enter(&a.body, blocked)));
                        }
                        _ => {}
                    }
                }
                Task::Exit(id) => {
                    projection
                        .sites
                        .get_mut(&id)
                        .ok_or(E::MissingWitness(id))?
                        .end = order
                }
            }
        }
        self.projection = projection;
        self.needed.clear();
        self.named.clear();
        let mut checks = 0_usize;
        for scope in &self.scopes {
            for read in self.reads.values() {
                paid(pay, 16)?;
                checks += 1;
                if checks > MAX_LIFTED_RECORDS {
                    break;
                }
                if (!read.is_size || read.value.is_some())
                    && self.visible(read.block, *scope)
                    && self.call_available(read.call, read.block, read.position, *scope)
                {
                    self.needed.insert((read.call, *scope));
                    self.named.insert(read.call);
                }
            }
            for (call, block) in &self.terminal_uses {
                paid(pay, 16)?;
                checks += 1;
                if checks > MAX_LIFTED_RECORDS {
                    break;
                }
                if self.visible(*block, *scope)
                    && self.call_available(*call, *block, usize::MAX, *scope)
                {
                    self.needed.insert((*call, *scope));
                    self.named.insert(*call);
                }
            }
        }
        Ok(())
    }
    fn visible(&self, block: EntityId, scope: EntityId) -> bool {
        let Some(boundary) = self.projection.sites.get(&scope) else {
            return false;
        };
        let Some(sites) = self.projection.blocks.get(&block) else {
            return false;
        };
        let mut found = false;
        for id in sites {
            let Some(site) = self.projection.sites.get(id) else {
                return false;
            };
            if site.start < boundary.start || site.start >= boundary.end {
                continue;
            }
            found = true;
            if let Some(owner) = site.blocked
                && owner != scope
                && self
                    .projection
                    .sites
                    .get(&owner)
                    .is_some_and(|owner| owner.start >= boundary.start)
            {
                return false;
            }
        }
        found
    }
    fn call_available(
        &self,
        call: EntityId,
        block: EntityId,
        before: usize,
        scope: EntityId,
    ) -> bool {
        self.call_blocks
            .get(&call)
            .is_some_and(|owner| self.visible(*owner, scope))
            && self.facts.available(call, block, before)
    }
    pub(super) fn scope(&self, scope: Option<&crate::SharedProcedure>) -> Option<EntityId> {
        scope.map(|p| p.body).or(self.main)
    }
    pub(super) fn call(&self, op: EntityId) -> Option<&LiftedCallResult> {
        self.calls.get(&op)
    }
    pub(super) fn origin(&self, op: EntityId) -> Option<EntityId> {
        self.reads.get(&op).map(|r| r.call)
    }
    pub(super) fn data_source(&self, data: &crate::LiftedRevertData, call: EntityId) -> bool {
        data.writes.len() == 1 && self.facts.origin(data.writes[0]) == Some(call)
    }
    pub(super) fn word_source(&self, value: EntityId) -> Option<EntityId> {
        self.facts.word_origin(value)
    }
    pub(super) fn scalar_name(&self, value: EntityId) -> Option<String> {
        self.scalar_calls
            .get(&value)
            .filter(|call| self.named.contains(call))
            .and_then(|call| self.calls.get(call))
            .map(|call| format!("success{}", call.ordinal))
    }
    pub(super) fn buffer(
        &self,
        call: EntityId,
        block: EntityId,
        scope: EntityId,
    ) -> Option<&LiftedCallResult> {
        (self.needed.contains(&(call, scope))
            && self.call_available(call, block, usize::MAX, scope))
        .then(|| self.calls.get(&call))
        .flatten()
    }
    pub(super) fn read_buffer(&self, op: EntityId, scope: EntityId) -> Option<&LiftedCallResult> {
        let read = self.reads.get(&op)?;
        (read.is_size && self.visible(read.block, scope))
            .then(|| self.buffer(read.call, read.block, scope))
            .flatten()
    }
    pub(super) fn hide(&self, op: EntityId, scope: EntityId) -> bool {
        // Word-only procedure interfaces retain their scalar size binding.
        if Some(scope) != self.main {
            return false;
        }
        let Some(read) = self.reads.get(&op) else {
            return false;
        };
        self.read_buffer(op, scope).is_some()
            && read.value.is_none_or(|value| {
                !self.exposed.contains(&value) && !self.unsafe_sizes.contains(&value)
            })
    }
    pub(super) fn size_alias(
        &self,
        value: EntityId,
        block: EntityId,
        scope: EntityId,
    ) -> Option<String> {
        let op = *self.sizes.get(&value)?;
        if !self.hide(op, scope) {
            return None;
        }
        let call = self.reads.get(&op)?.call;
        self.buffer(call, block, scope)
            .map(|b| format!("returndata{}.size", b.ordinal))
    }
}
