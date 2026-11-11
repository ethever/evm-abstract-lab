//! Cached semantic fields for the selected occurrence, including syntax and
//! dispatcher transitions. References are actual IDs in the selected stage.
use super::{
    index::{StageIndex, children},
    structured,
};
use evm_abstract_protocol::decompile_model::{
    EntityId, EntityRef, Stage, StageKind, StructuredKind,
};
use std::collections::BTreeSet;

#[derive(Default)]
pub(super) struct Details {
    pub lines: Vec<String>,
    pub references: Vec<EntityRef>,
}

#[cfg(test)]
pub(super) fn build(stage: &Stage, index: &StageIndex, id: EntityId, bytecode: &[u8]) -> Details {
    build_expanded(stage, index, id, bytecode, &BTreeSet::new())
}

pub(super) fn build_expanded(
    stage: &Stage,
    index: &StageIndex,
    id: EntityId,
    bytecode: &[u8],
    expanded: &BTreeSet<EntityId>,
) -> Details {
    let work = index.render_work();
    let sources: BTreeSet<_> = stage
        .ir
        .lifted
        .as_ref()
        .into_iter()
        .flat_map(|program| &program.invocations)
        .filter(|call| expanded.contains(&call.id))
        .map(|call| call.source)
        .collect();
    let expanded_prepared = (!sources.is_empty())
        .then(|| super::lifted::shared::Prepared::with_expanded(&stage.ir, &work, &sources));
    let prepared = expanded_prepared.as_ref().unwrap_or(&index.shared);
    let mut call_details = Details::default();
    if let Some(program) = &stage.ir.lifted {
        let mut renderer =
            program.render_base_index_with_budget(&stage.ir, |amount| work.reserve(amount));
        if renderer
            .set_call_result_projection(
                |source| prepared.error().is_none() && prepared.invocation(source, false).is_some(),
                |amount| work.reserve(amount),
            )
            .is_err()
        {
            renderer = program.render_base_index_with_budget(&stage.ir, |_| false);
        }
        if let Some(details) =
            super::lifted::details_prepared(stage, index, id, &renderer, &work, prepared)
        {
            return details;
        }
        if index.operation(&stage.ir, id).is_some() {
            let mut references = Vec::new();
            super::lifted::call_result_details(
                &renderer,
                id,
                &work,
                &mut call_details.lines,
                &mut references,
            );
            call_details
                .references
                .extend(references.into_iter().map(|entity| EntityRef {
                    stage: stage.id,
                    entity,
                }));
        }
    }
    let mut details = Details::default();
    if stage.kind == StageKind::Decoded {
        details
            .lines
            .push("Original decode; recovery unavailable in this stage".into());
        if let Some(op) = index.operation(&stage.ir, id) {
            details
                .lines
                .push(structured::decoded_operation(op, bytecode));
        } else if let Some(block) = index.block(&stage.ir, id) {
            details.lines.push(format!(
                "Decoded block b{} · {} original instructions",
                id.0,
                block.operations.len()
            ));
        } else if let Some(value) = index.value(&stage.ir, id) {
            details.lines.push(format!(
                "Unrecovered placeholder e{} · {:?}",
                id.0, value.fact
            ));
        }
        return details;
    }
    let mut refs = Vec::new();
    if let Some(block) = index.block(&stage.ir, id) {
        details.lines.push(format!(
            "Block b{} · {} operations · memory m{} · effect q{}",
            id.0,
            block.operations.len(),
            block.memory_parameter.0,
            block.effect_parameter.0
        ));
        details.lines.push(structured::block_terminator(block));
        refs.extend(block.parameters.iter().copied());
        refs.extend([block.memory_parameter, block.effect_parameter]);
        refs.extend(block.terminator.edges());
        refs.extend(index.source_block(&stage.ir, id).map(|block| block.id));
    } else if let Some(op) = index.operation(&stage.ir, id) {
        details.lines.push(format!(
            "Raw IR o{}: {}",
            id.0,
            structured::operation(op, &stage.ir, index)
        ));
        details.lines.push(format!(
            "{}: {:?}",
            if structured::fault_reason(op, &stage.ir, index).is_some() {
                "Declared primitive effects (not completed)"
            } else {
                "Primitive effects"
            },
            op.effects
        ));
        refs.extend(op.inputs.iter().chain(&op.outputs).copied());
        refs.extend(
            [
                op.memory_input,
                op.memory_output,
                op.effect_input,
                op.effect_output,
            ]
            .into_iter()
            .flatten(),
        );
    } else if let Some(value) = index.value(&stage.ir, id) {
        details
            .lines
            .push(format!("Value v{}: {:?}", id.0, value.kind));
        details.lines.push(format!(
            "Fact: {:?} · scope: {:?}",
            value.fact, stage.ir.fact_scope
        ));
    } else if let Some(edge) = index.edge(&stage.ir, id) {
        details.lines.push(format!(
            "Edge e{}: b{} → b{} · {:?}",
            id.0, edge.from.0, edge.to.0, edge.kind
        ));
        details.lines.push(format!(
            "Parallel arguments: ({}) · memory m{} · effect q{}",
            structured::values(&edge.arguments),
            edge.memory.0,
            edge.effect.0
        ));
        refs.extend([edge.from, edge.to, edge.memory, edge.effect]);
        refs.extend(edge.arguments.iter().copied());
        refs.extend(
            index
                .source_transfer(&stage.ir, id)
                .map(|transfer| transfer.id),
        );
    } else if let Some(node) = index.node(&stage.ir, id) {
        refs.extend(
            index
                .source_control(&stage.ir, id)
                .map(|control| control.id),
        );
        refs.extend(children(node).into_iter().map(|child| child.id));
        let text = match &node.kind {
            StructuredKind::Sequence(nodes) => {
                format!("Sequence ast{} · {} ordered children", id.0, nodes.len())
            }
            StructuredKind::Block { block } => {
                refs.push(*block);
                format!("Block ast{} → b{}", id.0, block.0)
            }
            StructuredKind::Transfer { edge } => {
                refs.push(*edge);
                format!("Transfer ast{} → e{}", id.0, edge.0)
            }
            StructuredKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                refs.push(*condition);
                format!(
                    "If ast{}: v{} != 0 → ast{}; else ast{}",
                    id.0, condition.0, then_branch.id.0, else_branch.id.0
                )
            }
            StructuredKind::While {
                header,
                condition,
                condition_nonzero,
                body,
                exit_edge,
            } => {
                refs.extend(condition.iter().chain(exit_edge.iter()).copied());
                format!(
                    "While ast{}: header ast{}; condition {:?}, nonzero={condition_nonzero}; body ast{}; exit {:?}",
                    id.0, header.id.0, condition, body.id.0, exit_edge
                )
            }
            StructuredKind::Break => format!("Break ast{}", id.0),
            StructuredKind::Continue => format!("Continue ast{}", id.0),
            StructuredKind::Exit { block } => {
                refs.push(*block);
                format!("Exit ast{} → b{}", id.0, block.0)
            }
            StructuredKind::PartialExit { block } => {
                refs.push(*block);
                format!("Partial exit ast{} → b{}", id.0, block.0)
            }
            StructuredKind::Indirect {
                block,
                arms,
                unresolved,
            } => {
                refs.push(*block);
                format!(
                    "Indirect ast{}: block b{}; {} known alternatives; unresolved={unresolved}",
                    id.0,
                    block.0,
                    arms.len()
                )
            }
            StructuredKind::Dispatcher { entry, cases } => {
                refs.push(*entry);
                for case in cases {
                    refs.push(case.block);
                    refs.extend(case.transitions.iter().map(|transition| transition.id));
                }
                format!(
                    "Dispatcher ast{}: entry b{}; {} cases",
                    id.0,
                    entry.0,
                    cases.len()
                )
            }
        };
        details.lines.push(text);
    } else if let Some(transition) = index.transition(&stage.ir, id) {
        details.lines.push(format!(
            "Transition t{}: on e{} assign dispatch_state = b{}",
            id.0, transition.edge.0, transition.target.0
        ));
        refs.extend([transition.edge, transition.target]);
        if let Some(edge) = index.edge(&stage.ir, transition.edge)
            && let Some(target) = index.block(&stage.ir, edge.to)
        {
            details.lines.push(format!(
                "Parallel assignment ({}) = ({})",
                structured::values(&target.parameters),
                structured::values(&edge.arguments)
            ));
            details.lines.push(format!(
                "Memory m{} = m{}; effect q{} = q{}",
                target.memory_parameter.0, edge.memory.0, target.effect_parameter.0, edge.effect.0
            ));
        }
    }
    details.references = refs
        .into_iter()
        .map(|entity| EntityRef {
            stage: stage.id,
            entity,
        })
        .collect();
    details.lines.extend(call_details.lines);
    details.references.extend(call_details.references);
    details
}
