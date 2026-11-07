//! Presentation and navigation over the model's flat semantic projection.
//! All expression and liveness decisions belong to the shared model.

use super::{details::Details, index::StageIndex, model::EntityKind, structured};
use evm_abstract_protocol::decompile_model::{
    EntityId, EntityRef, LiftedConstruction, LiftedExpressionKind, LiftedGuardPayload,
    LiftedIfPlan, LiftedIfSuffix, LiftedProgram, LiftedRenderIndex, LiftedStatement,
    LiftedTransfer, Stage,
};

pub(super) mod shared;
pub(super) mod work;

pub(super) fn visit(program: &LiftedProgram, visitor: &mut impl FnMut(EntityId, EntityKind)) {
    for expression in &program.expressions {
        visitor(expression.id, EntityKind::LiftedExpression);
    }
    for block in &program.blocks {
        visitor(block.id, EntityKind::LiftedBlock);
        for statement in &block.statements {
            visitor(statement.id, EntityKind::LiftedStatement);
        }
    }
    for transfer in &program.transfers {
        visitor(transfer.id, EntityKind::LiftedTransfer);
    }
    for control in &program.controls {
        visitor(control.id, EntityKind::LiftedControl);
    }
    for payload in &program.payloads {
        visitor(payload.id, EntityKind::LiftedPayload);
    }
    for revert in &program.reverts {
        visitor(revert.id, EntityKind::LiftedRevert);
    }
    for construction in &program.constructions {
        visitor(construction.id, EntityKind::LiftedConstruction);
    }
    for procedure in &program.procedures {
        visitor(procedure.id, EntityKind::LiftedProcedure);
    }
    for invocation in &program.invocations {
        visitor(invocation.id, EntityKind::LiftedInvocation);
    }
}

pub(super) fn expression(renderer: &LiftedRenderIndex<'_>, id: EntityId) -> String {
    renderer
        .render_expression(id, |_| true)
        .unwrap_or_else(|error| format!("unavailable_expression(x{}: {error})", id.0))
}

pub(super) fn statement(renderer: &LiftedRenderIndex<'_>, statement: &LiftedStatement) -> String {
    statement_with_dependencies(renderer, statement, true)
}

pub(super) fn statement_with_dependencies(
    renderer: &LiftedRenderIndex<'_>,
    statement: &LiftedStatement,
    show_state_dependencies: bool,
) -> String {
    renderer
        .render_statement_with_dependencies(statement, show_state_dependencies, |_| true)
        .unwrap_or_else(|error| format!("unavailable_statement(h{}: {error});", statement.id.0))
}

pub(super) fn transfer(
    renderer: &LiftedRenderIndex<'_>,
    transfer: &LiftedTransfer,
) -> Option<String> {
    renderer
        .render_transfer(transfer, |_| true)
        .unwrap_or_else(|error| {
            Some(format!(
                "unavailable_live_transfer(h{}: {error});",
                transfer.id.0
            ))
        })
}

pub(super) fn details(
    stage: &Stage,
    index: &StageIndex,
    id: EntityId,
    renderer: &LiftedRenderIndex<'_>,
    work: &work::Work,
) -> Option<Details> {
    if let Some(details) = shared::details(stage, index, id, work) {
        return Some(details);
    }
    let mut lines = Vec::new();
    let mut references = Vec::new();
    if let Some(definition) = index.expression(&stage.ir, id) {
        lines.push(format!(
            "Expression x{} · {:?}: {}",
            id.0,
            definition.ty,
            expression(renderer, id)
        ));
        lines.push(format!("Typed expression: {:?}", definition.kind));
        lines.push(format!("Evaluation block b{}", definition.block.0));
        references.push(definition.block);
        match &definition.kind {
            LiftedExpressionKind::Word(_) | LiftedExpressionKind::Predicate(_) => {}
            LiftedExpressionKind::Value(value)
            | LiftedExpressionKind::WordFromPredicate(value)
            | LiftedExpressionKind::NonZero(value)
            | LiftedExpressionKind::Not(value) => references.push(*value),
            LiftedExpressionKind::Compare { left, right, .. } => {
                references.extend([*left, *right]);
            }
            LiftedExpressionKind::WordOp { arguments, .. } => {
                references.extend(arguments.iter().copied());
            }
        }
    } else if let Some(payload) = index.payload(&stage.ir, id) {
        lines.push(format!(
            "{} Error(string) payload h{} · {} ABI bytes",
            if renderer.payload(id).is_some() {
                "Certified"
            } else {
                "Unadmitted"
            },
            id.0,
            payload.bytes.len(),
        ));
        lines.push(format!("Exact UTF-8 message: {:?}", payload.message));
        lines.push(format!(
            "Canonical ABI bytes: 0x{}",
            payload
                .bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ));
    } else if let Some(revert) = index.revert(&stage.ir, id) {
        lines.push(format!(
            "{} Error(string) REVERT h{} · b{} · o{}",
            if renderer.revert(revert.block).is_some() {
                "Certified"
            } else {
                "Unadmitted"
            },
            id.0,
            revert.block.0,
            revert.operation.0,
        ));
        lines.push(format!(
            "Terminal memory m{} · offset v{} · size v{} · payload h{}",
            revert.memory.0, revert.offset.0, revert.size.0, revert.payload.0,
        ));
        lines.push(format!(
            "Ordered byte-write witnesses: {}",
            revert
                .writes
                .iter()
                .map(|operation| format!("o{}", operation.0))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        references.extend([
            revert.block,
            revert.operation,
            revert.memory,
            revert.offset,
            revert.size,
            revert.payload,
        ]);
        references.extend(revert.writes.iter().copied());
        if let Some(construction) = renderer.construction(revert.block) {
            references.push(construction.id);
        } else {
            lines.push("Original construction proof unavailable; original memory operations and raw REVERT are retained.".into());
        }
    } else if let Some(construction) = index.construction(&stage.ir, id) {
        construction_details(
            stage,
            index,
            renderer,
            construction,
            &mut lines,
            &mut references,
        );
    } else if let Some(block) = index.lifted_block(&stage.ir, id) {
        lines.push(format!(
            "High-level block h{} → b{} · live parameters ({}) · {} statements",
            id.0,
            block.block.0,
            structured::values(&block.parameters),
            block.statements.len()
        ));
        references.push(block.block);
        references.extend(block.parameters.iter().copied());
        references.extend(block.statements.iter().map(|statement| statement.id));
        references.extend(block.condition.iter().chain(&block.target).copied());
        references.extend(block.exit_values.iter().copied());
        if let Some(source) = index.block(&stage.ir, block.block) {
            references.extend(source.terminator.edges());
        }
        if let Some(revert) = renderer.revert(block.block) {
            references.push(revert.id);
            if let Some(construction) = renderer.construction(block.block) {
                references.push(construction.id);
            } else {
                lines.push("Payload bytes are certified at REVERT; construction proof unavailable. Raw memory operations and REVERT are retained.".into());
            }
        }
        if let Some(condition) = block.condition {
            lines.push(format!("Predicate: {}", expression(renderer, condition)));
        }
        if let Some(target) = block.target {
            lines.push(format!("Indirect target: {}", expression(renderer, target)));
        }
    } else if let Some(statement) = index.statement(&stage.ir, id) {
        lines.push(format!(
            "Statement h{} · o{}: {}",
            id.0,
            statement.operation.0,
            self::statement(renderer, statement)
        ));
        references.push(statement.operation);
        references.extend(statement.inputs.iter().chain(&statement.outputs).copied());
        if let Some(op) = index.operation(&stage.ir, statement.operation) {
            lines.push(format!(
                "{}: {:?}",
                if structured::fault_reason(op, &stage.ir, index).is_some() {
                    "Declared primitive effects (not completed)"
                } else {
                    "Primitive effects"
                },
                op.effects
            ));
            references.extend(
                [
                    op.memory_input,
                    op.memory_output,
                    op.effect_input,
                    op.effect_output,
                ]
                .into_iter()
                .flatten(),
            );
        }
    } else if let Some(transfer) = index.lifted_transfer(&stage.ir, id) {
        lines.push(format!(
            "Live transfer h{} · e{}: {}",
            id.0,
            transfer.edge.0,
            self::transfer(renderer, transfer).unwrap_or_else(|| "No live word arguments".into())
        ));
        references.push(transfer.edge);
        references.extend(transfer.arguments.iter().copied());
        if let Some(edge) = index.edge(&stage.ir, transfer.edge) {
            references.extend([edge.from, edge.to, edge.memory, edge.effect]);
            if let Some(target) = index.source_block(&stage.ir, edge.to) {
                references.push(target.id);
                references.extend(target.parameters.iter().copied());
            }
        }
    } else {
        let control = index.control(&stage.ir, id)?;
        references.extend([control.node, control.condition]);
        let canonical = index.shared.control_plan(control.id);
        let selected = if canonical.is_some() {
            LiftedIfPlan::Original
        } else {
            stage
                .ir
                .structured
                .as_ref()
                .map_or(LiftedIfPlan::Original, |source| {
                    renderer.if_plan(control.node, source.root.id)
                })
        };
        let selected_header = if let Some(canonical) = canonical {
            references.extend(canonical.references.iter().copied());
            canonical.header.clone()
        } else {
            match &selected {
                LiftedIfPlan::Require(plan) => {
                    references.extend([plan.failure_node(), plan.success_node()]);
                    renderer.render_require(plan, |_| true).ok()
                }
                LiftedIfPlan::FailureFirst(plan) => {
                    references.extend([plan.failure_node(), plan.success_node()]);
                    if let Some(suffix) = renderer.failure_first_suffix(plan) {
                        references.extend(suffix.iter().map(|node| node.id));
                    }
                    renderer
                        .render_failure_first_condition(plan, |_| true)
                        .ok()
                        .map(|condition| format!("if ({condition}) {{"))
                }
                LiftedIfPlan::Original => None,
            }
        };
        if let Some(guard) = &control.guard {
            let admitted = canonical
                .map_or(matches!(&selected, LiftedIfPlan::Require(_)), |plan| {
                    plan.require
                });
            let kind = match guard.payload {
                LiftedGuardPayload::EmptyRevert => "empty-REVERT",
                LiftedGuardPayload::ErrorString(payload) => {
                    references.push(payload);
                    "Error(string)"
                }
            };
            lines.push(format!(
                "{} {kind} guard h{} · ast{}: {}",
                if admitted { "Certified" } else { "Unadmitted" },
                id.0,
                control.node.0,
                selected_header.unwrap_or_else(|| "original source If retained".into())
            ));
            lines.push(format!(
                "Predicate: {} · successful source truth: {}",
                expression(renderer, control.condition),
                if guard.pass_nonzero {
                    "nonzero"
                } else {
                    "zero"
                }
            ));
            lines.push(format!(
                "Pass: e{} → ast{} · failure: e{} → ast{} · payload: {:?}",
                guard.pass_edge.0,
                guard.pass_node.0,
                guard.failure_edge.0,
                guard.failure_node.0,
                guard.payload,
            ));
            references.extend([
                guard.block,
                guard.pass_edge,
                guard.pass_node,
                guard.failure_edge,
                guard.failure_node,
            ]);
            if let Some(edge) = index.edge(&stage.ir, guard.pass_edge) {
                references.push(edge.to);
            }
            if let Some(edge) = index.edge(&stage.ir, guard.failure_edge) {
                references.push(edge.to);
                if let Some(failure) = renderer.failure(edge.to) {
                    lines.push(format!(
                        "Shared {}-failure proof: b{} · decreasing rank {}",
                        if matches!(guard.payload, LiftedGuardPayload::EmptyRevert) {
                            "empty"
                        } else {
                            "error"
                        },
                        failure.block.0,
                        failure.rank
                    ));
                }
                if let Some(construction) = renderer.construction(edge.to) {
                    references.push(construction.id);
                    references.push(construction.revert);
                }
            }
            if let LiftedGuardPayload::ErrorString(payload) = guard.payload {
                if let Some(payload) = renderer.payload(payload) {
                    lines.push(format!("Exact UTF-8 message: {:?}", payload.message));
                }
                lines.push("Lazy failure construction retains original memory expansion, operation order and exceptional faults; evaluated only on the original failure route.".into());
            }
            if let Some(continuation) = &guard.continuation {
                lines.push(format!(
                    "Failure continuation: ast{} children {}..{} · pass falls through: {} · {}",
                    continuation.sequence.0,
                    continuation.start,
                    continuation.end,
                    continuation.pass_falls_through,
                    match canonical.map_or(selected.suffix(), |plan| plan.suffix) {
                        LiftedIfSuffix::Absorb { .. } => "absorbed by require",
                        LiftedIfSuffix::Move { .. } => "moved into explicit failed arm",
                        LiftedIfSuffix::Keep => "retained after pass",
                    },
                ));
                references.push(continuation.sequence);
            }
        } else {
            lines.push(selected_header.map_or_else(
                || {
                    format!(
                        "Control h{} · ast{}: {}",
                        id.0,
                        control.node.0,
                        expression(renderer, control.condition)
                    )
                },
                |header| {
                    format!(
                        "Failure-first If h{} · ast{}: {header}",
                        id.0, control.node.0
                    )
                },
            ));
        }
    }
    Some(Details {
        lines,
        references: references
            .into_iter()
            .map(|entity| EntityRef {
                stage: stage.id,
                entity,
            })
            .collect(),
    })
}

fn construction_details(
    stage: &Stage,
    index: &StageIndex,
    renderer: &LiftedRenderIndex<'_>,
    construction: &LiftedConstruction,
    lines: &mut Vec<String>,
    references: &mut Vec<EntityId>,
) {
    let admitted = renderer.construction(construction.block) == Some(construction);
    lines.push(format!(
        "{} lazy failure construction h{} · b{} · statements {}..{}",
        if admitted { "Certified" } else { "Unadmitted" },
        construction.id.0,
        construction.block.0,
        construction.start,
        construction.end,
    ));
    if admitted {
        lines.push("Original operations, memory expansion and exceptional faults remain explicit on the original failure route.".into());
    } else {
        lines.push(
            "Source proof unavailable; original construction and raw REVERT are retained.".into(),
        );
    }
    references.extend([construction.block, construction.revert]);
    if let Some(block) = index.source_block(&stage.ir, construction.block)
        && let Some(statements) = block.statements.get(construction.start..construction.end)
    {
        for statement in statements {
            references.extend([statement.id, statement.operation]);
        }
    }
}
