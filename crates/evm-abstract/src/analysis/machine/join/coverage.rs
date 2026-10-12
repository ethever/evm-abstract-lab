//! Optional complete-payload coverage proofs. Immutable roots and numeric
//! shape must agree first; SMT then proves all scalar bindings and old path
//! guarantees. A failed/unknown proof never removes the incoming alternative.

use super::{MachineError, checkpoint};
use crate::{
    analysis::MachinePayload,
    domain::{
        AbstractValue, Domain,
        relational::{
            CheckResult, Constraint, QueryReason, RelationPath, RelationPathKind, RelationState,
        },
        symbolic::{ExprId, ExprLimits},
    },
    resource::WorkBudget,
    world::Snapshot,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeMap;

fn reserve(budget: &mut WorkBudget, cost: usize) -> Result<(), MachineError> {
    if cost == usize::MAX || !budget.charge(cost) {
        Err(MachineError::work_failure())
    } else {
        Ok(())
    }
}

fn shape(payload: &MachinePayload) -> Result<(MachinePayload, Vec<Option<ExprId>>), MachineError> {
    let mut copy = payload.clone();
    let mut expressions = Vec::new();
    let mut erase = |value: &mut AbstractValue| {
        expressions.push(value.symbolic_expression());
        value.forget_expression();
        value.forget_identity();
    };
    for frame in copy.call_stack.iter_mut() {
        checkpoint()?;
        for value in &mut frame.stack {
            erase(value);
        }
        erase(&mut frame.call_value);
        frame.memory.update_values(&mut erase);
        frame.calldata.update_values(&mut erase);
        frame.returndata.update_values(&mut erase);
        let mut saved = frame.saved_store.state().clone();
        saved.update_values(&mut erase);
        frame.saved_store = Snapshot::from_state(saved);
    }
    for child in copy.call_stack.children_mut() {
        checkpoint()?;
        erase(&mut child.continuation.output_offset);
        erase(&mut child.continuation.output_size);
    }
    copy.store.update_values(&mut erase);
    copy.relations = RelationState::default();
    checkpoint()?;
    Ok((copy, expressions))
}

fn operation(op: u8, args: &[ExprId], limits: ExprLimits) -> Result<ExprId, MachineError> {
    Ok(ExprId::operation(op, args, limits)?)
}

fn boolean(expression: ExprId, nonzero: bool, limits: ExprLimits) -> Result<ExprId, MachineError> {
    let zero = operation(opcode::ISZERO, &[expression], limits)?;
    if nonzero {
        operation(opcode::ISZERO, &[zero], limits)
    } else {
        Ok(zero)
    }
}

fn combine(mut terms: Vec<ExprId>, op: u8, limits: ExprLimits) -> Result<ExprId, MachineError> {
    if terms.is_empty() {
        return Ok(ExprId::constant(U256::from(u8::from(op == opcode::AND))));
    }
    // Balanced boolean words bound depth independently of conjunction length.
    while terms.len() > 1 {
        checkpoint()?;
        let mut next = Vec::new();
        for pair in terms.chunks(2) {
            next.push(if pair.len() == 1 {
                pair[0].clone()
            } else {
                operation(op, pair, limits)?
            });
        }
        terms = next;
    }
    Ok(terms.pop().expect("nonempty conjunction"))
}

fn atom(constraint: &Constraint, limits: ExprLimits) -> Result<ExprId, MachineError> {
    let constant = ExprId::constant;
    match constraint {
        Constraint::Truth {
            expression,
            nonzero,
        } => boolean(expression.clone(), *nonzero, limits),
        Constraint::Equal { left, right } => {
            operation(opcode::EQ, &[left.clone(), right.clone()], limits)
        }
        Constraint::Range {
            expression,
            lower,
            upper,
        }
        | Constraint::SignedRange {
            expression,
            lower,
            upper,
        } => {
            let expression = if matches!(constraint, Constraint::SignedRange { .. }) {
                operation(
                    opcode::XOR,
                    &[expression.clone(), constant(U256::from(1) << 255)],
                    limits,
                )?
            } else {
                expression.clone()
            };
            let lower = boolean(
                operation(opcode::LT, &[expression.clone(), constant(*lower)], limits)?,
                false,
                limits,
            )?;
            let upper = boolean(
                operation(opcode::GT, &[expression, constant(*upper)], limits)?,
                false,
                limits,
            )?;
            combine(vec![lower, upper], opcode::AND, limits)
        }
        Constraint::Modulo {
            expression,
            modulus,
            residue,
        } => {
            // Native BV remainder at zero is not EVM MOD-at-zero semantics.
            // This malformed/non-positive atom cannot use this optional proof.
            if *modulus == U256::ZERO {
                return Err(MachineError::Expression(
                    crate::domain::symbolic::ExprError::Unsupported(opcode::MOD),
                ));
            }
            let remainder = operation(
                opcode::MOD,
                &[expression.clone(), constant(*modulus)],
                limits,
            )?;
            operation(opcode::EQ, &[remainder, constant(*residue)], limits)
        }
        Constraint::Bits {
            expression,
            zero,
            one,
        } => {
            let zero = operation(opcode::AND, &[expression.clone(), constant(*zero)], limits)?;
            let zero = operation(opcode::EQ, &[zero, constant(U256::ZERO)], limits)?;
            let required = operation(opcode::AND, &[expression.clone(), constant(*one)], limits)?;
            let required = operation(opcode::EQ, &[required, constant(*one)], limits)?;
            combine(vec![zero, required], opcode::AND, limits)
        }
        Constraint::Members { expression, values } => {
            let terms = values
                .iter()
                .map(|value| operation(opcode::EQ, &[expression.clone(), constant(*value)], limits))
                .collect::<Result<_, _>>()?;
            combine(terms, opcode::OR, limits)
        }
    }
}

fn path(
    root: &RelationPath,
    limits: ExprLimits,
    budget: &mut WorkBudget,
) -> Result<ExprId, MachineError> {
    let mut stack = vec![(root, false)];
    let mut encoded = BTreeMap::new();
    while let Some((path, ready)) = stack.pop() {
        reserve(budget, 1)?;
        if encoded.contains_key(&path.id()) {
            continue;
        }
        if !ready {
            stack.push((path, true));
            match path.kind() {
                RelationPathKind::And(children) => {
                    stack.extend(children.iter().map(|child| (child, false)))
                }
                RelationPathKind::Ite {
                    then_path,
                    else_path,
                    ..
                } => stack.extend([(then_path, false), (else_path, false)]),
                _ => {}
            }
            continue;
        }
        let get = |path: &RelationPath| encoded.get(&path.id()).cloned().ok_or(MachineError::Shape);
        let term = match path.kind() {
            RelationPathKind::True => ExprId::constant(U256::from(1)),
            RelationPathKind::False => ExprId::constant(U256::ZERO),
            RelationPathKind::Atoms(atoms) => combine(
                atoms
                    .iter()
                    .map(|item| atom(item, limits))
                    .collect::<Result<_, _>>()?,
                opcode::AND,
                limits,
            )?,
            RelationPathKind::And(children) => combine(
                children.iter().map(get).collect::<Result<_, _>>()?,
                opcode::AND,
                limits,
            )?,
            RelationPathKind::Ite {
                selector,
                then_path,
                else_path,
            } => ExprId::ite(selector.clone(), get(then_path)?, get(else_path)?, limits)?,
        };
        encoded.insert(path.id(), term);
    }
    encoded.remove(&root.id()).ok_or(MachineError::Shape)
}

fn condition(
    previous: &RelationState,
    mut equalities: Vec<ExprId>,
    limits: ExprLimits,
    budget: &mut WorkBudget,
) -> Result<ExprId, MachineError> {
    if previous.is_bottom() {
        return Ok(ExprId::constant(U256::ZERO));
    }
    equalities.push(path(previous.path(), limits, budget)?);
    for constraint in previous.constraints() {
        reserve(budget, 1)?;
        equalities.push(atom(constraint, limits)?);
    }
    combine(equalities, opcode::AND, limits)
}

pub(super) fn covers(
    previous: &MachinePayload,
    incoming: &MachinePayload,
    domain: Domain,
    budget: &mut WorkBudget,
) -> Result<bool, MachineError> {
    let policy = domain.spec().relations();
    if !policy.enabled {
        return Ok(false);
    }
    reserve(
        budget,
        previous
            .work_size()
            .saturating_add(incoming.work_size())
            .saturating_mul(4),
    )?;
    let (old_shape, old_values) = shape(previous)?;
    let (new_shape, new_values) = shape(incoming)?;
    if old_shape != new_shape || old_values.len() != new_values.len() {
        return Ok(false);
    }
    let limits = policy.expressions();
    let proof: Result<Option<ExprId>, MachineError> = (|| {
        let mut equalities = Vec::new();
        for (old, new) in old_values.into_iter().zip(new_values) {
            checkpoint()?;
            match (old, new) {
                (Some(old), Some(new)) if old != new => {
                    equalities.push(operation(opcode::EQ, &[old, new], limits)?)
                }
                (Some(_), None) | (None, Some(_)) => return Ok(None),
                _ => {}
            }
        }
        Ok(Some(condition(
            &previous.relations,
            equalities,
            limits,
            budget,
        )?))
    })();
    let predicate = match proof {
        Ok(Some(predicate)) => predicate,
        Ok(None) => return Ok(false),
        Err(MachineError::Work) => return Err(MachineError::Work),
        Err(MachineError::Relations(QueryReason::Cancelled)) => {
            return Err(MachineError::Relations(QueryReason::Cancelled));
        }
        Err(_) => return Ok(false),
    };
    if let Some(value) = predicate.as_constant() {
        return Ok(value != U256::ZERO);
    }
    reserve(
        budget,
        incoming
            .relations
            .work_size()
            .saturating_add(predicate.work_size())
            .saturating_add(policy.rlimit as usize),
    )?;
    Ok(
        match incoming
            .relations
            .can_equal(&predicate, U256::ZERO, &policy)
        {
            CheckResult::Unsat => true,
            CheckResult::Unknown(QueryReason::Cancelled) => {
                return Err(MachineError::Relations(QueryReason::Cancelled));
            }
            CheckResult::Sat | CheckResult::Unknown(_) => false,
        },
    )
}
