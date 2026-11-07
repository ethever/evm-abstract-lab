//! Local ranked payload identity admission at the shared rendering boundary.
use super::construction::capsule_operation;
use crate::{EntityId, Ir, LiftedModelError, LiftedProgram};
use std::collections::BTreeMap;
/// The render boundary checks exact payload propagation once for the shared
/// failure graph. A recorded Passed marker cannot select another error merely
/// by changing a guard's payload reference.
pub(in crate::lifted) fn check_failure_payloads(
    program: &LiftedProgram,
    ir: &Ir,
    raw: &BTreeMap<EntityId, &crate::Block>,
    high: &BTreeMap<EntityId, &crate::LiftedBlock>,
) -> Result<(), LiftedModelError> {
    use crate::{LiftedGuardPayload, Terminator};
    let failures: BTreeMap<_, _> = program.failures.iter().map(|f| (f.block, f)).collect();
    let edges: BTreeMap<_, _> = ir.edges.iter().map(|e| (e.id, e)).collect();
    let expressions: BTreeMap<_, _> = program.expressions.iter().map(|e| (e.id, e)).collect();
    let reverts: BTreeMap<_, _> = program.reverts.iter().map(|r| (r.block, r)).collect();
    let plans: BTreeMap<_, _> = program.constructions.iter().map(|c| (c.block, c)).collect();
    for failure in &program.failures {
        let fail = || LiftedModelError::Payload(failure.block);
        let block = raw.get(&failure.block).ok_or_else(fail)?;
        if failure.rank == 0 {
            match failure.payload {
                LiftedGuardPayload::ErrorString(id) => {
                    if !reverts.get(&block.id).is_some_and(|r| r.payload == id)
                        || !plans.get(&block.id).is_some_and(|p| p.start == 0)
                        || block.operations.iter().any(|o| !capsule_operation(o))
                    {
                        return Err(fail());
                    }
                }
                LiftedGuardPayload::EmptyRevert => {
                    let body = high.get(&block.id).ok_or_else(fail)?;
                    if body.exit_values.len()!=2
                        || !expressions.get(&body.exit_values[1]).is_some_and(|e| e.block == block.id
                            && e.ty == crate::LiftedType::Word256
                            && matches!(e.kind,crate::LiftedExpressionKind::Word(v) if v==crate::U256::ZERO))
                        || block.operations.last().is_none_or(|o| {
                            o.opcode != 0xfd || !capsule_operation(o)
                                || !matches!(&block.terminator, Terminator::Exit {kind:crate::ExitKind::Revert, values} if *values == o.inputs)
                        })
                        || block.operations[..block.operations.len().saturating_sub(1)].iter().any(|o|!o.effects.is_pure() || !capsule_operation(o))
                    {return Err(fail());}
                }
            }
        } else {
            if block
                .operations
                .iter()
                .any(|o| !o.effects.is_pure() || !capsule_operation(o))
            {
                return Err(fail());
            }
            let known = match &block.terminator {
                Terminator::Fallthrough { .. } => true,
                Terminator::Jump {
                    edges,
                    unresolved: false,
                    invalid_targets,
                    ..
                } => !edges.is_empty() && invalid_targets.is_empty(),
                Terminator::Branch {
                    then_edges,
                    else_edge: Some(_),
                    unresolved: false,
                    invalid_targets,
                    ..
                } => !then_edges.is_empty() && invalid_targets.is_empty(),
                _ => false,
            };
            if !known {
                return Err(fail());
            }
            let mut max = 0;
            for edge in block.terminator.edges() {
                let edge = edges.get(&edge).ok_or_else(fail)?;
                let child = failures.get(&edge.to).ok_or_else(fail)?;
                if edge.from != block.id
                    || child.payload != failure.payload
                    || child.rank >= failure.rank
                {
                    return Err(fail());
                }
                max = max.max(child.rank);
            }
            if max.checked_add(1) != Some(failure.rank) {
                return Err(fail());
            }
        }
    }
    for control in &program.controls {
        if let Some(guard) = &control.guard {
            let edge = edges
                .get(&guard.failure_edge)
                .ok_or(LiftedModelError::Payload(control.id))?;
            if !failures
                .get(&edge.to)
                .is_some_and(|f| f.payload == guard.payload)
            {
                return Err(LiftedModelError::Payload(control.id));
            }
        }
    }
    Ok(())
}
