//! Freeze a bounded guard plan before projecting any wrapper. Only the actual
//! ISZERO targets use intrinsic Boolean preparation; unplanned or undecided
//! guards retain the original serial callback and its shared `known` map.

use super::super::super::errors;
use super::super::{
    PreparationKind, PreparedRefinement, TransferContext, charge, finish_refine,
    prepare_refine_mode,
};
use crate::{
    analysis::{DiagnosticKind, FrontierReason, MachinePayload},
    domain::{
        AbstractValue, Domain,
        relational::{BudgetedValueQuery, QueryReason, RelationState, ValueQuery},
        symbolic::{ExprId, ExprLimits},
    },
    world::{GuardRefinementError, Snapshot},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::{BTreeMap, BTreeSet};

// Every target is Word256: this also bounds the common group to 4096 bits.
const MAX_GUARDS: usize = embedded_smt::MAX_UNIQUE_TARGETS;

struct Pending {
    condition: ExprId,
    value: AbstractValue,
    expression: ExprId,
}

#[derive(Default)]
struct Proofs {
    known: BTreeMap<ExprId, Option<bool>>,
    infeasible: bool,
}

fn failure(error: GuardRefinementError<FrontierReason>) -> FrontierReason {
    match error {
        GuardRefinementError::Bytes(error) => errors::bytes(error),
        GuardRefinementError::Store(error) => errors::store(error),
        GuardRefinementError::Facts(reason) => FrontierReason::Relations(reason),
        GuardRefinementError::Proof(reason) => reason,
    }
}

fn boolean(condition: &ExprId, limits: ExprLimits) -> Result<AbstractValue, FrontierReason> {
    let zero = ExprId::operation(opcode::ISZERO, std::slice::from_ref(condition), limits)
        .map_err(errors::expression)?;
    Ok(AbstractValue::unsigned_range(U256::ZERO, U256::ONE)
        .expect("nonempty boolean range")
        .with_expression(zero))
}

fn gather(
    payload: &MachinePayload,
    context: &mut TransferContext<'_>,
) -> Result<Vec<ExprId>, FrontierReason> {
    // Pay only for wrappers actually collected. Once the bounded plan fills,
    // the original apply callback handles the remaining wrappers in order.
    let limits = context.config.analysis.relations.expressions();
    let mut planned = Vec::new();
    if collect(&mut planned, payload.store.work_size(), context, || {
        payload
            .store
            .guard_conditions(limits)
            .map_err(errors::store)
    })? {
        return Ok(planned);
    }
    for frame in payload.call_stack.iter() {
        for bytes in [&frame.memory, &frame.calldata, &frame.returndata] {
            if collect(&mut planned, bytes.work_size(), context, || {
                bytes.guard_conditions(limits).map_err(errors::bytes)
            })? {
                return Ok(planned);
            }
        }
        if collect(
            &mut planned,
            frame.saved_store.state().work_size(),
            context,
            || {
                frame
                    .saved_store
                    .state()
                    .guard_conditions(limits)
                    .map_err(errors::store)
            },
        )? {
            return Ok(planned);
        }
    }
    Ok(planned)
}

fn collect(
    planned: &mut Vec<ExprId>,
    work: usize,
    context: &mut TransferContext<'_>,
    conditions: impl FnOnce() -> Result<BTreeSet<ExprId>, FrontierReason>,
) -> Result<bool, FrontierReason> {
    if !context.budget.charge(work.saturating_mul(2)) {
        return Err(FrontierReason::Work);
    }
    for condition in conditions()? {
        // Prepay bounded retention, comparisons, Boolean construction and
        // the later target/alias vectors, as on the active-value joint path.
        let cost = condition
            .work_size()
            .saturating_mul(MAX_GUARDS.saturating_mul(5).saturating_add(4))
            .saturating_add(MAX_GUARDS);
        if !context.budget.charge(cost) {
            return Err(FrontierReason::Work);
        }
        if !planned.contains(&condition) {
            planned.push(condition);
            if planned.len() == MAX_GUARDS {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn remember(proofs: &mut Proofs, condition: ExprId, value: &AbstractValue, feasible: bool) {
    proofs.infeasible |= !feasible;
    proofs.known.insert(
        condition,
        if feasible {
            value.singleton().map(|zero| zero == U256::ZERO)
        } else {
            None
        },
    );
}

fn plan(
    conditions: Vec<ExprId>,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
    proofs: &mut Proofs,
) -> Result<(), FrontierReason> {
    let limits = context.config.analysis.relations;
    let mut pending = Vec::new();
    for condition in conditions {
        let mut value = match boolean(&condition, limits.expressions()) {
            Ok(value) => value,
            Err(FrontierReason::Relations(reason)) if reason != QueryReason::Cancelled => {
                continue;
            }
            Err(reason) => return Err(reason),
        };
        let expression = match prepare_refine_mode(
            &mut value,
            relations,
            context,
            PreparationKind::IntrinsicBoolean,
        ) {
            Ok(PreparedRefinement::Done(feasible)) => {
                remember(proofs, condition, &value, feasible);
                if !feasible {
                    return Ok(());
                }
                continue;
            }
            Ok(PreparedRefinement::Pending(expression)) => expression,
            Err(FrontierReason::Relations(reason)) if reason != QueryReason::Cancelled => {
                // Optional preparation is not a new precision frontier. The
                // apply callback retries the original single-value policy.
                continue;
            }
            Err(reason) => return Err(reason),
        };
        // Prepare the complete original Fi before lookup, with no native fee.
        // Optional cache maintenance keeps the original quarter allowance.
        if !charge(relations, &expression, 0, context) {
            return Err(FrontierReason::Work);
        }
        let answer = relations.probe_unique_value_cached(
            &expression,
            &limits,
            &mut context
                .budget
                .smt_cache_query(limits.cache_scope())
                .with_allowance((limits.rlimit as usize).saturating_mul(2) / 4),
        );
        match answer {
            Ok(Some(answer)) => {
                // Only a proof of this actual Fi may enter the ordinary cache.
                context
                    .query_cache
                    .remember(relations, &expression, &limits, &answer);
                match finish_refine(&mut value, &expression, relations, context, answer) {
                    Ok(feasible) => {
                        remember(proofs, condition, &value, feasible);
                        if !feasible {
                            return Ok(());
                        }
                    }
                    Err(FrontierReason::Relations(reason)) if reason != QueryReason::Cancelled => {}
                    Err(reason) => return Err(reason),
                }
            }
            Ok(None) => pending.push(Pending {
                condition,
                value,
                expression,
            }),
            Err(QueryReason::Cancelled) => {
                return Err(FrontierReason::Relations(QueryReason::Cancelled));
            }
            Err(_) => {}
        }
    }

    // Aliases retain their own Boolean bounds, while the common full F encodes
    // each distinct target exactly once, with every source/array definition.
    let mut expressions = Vec::new();
    let mut indices = Vec::new();
    for target in &pending {
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
    if expressions.len() < 2 {
        return Ok(());
    }
    let cost = expressions
        .iter()
        .fold(relations.work_size(), |cost, expression| {
            cost.saturating_add(expression.work_size())
        })
        .saturating_add(limits.rlimit as usize);
    let answers = match relations.unique_many_budgeted(
        &expressions,
        &limits,
        &mut context.budget.query_budget(cost),
    ) {
        Ok(answers) => answers,
        Err(QueryReason::Cancelled) => {
            return Err(FrontierReason::Relations(QueryReason::Cancelled));
        }
        Err(_) => return Ok(()),
    };
    if answers
        .iter()
        .any(|answer| matches!(answer, BudgetedValueQuery::Work))
    {
        return Err(FrontierReason::Work);
    }
    if answers.iter().any(|answer| {
        matches!(
            answer,
            BudgetedValueQuery::Value(ValueQuery::Unknown(QueryReason::Cancelled))
        )
    }) {
        return Err(FrontierReason::Relations(QueryReason::Cancelled));
    }
    for (mut target, index) in pending.into_iter().zip(indices) {
        if let BudgetedValueQuery::Value(answer) = &answers[index]
            && !matches!(answer, ValueQuery::Unknown(_))
        {
            let feasible = finish_refine(
                &mut target.value,
                &target.expression,
                relations,
                context,
                answer.clone(),
            )?;
            // These stronger common-F proofs live only in the existing guard
            // callback map, never under a weaker Fi or ordinary QueryCache key.
            remember(proofs, target.condition, &target.value, feasible);
            if !feasible {
                return Ok(());
            }
        }
    }
    Ok(())
}

pub(super) fn refine(
    payload: &mut MachinePayload,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
    diagnostics: &mut Vec<(usize, DiagnosticKind)>,
    pc: usize,
    policy: (Domain, ExprLimits),
) -> Result<bool, FrontierReason> {
    let mut proofs = Proofs::default();
    if context.config.analysis.relations.enabled {
        let conditions = match gather(payload, context) {
            Ok(conditions) => conditions,
            Err(FrontierReason::Relations(reason)) if reason != QueryReason::Cancelled => {
                Vec::new()
            }
            Err(reason) => return Err(reason),
        };
        plan(conditions, relations, context, &mut proofs)?;
    }
    if proofs.infeasible {
        return Ok(false);
    }
    apply(payload, relations, context, diagnostics, pc, policy, proofs)
}

fn apply(
    payload: &mut MachinePayload,
    relations: &RelationState,
    context: &mut TransferContext<'_>,
    diagnostics: &mut Vec<(usize, DiagnosticKind)>,
    pc: usize,
    policy: (Domain, ExprLimits),
    Proofs {
        mut known,
        mut infeasible,
    }: Proofs,
) -> Result<bool, FrontierReason> {
    let (domain, limits) = policy;
    let mut proof = |condition: &ExprId| -> Result<Option<bool>, FrontierReason> {
        if let Some(truth) = known.get(condition) {
            return Ok(*truth);
        }
        // Unplanned/unknown targets run the original individual preparation,
        // finite-candidate policy and actual fees, in original wrapper order.
        let mut value = boolean(condition, limits)?;
        let truth = match super::super::refine(&mut value, relations, context) {
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

#[cfg(test)]
mod tests;
