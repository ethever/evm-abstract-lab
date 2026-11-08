//! Scoped semantic DAG construction, simplification and logical liveness.
//!
//! The primitive witness is immutable. Expressions capture only earlier pure
//! definitions in their own block. Copy proofs introduce consumer-scoped raw
//! leaves, never cross-block arena edges; their definitions remain bound once.
//! Logical entry liveness and edge argument projection reach a joint fixed point.
use crate::{
    Completion, DiagnosticCode, EntityId, Ir, LiftedBlock, LiftedControl, LiftedExpression,
    LiftedExpressionKind as Kind, LiftedProgram, LiftedStatement, LiftedTransfer, Progress,
    StageKind, StructuredKind, StructuredNode, Terminator, ValueKind, artifact::Builder,
};
use std::collections::{BTreeMap, BTreeSet};

mod aliases;
mod availability;
mod guards;
mod liveness;
mod materialize;
mod payloads;
mod shared;
mod simplify;
mod validate;
pub(crate) use validate::certify;

pub(crate) fn run(owner: &mut Builder<impl FnMut(Progress)>) {
    run_with_sharing_limits(owner, usize::MAX, usize::MAX);
}
fn run_with_sharing_limits(
    owner: &mut Builder<impl FnMut(Progress)>,
    allowance: usize,
    max_event_slots: usize,
) {
    if owner.stopped || owner.current().kind != StageKind::Structured {
        return;
    }
    let certification_work = owner
        .current()
        .ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .filter(|o| evm_bytecode::word::arity(o.opcode).is_some())
        .fold(0_usize, |work, o| {
            work.saturating_add(evm_bytecode::word::evaluation_work(o.opcode))
        });
    if !owner.pay(
        crate::transform::ir_size(&owner.current().ir)
            .saturating_mul(8)
            .saturating_add(certification_work),
    ) {
        return;
    }
    let mut ir = owner.current().ir.clone();
    let mut projection_work = 0_usize;
    let Some(mut projection) = project(&ir, |work| {
        projection_work = projection_work.saturating_add(work);
        owner.pay(work)
    }) else {
        return;
    };
    // Certification reconstructs this deterministic proof and checks the actual
    // projection's bindings. Reserve both before validation, whose public API is
    // cancellation-only. This includes metered dominator/SCC convergence work.
    if !owner.pay(projection_work.saturating_mul(2)) {
        return;
    }
    if projection.program.record_count() > crate::MAX_LIFTED_RECORDS {
        owner.stop(
            Completion::Limited,
            DiagnosticCode::Limit,
            "lifted semantic record limit exceeded; Structured retained",
        );
        return;
    }
    let remap: BTreeMap<_, _> = projection
        .program
        .entity_ids()
        .map(|id| (id, owner.id()))
        .collect();
    remap_program(&mut projection.program, &remap);
    let parents: BTreeMap<EntityId, Vec<EntityId>> = projection
        .parents
        .into_iter()
        .filter_map(|(id, parents)| Some((*remap.get(&id)?, parents.into_iter().collect())))
        .collect();
    ir.lifted = Some(projection.program);
    let explanation = (
        "lift_semantic_expressions",
        "typed scoped EVM expressions, live simultaneous transfers and lazy exact-payload REVERT guards certified against unchanged primitive witness",
    );
    // Prepare and pay the mandatory base completely, including actual ancestry
    // traversal. Optional optimization cannot spend its publication reserve.
    let Some(base) = owner.prepare_publication(
        StageKind::Lifted,
        ir,
        explanation,
        parents,
        Vec::new(),
        Vec::new(),
    ) else {
        return;
    };
    if !base.passed() {
        owner.adopt_publication(base);
        return;
    }
    let result = owner.optional(allowance, |owner| {
        optional_sharing(owner, &base, projection_work, max_event_slots, explanation)
    });
    let (selected, refusal) = match result {
        Err(crate::artifact::OptionalStop::Stopped) => return,
        Err(reason) => (base, Some(reason.to_string())),
        Ok(Ok(Some(candidate))) => (candidate, None),
        Ok(Ok(None)) => (base, None),
        Ok(Err(reason)) if reason.optional() => (base, Some(reason.to_string())),
        Ok(Err(reason)) => {
            owner.stop(
                Completion::Partial,
                DiagnosticCode::Validation,
                &format!("invalid optional shared witness: {reason}; Structured retained"),
            );
            return;
        }
    };
    if owner.adopt_publication(selected)
        && let Some(reason) = refusal
    {
        owner.diagnostic(DiagnosticCode::OptimizationRefused, crate::Severity::Info,
            None, None, format!("optional shared optimization skipped: {reason}; validated base Lifted retained; coverage unchanged"));
    }
}

fn optional_sharing(
    owner: &mut Builder<impl FnMut(Progress)>,
    base: &crate::artifact::PreparedPublication,
    projection_work: usize,
    max_event_slots: usize,
    explanation: (&str, &str),
) -> Result<Option<crate::artifact::PreparedPublication>, shared::DiscoveryError> {
    use shared::DiscoveryError;
    let source = base.ir();
    let program = source
        .lifted
        .as_ref()
        .expect("prepared mandatory projection");
    // Source and semantic IDs share one artifact namespace; provisional shared
    // IDs are remapped only after a complete, useful candidate exists.
    let mut next = program
        .entity_ids()
        .map(|id| id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut plan = shared::discover(source, program, &mut next, max_event_slots, &mut |work| {
        owner.pay(work)
    })?;
    if plan.procedures.is_empty() && plan.invocations.is_empty() {
        return Ok(None);
    }
    if !owner.pay(base.candidate_clone_work()) {
        return Err(DiscoveryError::Payment);
    }
    let mut ir = source.clone();
    let mut parents = base.parents();
    let mut definitions = BTreeMap::new();
    for definition in &mut plan.procedures {
        let old = definition.id;
        definition.id = owner.id();
        definitions.insert(old, definition.id);
        parents.insert(definition.id, vec![definition.body]);
    }
    for invocation in &mut plan.invocations {
        invocation.id = owner.id();
        invocation.procedure = definitions[&invocation.procedure];
        parents.insert(invocation.id, vec![invocation.source]);
    }
    let program = ir.lifted.as_mut().expect("cloned mandatory projection");
    program.procedures = plan.procedures;
    program.invocations = plan.invocations;
    let mut admission_work = 0_usize;
    ir.lifted
        .as_ref()
        .expect("candidate projection")
        .validate_shared_sources(&ir, |work| {
            admission_work = admission_work.saturating_add(work);
            owner.pay(work)
        })
        .map_err(DiscoveryError::source)?;
    // Candidate publication repeats the deterministic base proof and direct
    // shared admission. Quote those exact measured passes before preparation.
    if !owner.pay(
        projection_work
            .saturating_mul(2)
            .saturating_add(admission_work.saturating_mul(2)),
    ) {
        return Err(DiscoveryError::Payment);
    }
    let Some(candidate) = owner.prepare_publication(
        StageKind::Lifted,
        ir,
        explanation,
        parents,
        Vec::new(),
        Vec::new(),
    ) else {
        return Err(DiscoveryError::Payment);
    };
    if !candidate.passed() {
        return Err(DiscoveryError::Publication);
    }
    Ok(Some(candidate))
}

struct Projection<'a> {
    program: LiftedProgram,
    parents: BTreeMap<EntityId, BTreeSet<EntityId>>,
    proof: aliases::Copies<'a>,
}
struct Projector<'a, P> {
    ir: &'a Ir,
    pay: P,
    result: Projection<'a>,
    next: u64,
    memo: BTreeMap<(EntityId, Kind), EntityId>,
    indices: BTreeMap<EntityId, usize>,
    bindings: BTreeMap<(EntityId, EntityId), EntityId>,
    optional_statements: BTreeSet<EntityId>,
    inline_weights: Vec<usize>,
}
fn project(ir: &Ir, mut pay: impl FnMut(usize) -> bool) -> Option<Projection<'_>> {
    let proof = aliases::Copies::build(ir, &mut pay)?;
    let mut p = Projector {
        ir,
        pay,
        result: Projection {
            program: LiftedProgram::default(),
            parents: BTreeMap::new(),
            proof,
        },
        next: 1,
        memo: BTreeMap::new(),
        indices: BTreeMap::new(),
        bindings: BTreeMap::new(),
        optional_statements: BTreeSet::new(),
        inline_weights: Vec::new(),
    };
    for b in &ir.blocks {
        if !(p.pay)(b.parameters.len().saturating_add(1)) {
            return None;
        }
        let mut statements = Vec::new();
        for (position, op) in b.operations.iter().enumerate() {
            if !(p.pay)(
                op.inputs
                    .len()
                    .saturating_add(op.outputs.len())
                    .saturating_add(1),
            ) {
                return None;
            }
            let fault = position + 1 == b.operations.len()
                && matches!(b.terminator, Terminator::Trap { .. });
            let pending = matches!(b.terminator, Terminator::Pending);
            let pure_word = op.effects.is_pure()
                && evm_bytecode::word::arity(op.opcode) == Some(op.inputs.len())
                && op.outputs.len() == 1
                && !fault
                && !pending;
            let push =
                matches!(op.opcode, 0x5f..=0x7f) && op.outputs.len() == 1 && !fault && !pending;
            let materialize = (pure_word || push)
                && op
                    .outputs
                    .iter()
                    .any(|v| p.result.proof.materialize.contains(v));
            if (pure_word || push) && !materialize {
                let first_expression = p.result.program.expressions.len();
                let args = op
                    .inputs
                    .iter()
                    .map(|v| p.word(b.id, *v))
                    .collect::<Option<Vec<_>>>()?;
                let result = if pure_word {
                    p.simplify(b.id, op.opcode, args.clone())?
                } else {
                    p.word(b.id, op.outputs[0])?
                };
                // Large pure DAGs keep one raw computation at its original
                // position. Later consumers use that exact SSA word identity.
                let binding = if pure_word && p.needs_named_binding(result) {
                    let id = p.id([op.id]);
                    statements.push(LiftedStatement {
                        id,
                        operation: op.id,
                        inputs: args,
                        outputs: op.outputs.clone(),
                    });
                    p.optional_statements.insert(op.id);
                    p.expression(b.id, Kind::Value(op.outputs[0]), [op.id, op.outputs[0]])
                } else {
                    result
                };
                let source: Vec<_> = std::iter::once(op.id)
                    .chain(op.inputs.iter().copied())
                    .chain(op.outputs.iter().copied())
                    .collect();
                // A rewrite may synthesize children besides its returned root
                // (for example a combined mask literal). Attribute every fresh
                // arena record to the raw instruction that introduced it. The
                // interval bounds work by new records; never traverse the DAG
                // or copy an existing expression's accumulated ancestry.
                let mut rewritten: Vec<_> = p.result.program.expressions[first_expression..]
                    .iter()
                    .map(|expression| expression.id)
                    .collect();
                rewritten.push(result);
                // A branch consumes the predicate under its numeric 0/1 cast.
                // Retain ancestry updates for reused roots and predicates too.
                if let Kind::WordFromPredicate(predicate) = p.kind(result).clone() {
                    rewritten.push(predicate);
                }
                rewritten.sort_unstable();
                rewritten.dedup();
                if !(p.pay)(
                    rewritten
                        .len()
                        .saturating_mul(source.len().saturating_add(1)),
                ) {
                    return None;
                }
                for expression in rewritten {
                    p.result
                        .parents
                        .entry(expression)
                        .or_default()
                        .extend(source.iter().copied());
                }
                p.bindings.insert((b.id, op.outputs[0]), binding);
            } else if materialize
                || !op.effects.is_pure()
                || fault
                || pending
                || !matches!(op.opcode, 0x50 | 0x56 | 0x57 | 0x5b | 0x80..=0x9f)
            {
                let inputs = op
                    .inputs
                    .iter()
                    .map(|v| p.word(b.id, *v))
                    .collect::<Option<Vec<_>>>()?;
                let id = p.id([op.id]);
                statements.push(LiftedStatement {
                    id,
                    operation: op.id,
                    inputs,
                    outputs: op.outputs.clone(),
                });
                if materialize {
                    p.optional_statements.insert(op.id);
                }
                for output in &op.outputs {
                    let expression = p.expression(b.id, Kind::Value(*output), [op.id, *output]);
                    p.bindings.insert((b.id, *output), expression);
                }
            }
        }
        let condition = match &b.terminator {
            Terminator::Branch { condition, .. } => {
                let value = p.word(b.id, *condition)?;
                Some(p.nonzero(b.id, value))
            }
            _ => None,
        };
        let target = match &b.terminator {
            Terminator::Jump {
                target,
                edges,
                unresolved,
                invalid_targets,
            } if edges.len() != 1 || *unresolved || !invalid_targets.is_empty() => {
                Some(p.word(b.id, *target)?)
            }
            Terminator::Branch {
                target,
                then_edges,
                unresolved,
                invalid_targets,
                ..
            } if then_edges.len() != 1 || *unresolved || !invalid_targets.is_empty() => {
                Some(p.word(b.id, *target)?)
            }
            _ => None,
        };
        let exit_values = match &b.terminator {
            Terminator::Exit { values, .. } => values
                .iter()
                .map(|v| p.word(b.id, *v))
                .collect::<Option<Vec<_>>>()?,
            _ => Vec::new(),
        };
        let id = p.id([b.id]);
        p.result.program.blocks.push(LiftedBlock {
            id,
            block: b.id,
            parameters: b.parameters.clone(),
            statements,
            condition,
            target,
            exit_values,
        });
    }
    // All raw RHSs are initially available; liveness selects matching positions
    // only after semantic consumers have been established in every block.
    for edge in &ir.edges {
        if !(p.pay)(edge.arguments.len().saturating_add(1)) {
            return None;
        }
        let arguments = edge
            .arguments
            .iter()
            .map(|v| p.word(edge.from, *v))
            .collect::<Option<Vec<_>>>()?;
        // Retain a transfer witness even when all its logical slots disappear.
        // Direct slot correspondences preserve source lookup without flattening
        // a transitive copy chain into every consumer expression's ancestry.
        let target = p.result.proof.blocks[&edge.to];
        if !(p.pay)(edge.arguments.len().saturating_mul(3).saturating_add(1)) {
            return None;
        }
        let parents = std::iter::once(edge.id)
            .chain(
                ir.blocks[target]
                    .parameters
                    .iter()
                    .zip(&edge.arguments)
                    .filter(|(parameter, _)| p.result.proof.aliases.contains_key(parameter))
                    .flat_map(|(parameter, argument)| {
                        [*parameter, *argument, p.result.proof.canonical(*parameter)]
                    }),
            )
            .collect::<Vec<_>>();
        let id = p.id(parents);
        p.result.program.transfers.push(LiftedTransfer {
            id,
            edge: edge.id,
            arguments,
        });
    }
    if let Some(ast) = &ir.structured {
        let mut pending = vec![&ast.root];
        while let Some(n) = pending.pop() {
            if !(p.pay)(1) {
                return None;
            }
            let condition = match &n.kind {
                StructuredKind::If { condition, .. } => Some(*condition),
                StructuredKind::While { condition, .. } => *condition,
                _ => None,
            };
            if let Some(value) = condition {
                let block = ir.blocks[p.result.proof.definitions.get(&value)?.block].id;
                let word = p.word(block, value)?;
                let condition = p.nonzero(block, word);
                let id = p.id([n.id, value]);
                p.result.program.controls.push(LiftedControl {
                    id,
                    node: n.id,
                    condition,
                    guard: None,
                });
            }
            children(n, &mut pending);
        }
    }
    p.prune()?;
    // Payload and guard metadata use the final high bodies/control and joint
    // liveness result. Every original construction/REVERT statement stays in
    // the arena as the lazy source, state dependency and fault witness.
    {
        let parents = &mut p.result.parents;
        let next = &mut p.next;
        payloads::discover(
            ir,
            &mut p.result.program,
            |source| {
                let id = EntityId(*next);
                *next += 1;
                parents.insert(id, source.into_iter().collect());
                id
            },
            &mut p.pay,
        )?;
    }
    guards::discover(ir, &mut p.result.program, &mut p.pay)?;
    for control in &p.result.program.controls {
        if let Some(guard) = &control.guard {
            if !(p.pay)(8) {
                return None;
            }
            p.result
                .parents
                .get_mut(&control.id)?
                .extend(guard.source_references());
        }
    }
    if !p
        .result
        .proof
        .check_program(ir, &p.result.program, &mut p.pay)?
    {
        return None;
    }
    if !validate::source_correspondences(&p.result, ir, &mut p.pay)? {
        return None;
    }
    // Canonical local IDs do not depend on discarded expressions. Publication
    // substitutes fresh artifact IDs; certification canonicalizes them again.
    let ids: BTreeMap<_, _> = p
        .result
        .program
        .entity_ids()
        .enumerate()
        .map(|(i, id)| (id, EntityId(i as u64 + 1)))
        .collect();
    remap_program(&mut p.result.program, &ids);
    p.result.parents = p
        .result
        .parents
        .into_iter()
        .filter_map(|(id, parents)| Some((*ids.get(&id)?, parents)))
        .collect();
    Some(p.result)
}
impl<P: FnMut(usize) -> bool> Projector<'_, P> {
    fn id(&mut self, parents: impl IntoIterator<Item = EntityId>) -> EntityId {
        let id = EntityId(self.next);
        self.next += 1;
        self.result
            .parents
            .insert(id, parents.into_iter().collect());
        id
    }
    fn expression(
        &mut self,
        block: EntityId,
        kind: Kind,
        parents: impl IntoIterator<Item = EntityId>,
    ) -> EntityId {
        if let Some(id) = self.memo.get(&(block, kind.clone())) {
            self.result.parents.entry(*id).or_default().extend(parents);
            return *id;
        }
        let weight = self.inline_weight(&kind);
        let id = self.id(parents);
        let index = self.result.program.expressions.len();
        self.result.program.expressions.push(LiftedExpression {
            id,
            block,
            ty: kind.ty(),
            kind: kind.clone(),
        });
        self.inline_weights.push(weight);
        self.indices.insert(id, index);
        self.memo.insert((block, kind), id);
        id
    }
    fn kind(&self, id: EntityId) -> &Kind {
        &self.result.program.expressions[self.indices[&id]].kind
    }
    fn word(&mut self, block: EntityId, value: EntityId) -> Option<EntityId> {
        let canonical = self.result.proof.canonical(value);
        if let Some(id) = self.bindings.get(&(block, value)) {
            return Some(*id);
        }
        // Only exact literal definitions are substituted. Numeric facts at open
        // boundaries never justify erasing an observation or a phi dependency.
        let kind = match self.result.proof.values.get(&canonical)?.kind {
            ValueKind::Constant(word) => Kind::Word(word),
            _ => Kind::Value(canonical),
        };
        // Memoization merges expression identity, not source occurrences. Each
        // alias contributes its own direct parent even if a prior use hit memo.
        let id = self.expression(block, kind, [value, canonical]);
        self.bindings.insert((block, value), id);
        Some(id)
    }
}
fn children<'a>(n: &'a StructuredNode, pending: &mut Vec<&'a StructuredNode>) {
    match &n.kind {
        StructuredKind::Sequence(nodes) => pending.extend(nodes.iter().rev()),
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            pending.push(else_branch);
            pending.push(then_branch);
        }
        StructuredKind::While { header, body, .. } => {
            pending.push(body);
            pending.push(header);
        }
        StructuredKind::Indirect { arms, .. } => pending.extend(arms.iter().rev().map(|a| &a.body)),
        _ => {}
    }
}
fn remap_program(program: &mut LiftedProgram, ids: &BTreeMap<EntityId, EntityId>) {
    let id = |old: &mut EntityId| *old = ids[old];
    for e in &mut program.expressions {
        id(&mut e.id);
        match &mut e.kind {
            Kind::WordFromPredicate(v) | Kind::NonZero(v) | Kind::Not(v) => id(v),
            Kind::Compare { left, right, .. } => {
                id(left);
                id(right);
            }
            Kind::WordOp { arguments, .. } => {
                for v in arguments {
                    id(v);
                }
            }
            _ => {}
        }
    }
    for b in &mut program.blocks {
        id(&mut b.id);
        for s in &mut b.statements {
            id(&mut s.id);
            for v in &mut s.inputs {
                id(v);
            }
        }
        if let Some(v) = &mut b.condition {
            id(v);
        }
        if let Some(v) = &mut b.target {
            id(v);
        }
        for v in &mut b.exit_values {
            id(v);
        }
    }
    for t in &mut program.transfers {
        id(&mut t.id);
        for v in &mut t.arguments {
            id(v);
        }
    }
    for payload in &mut program.payloads {
        id(&mut payload.id);
    }
    for revert in &mut program.reverts {
        id(&mut revert.id);
        id(&mut revert.payload);
    }
    for construction in &mut program.constructions {
        id(&mut construction.id);
        id(&mut construction.revert);
    }
    for failure in &mut program.failures {
        if let crate::LiftedGuardPayload::ErrorString(payload) = &mut failure.payload {
            id(payload);
        }
    }
    for p in &mut program.procedures {
        id(&mut p.id);
    }
    for i in &mut program.invocations {
        id(&mut i.id);
        id(&mut i.procedure);
    }
    for c in &mut program.controls {
        id(&mut c.id);
        id(&mut c.condition);
        if let Some(guard) = &mut c.guard
            && let crate::LiftedGuardPayload::ErrorString(payload) = &mut guard.payload
        {
            id(payload);
        }
    }
}
