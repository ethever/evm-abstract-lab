//! Preserve branch proofs in numeric views of conditional byte and slot roots.

use super::super::{TransferContext, errors};
use crate::{
    analysis::{DiagnosticKind, FrontierReason, MachinePayload},
    domain::{
        AbstractValue,
        relational::{QueryReason, RelationState, ScalarQuery},
        symbolic::{ExprId, ExprKind},
    },
    world::{GuardRefinementError, Snapshot},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeMap;

fn failure(error: GuardRefinementError<FrontierReason>) -> FrontierReason {
    match error {
        GuardRefinementError::Bytes(error) => errors::bytes(error),
        GuardRefinementError::Store(error) => errors::store(error),
        GuardRefinementError::Facts(reason) => FrontierReason::Relations(reason),
        GuardRefinementError::Proof(reason) => reason,
    }
}

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
    let return_length = payload.active().returndata.len().expression().cloned();
    if return_length
        .as_ref()
        .is_some_and(|expression| !context.budget.charge(expression.work_size()))
    {
        return Err(FrontierReason::Work);
    }
    let return_length_leaves =
        return_length.map_or_else(Default::default, |expression| expression.variables());
    // CALL output copies retain their length expressions in the byte operand
    // maps. A later RETURNDATASIZE guard must refine those same expressions,
    // before root evaluation can identify the untouched suffix. Refining only
    // ITE truth loses the stronger fact length == 1.
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
                let refinement = if matches!(expression.kind(), ExprKind::Fresh(_))
                    && return_length_leaves.contains(expression)
                {
                    // A joined CALL buffer may include an empty failure arm.
                    // Its length guard can imply a fresh length only through
                    // the selector. Query those retained scalar length leaves,
                    // never each cached byte, under the original state policy.
                    let mut identified = value.clone().with_expression(expression.clone());
                    let refinement = super::refine(&mut identified, relations, context);
                    if refinement == Ok(true) {
                        *value = value.clone().with_numeric(identified.numeric().clone());
                    }
                    refinement
                } else {
                    refine_expression(expression, value, relations, context)
                };
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
    let mut known = BTreeMap::new();
    let mut infeasible = false;
    let mut proof = |condition: &ExprId| -> Result<Option<bool>, FrontierReason> {
        if let Some(truth) = known.get(condition) {
            return Ok(*truth);
        }
        // A nonzero Word256 need not be 1. Ask for the unique boolean ISZERO
        // result rather than assigning a fabricated concrete selector value.
        let zero = ExprId::operation(opcode::ISZERO, std::slice::from_ref(condition), limits)
            .map_err(errors::expression)?;
        let mut value = AbstractValue::unsigned_range(U256::ZERO, U256::ONE)
            .expect("nonempty boolean range")
            .with_expression(zero);
        let truth = match super::refine(&mut value, relations, context) {
            Ok(false) => {
                infeasible = true;
                None
            }
            Ok(true) => value.singleton().map(|zero| zero == U256::ZERO),
            Err(FrontierReason::Relations(reason)) if reason != QueryReason::Cancelled => {
                diagnostics.push((pc, DiagnosticKind::RelationalPrecisionLost(reason)));
                None
            }
            Err(reason) => return Err(reason),
        };
        known.insert(condition.clone(), truth);
        Ok(truth)
    };
    if !payload
        .store
        .refine_guards(domain, limits, &mut proof)
        .map_err(failure)?
    {
        return Ok(false);
    }
    for frame in payload.call_stack.iter_mut() {
        for bytes in [
            &mut frame.memory,
            &mut frame.calldata,
            &mut frame.returndata,
        ] {
            if !bytes
                .refine_guards(domain, limits, &mut proof)
                .map_err(failure)?
            {
                return Ok(false);
            }
        }
        let mut saved = frame.saved_store.state().clone();
        if !saved
            .refine_guards(domain, limits, &mut proof)
            .map_err(failure)?
        {
            return Ok(false);
        }
        frame.saved_store = Snapshot::from_state(saved);
    }
    Ok(!infeasible)
}
