//! Bounded projection of active branch values under one frozen relation state.
//!
//! Original per-value proofs win before a common conjunction is prepared.
//! Common proofs never enter either cache keyed by the weaker single-value Fi.

use super::{
    PreparedRefinement, TransferContext, charge, finish_refine, native_refine, prepare_refine,
};
use crate::{
    analysis::{DiagnosticKind, FrontierReason, MachinePayload},
    domain::{
        AbstractValue,
        relational::{BudgetedValueQuery, QueryReason, RelationState, ValueQuery},
        symbolic::ExprId,
    },
};

const MAX_VALUES: usize = embedded_smt::MAX_UNIQUE_TARGETS;

#[derive(Clone, Copy)]
enum Position {
    Stack(usize),
    CallValue,
}

impl Position {
    fn value(self, payload: &mut MachinePayload) -> &mut AbstractValue {
        match self {
            Self::Stack(index) => &mut payload.active_mut().stack[index],
            Self::CallValue => &mut payload.active_mut().call_value,
        }
    }
}

struct Pending {
    position: Position,
    expression: ExprId,
}

/// Optional precision failures retain the current value. Real ledger failure
/// and cancellation propagate to branch's existing frontier boundary.
fn optional(
    result: Result<bool, FrontierReason>,
    diagnostics: &mut Vec<(usize, DiagnosticKind)>,
    pc: usize,
) -> Result<bool, FrontierReason> {
    match result {
        Err(FrontierReason::Relations(reason)) if reason != QueryReason::Cancelled => {
            diagnostics.push((pc, DiagnosticKind::RelationalPrecisionLost(reason)));
            Ok(true)
        }
        result => result,
    }
}

/// A rejected assumption supplies no new joint context. Keep the exact
/// original sequential oracle for that edge, including all optional failures.
pub(super) fn individual(
    payload: &mut MachinePayload,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
    diagnostics: &mut Vec<(usize, DiagnosticKind)>,
    pc: usize,
) -> Result<bool, FrontierReason> {
    let frame = payload.active_mut();
    for value in frame.stack.iter_mut().chain([&mut frame.call_value]) {
        if !optional(super::refine(value, relations, context), diagnostics, pc)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn refine(
    payload: &mut MachinePayload,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
    diagnostics: &mut Vec<(usize, DiagnosticKind)>,
    pc: usize,
) -> Result<bool, FrontierReason> {
    let limits = context.config.analysis.relations;
    let mut pending = Vec::new();
    let stack_len = payload.active().stack.len();
    for index in 0..=stack_len {
        let position = if index == stack_len {
            Position::CallValue
        } else {
            Position::Stack(index)
        };
        let prepared = prepare_refine(position.value(payload), relations, context);
        let expression = match prepared {
            Ok(PreparedRefinement::Done(false)) => return Ok(false),
            Ok(PreparedRefinement::Done(true)) => continue,
            Ok(PreparedRefinement::Pending(expression)) => expression,
            Err(reason) => {
                optional(Err(reason), diagnostics, pc)?;
                continue;
            }
        };

        // Pay complete old Fi preparation before a lookup-only probe. It has
        // no native obligation; the old oracle still prepays 2r on fallback.
        // Keep the existing optional maintenance quarter allowance unchanged.
        if !charge(relations, &expression, 0, context) {
            return Err(FrontierReason::Work);
        }
        let proof = relations.probe_unique_value_cached(
            &expression,
            &limits,
            &mut context
                .budget
                .smt_cache_query(limits.cache_scope())
                .with_allowance((limits.rlimit as usize).saturating_mul(2) / 4),
        );
        match proof {
            Ok(Some(answer)) => {
                // Only this proof of the original Fi may populate QueryCache.
                context
                    .query_cache
                    .remember(relations, &expression, &limits, &answer);
                if !optional(
                    finish_refine(
                        position.value(payload),
                        &expression,
                        relations,
                        context,
                        answer,
                    ),
                    diagnostics,
                    pc,
                )? {
                    return Ok(false);
                }
                continue;
            }
            Ok(None) => {}
            Err(QueryReason::Cancelled) => {
                return Err(FrontierReason::Relations(QueryReason::Cancelled));
            }
            Err(_) => {
                // An encoding failure is not a license to drop old precision.
                // Finish earlier pending values before this individual retry.
                if !finish_block(payload, relations, context, diagnostics, pc, &pending)? {
                    return Ok(false);
                }
                pending.clear();
                if !optional(
                    fallback_refine(position.value(payload), &expression, relations, context),
                    diagnostics,
                    pc,
                )? {
                    return Ok(false);
                }
                continue;
            }
        }

        // Reserve bounded retention, later ExprId comparisons and clones before
        // growing a block. Aliases keep separate positions and numeric bounds.
        let retention = expression
            .work_size()
            .saturating_mul(MAX_VALUES.saturating_mul(5).saturating_add(4))
            .saturating_add(MAX_VALUES);
        if !context.budget.charge(retention) {
            return Err(FrontierReason::Work);
        }
        pending.push(Pending {
            position,
            expression,
        });
        if pending.len() == MAX_VALUES
            && !finish_block(payload, relations, context, diagnostics, pc, &pending)?
        {
            return Ok(false);
        }
        if pending.len() == MAX_VALUES {
            pending.clear();
        }
    }
    finish_block(payload, relations, context, diagnostics, pc, &pending)
}

fn finish_block(
    payload: &mut MachinePayload,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
    diagnostics: &mut Vec<(usize, DiagnosticKind)>,
    pc: usize,
    pending: &[Pending],
) -> Result<bool, FrontierReason> {
    if pending.is_empty() {
        return Ok(true);
    }
    // No target collection grows beyond this paid, fixed-size block.
    let mut expressions = Vec::new();
    let mut indices = Vec::new();
    for target in pending {
        let index = expressions
            .iter()
            .position(|expression| expression == &target.expression)
            .unwrap_or_else(|| {
                let index = expressions.len();
                expressions.push(target.expression.clone());
                index
            });
        indices.push(index);
    }
    let answers = if expressions.len() >= 2 {
        let cost = expressions
            .iter()
            .fold(relations.work_size(), |cost, expression| {
                cost.saturating_add(expression.work_size())
            })
            .saturating_add(context.config.analysis.relations.rlimit as usize);
        let mut budget = context.budget.query_budget(cost);
        match relations.unique_many_budgeted(
            &expressions,
            &context.config.analysis.relations,
            &mut budget,
        ) {
            Ok(answers) => Some(answers),
            Err(QueryReason::Cancelled) => {
                return Err(FrontierReason::Relations(QueryReason::Cancelled));
            }
            Err(_) => None,
        }
    } else {
        None
    };
    if answers.as_ref().is_some_and(|answers| {
        answers
            .iter()
            .any(|answer| matches!(answer, BudgetedValueQuery::Work))
    }) {
        return Err(FrontierReason::Work);
    }
    if answers.as_ref().is_some_and(|answers| {
        answers.iter().any(|answer| {
            matches!(
                answer,
                BudgetedValueQuery::Value(ValueQuery::Unknown(QueryReason::Cancelled))
            )
        })
    }) {
        return Err(FrontierReason::Relations(QueryReason::Cancelled));
    }
    // Retry only undecided values, in original order, before installing joint
    // narrowing. In particular a known Multiple must not prune a finite bound
    // before a later unresolved value reaches its old Work/Cancel boundary.
    for (target, index) in pending.iter().zip(&indices) {
        if answers.as_ref().is_some_and(|answers| {
            matches!(&answers[*index], BudgetedValueQuery::Value(answer)
                if !matches!(answer, ValueQuery::Unknown(_)))
        }) {
            continue;
        }
        if !optional(
            fallback_refine(
                target.position.value(payload),
                &target.expression,
                relations,
                context,
            ),
            diagnostics,
            pc,
        )? {
            return Ok(false);
        }
    }
    for (target, index) in pending.iter().zip(&indices) {
        if let Some(BudgetedValueQuery::Value(answer)) =
            answers.as_ref().map(|answers| &answers[*index])
            && !matches!(answer, ValueQuery::Unknown(_))
            && !optional(
                // Joint F may be stronger than Fi: apply, but never cache it.
                finish_refine(
                    target.position.value(payload),
                    &target.expression,
                    relations,
                    context,
                    answer.clone(),
                ),
                diagnostics,
                pc,
            )?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn fallback_refine(
    value: &mut AbstractValue,
    expression: &ExprId,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
) -> Result<bool, FrontierReason> {
    // An earlier fallback can have proved this same original Fi since the
    // local phase ran. Recheck only that cache, with its real comparison fee;
    // joint proofs were never remembered there.
    let work = context.query_cache.work(relations, expression);
    if work == usize::MAX || !context.budget.charge(work) {
        return Err(FrontierReason::Work);
    }
    let limits = context.config.analysis.relations;
    if let Some(answer) = context.query_cache.get(relations, expression, &limits) {
        finish_refine(value, expression, relations, context, answer)
    } else {
        native_refine(value, expression, relations, context)
    }
}
