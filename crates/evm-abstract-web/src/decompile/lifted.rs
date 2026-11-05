//! Presentation and navigation over the model's flat semantic projection.
//! All expression and liveness decisions belong to the shared model.

use super::{details::Details, index::StageIndex, model::EntityKind, structured};
use evm_abstract_protocol::decompile_model::{
    EntityId, EntityRef, LiftedExpressionKind, LiftedProgram, LiftedRenderIndex, LiftedStatement,
    LiftedTransfer, Stage,
};

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
}

pub(super) fn expression(renderer: &LiftedRenderIndex<'_>, id: EntityId) -> String {
    renderer
        .render_expression(id, |_| true)
        .unwrap_or_else(|error| format!("unavailable_expression(x{}: {error})", id.0))
}

pub(super) fn statement(renderer: &LiftedRenderIndex<'_>, statement: &LiftedStatement) -> String {
    renderer
        .render_statement(statement, |_| true)
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
) -> Option<Details> {
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
        lines.push(format!(
            "Control h{} · ast{}: {}",
            id.0,
            control.node.0,
            expression(renderer, control.condition)
        ));
        references.extend([control.node, control.condition]);
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
