//! Preserve branch proofs in numeric views of conditional byte and slot roots.

mod guards;
mod length;

use super::super::TransferContext;
use crate::{
    analysis::{DiagnosticKind, FrontierReason, MachinePayload},
    domain::{
        AbstractValue,
        relational::{QueryReason, RelationState, ScalarQuery},
        symbolic::{ExprId, ExprKind},
    },
};

fn refine_operand(
    value: &mut AbstractValue,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
) -> Result<bool, FrontierReason> {
    let Some(expression) = value.expression().cloned() else {
        return Ok(true);
    };
    refine_expression(&expression, value, relations, context)
}

fn refine_expression(
    expression: &ExprId,
    value: &mut AbstractValue,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
) -> Result<bool, FrontierReason> {
    if value.singleton().is_some() || relations.is_empty() {
        return Ok(true);
    }
    if !context
        .budget
        .charge(relations.work_size().saturating_add(expression.work_size()))
    {
        return Err(FrontierReason::Work);
    }
    if !relations.relevant(expression) {
        return Ok(true);
    }
    if !context
        .budget
        .charge(context.domain.refinement_work(value, relations.len()))
    {
        return Err(FrontierReason::Work);
    }
    // Metadata refinement imports local proved facts; it does not launch a
    // separate native query for every cached byte or array operand.
    match relations.refine_numeric(
        expression,
        value.numeric(),
        context.domain,
        &context.config.analysis.relations,
    ) {
        ScalarQuery::Refined { numeric, .. } => {
            *value = value.clone().with_numeric(*numeric);
            Ok(true)
        }
        ScalarQuery::Infeasible => Ok(false),
        ScalarQuery::Unchanged | ScalarQuery::Unknown(QueryReason::ScalarFactLimit) => Ok(true),
        ScalarQuery::Unknown(reason) => Err(FrontierReason::Relations(reason)),
    }
}

pub(super) fn refine(
    payload: &mut MachinePayload,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
    diagnostics: &mut Vec<(usize, DiagnosticKind)>,
    pc: usize,
) -> Result<bool, FrontierReason> {
    // Pay for metadata traversal and checkpoint copies before doing either.
    // World refinement visits retained nodes, never the logical COPY length.
    if !context.budget.charge(payload.work_size().saturating_mul(2)) {
        return Err(FrontierReason::Work);
    }
    let domain = context.domain;
    let limits = context.config.analysis.relations.expressions();
    let mut failed = None;
    for frame in payload.call_stack.iter_mut() {
        for value in [&mut frame.address_operand, &mut frame.caller_operand]
            .into_iter()
            .flatten()
        {
            match refine_operand(value, relations, context) {
                Ok(false) => return Ok(false),
                Ok(true) => {}
                Err(FrontierReason::Relations(reason)) if reason != QueryReason::Cancelled => {
                    diagnostics.push((pc, DiagnosticKind::RelationalPrecisionLost(reason)));
                }
                Err(reason) => return Err(reason),
            }
        }
        for bytes in [
            &mut frame.memory,
            &mut frame.calldata,
            &mut frame.returndata,
        ] {
            bytes.update_values(&mut |value| {
                if failed.is_some()
                    || value.expression().is_some_and(|expression| {
                        matches!(
                            expression.kind(),
                            ExprKind::ByteRead { .. }
                                | ExprKind::WordRead { .. }
                                | ExprKind::Keccak { .. }
                        )
                    })
                {
                    return;
                }
                match refine_operand(value, relations, context) {
                    Ok(false) => failed = Some(Ok(false)),
                    Ok(true) => {}
                    Err(FrontierReason::Relations(reason)) if reason != QueryReason::Cancelled => {
                        diagnostics.push((pc, DiagnosticKind::RelationalPrecisionLost(reason)));
                    }
                    Err(reason) => failed = Some(Err(reason)),
                }
            });
            if let Some(failed) = failed.take() {
                return failed;
            }
            // Operand values deliberately carry no runtime identity; their
            // retained expression keys identify the facts to import. Visiting
            // values alone misses a copied length shared with RETURNDATASIZE.
            bytes.refine_operands(&mut |expression, value| {
                if failed.is_some() {
                    return;
                }
                let refinement = refine_expression(expression, value, relations, context);
                match refinement {
                    Ok(false) => failed = Some(Ok(false)),
                    Ok(true) => {}
                    Err(FrontierReason::Relations(reason)) if reason != QueryReason::Cancelled => {
                        diagnostics.push((pc, DiagnosticKind::RelationalPrecisionLost(reason)));
                    }
                    Err(reason) => failed = Some(Err(reason)),
                }
            });
            if let Some(failed) = failed.take() {
                return failed;
            }
        }
    }
    length::refine(payload, relations, context)?;
    guards::refine(
        payload,
        relations,
        context,
        diagnostics,
        pc,
        (domain, limits),
    )
}
