//! Once-indexed transparent raw/high failure rules and adjacency.
use crate::{EntityId, Ir, LiftedProgram, Terminator};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Rule {
    Terminal(crate::LiftedGuardPayload),
    Transparent,
    Refused,
}
pub(super) struct Graph<'a> {
    pub blocks: BTreeMap<EntityId, usize>,
    pub edges: BTreeMap<EntityId, &'a crate::Edge>,
    pub high: BTreeMap<EntityId, &'a crate::LiftedBlock>,
    pub rules: Vec<Rule>,
    pub successors: Vec<Vec<usize>>,
    pub predecessors: Vec<Vec<usize>>,
}
impl<'a> Graph<'a> {
    pub fn build(
        ir: &'a Ir,
        program: &'a LiftedProgram,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Option<Self> {
        // All arenas and adjacency storage are quoted before indexing/allocation.
        let units = ir
            .blocks
            .len()
            .saturating_add(ir.edges.len())
            .saturating_add(program.expressions.len())
            .saturating_add(program.blocks.len())
            .saturating_add(program.transfers.len())
            .saturating_add(program.controls.len())
            .saturating_add(program.failures.len())
            .saturating_add(program.constructions.len())
            .saturating_add(program.reverts.len())
            .saturating_add(program.revert_data.len())
            .saturating_add(1);
        if !pay(units.saturating_mul(6)) {
            return None;
        }
        let blocks: BTreeMap<_, _> = ir
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.id, i))
            .collect();
        let edges: BTreeMap<_, _> = ir.edges.iter().map(|e| (e.id, e)).collect();
        let high: BTreeMap<_, _> = program.blocks.iter().map(|b| (b.block, b)).collect();
        let expressions: BTreeMap<_, _> = program.expressions.iter().map(|e| (e.id, e)).collect();
        let plans: BTreeMap<_, _> = program.constructions.iter().map(|c| (c.block, c)).collect();
        let reverts: BTreeMap<_, _> = program.reverts.iter().map(|r| (r.id, r)).collect();
        let data: BTreeMap<_, _> = program.revert_data.iter().map(|r| (r.id, r)).collect();
        let mut rules = vec![Rule::Refused; ir.blocks.len()];
        let mut successors = vec![Vec::new(); ir.blocks.len()];
        let mut predecessors = vec![Vec::new(); ir.blocks.len()];
        for (i, block) in ir.blocks.iter().enumerate() {
            let Some(lifted) = high.get(&block.id) else {
                continue;
            };
            if !pay(block
                .operations
                .len()
                .saturating_add(lifted.statements.len())
                .saturating_add(1))
            {
                return None;
            }
            let empty = matches!(&block.terminator,
                Terminator::Exit { kind: crate::ExitKind::Revert, values } if values.len() == 2)
                && lifted.exit_values.len() == 2
                && expressions.get(&lifted.exit_values[1])
                    .is_some_and(|e| matches!(e.kind, crate::LiftedExpressionKind::Word(v) if v == crate::U256::ZERO))
                && block.operations.last().is_some_and(|o| o.opcode == 0xfd
                    && matches!(&block.terminator, Terminator::Exit { values, .. } if *values == o.inputs));
            let plan = plans
                .get(&block.id)
                .filter(|p| p.start == 0 && p.end == lifted.statements.len());
            let payload = plan.and_then(|p| {
                reverts
                    .get(&p.revert)
                    .filter(|r| r.block == block.id)
                    .map(|r| crate::LiftedGuardPayload::ErrorString(r.payload))
                    .or_else(|| {
                        data.get(&p.revert)
                            .filter(|r| {
                                r.block == block.id
                                    && matches!(
                                        &r.content,
                                        crate::LiftedRevertDataKind::Segments(_)
                                    )
                            })
                            .map(|r| crate::LiftedGuardPayload::Data(r.id))
                    })
            });
            let prepared = plan.is_some_and(|p| {
                p.kind == crate::LiftedConstructionKind::PreparedConsumer
                    && matches!(payload, Some(crate::LiftedGuardPayload::Data(_)))
            });
            // All reads remain primitive witnesses. Even a discarded SLOAD or
            // CALLDATASIZE forbids erasing this failed-path observation. Only a
            // certified consumer may replay its required RDS lazily on failure.
            let transparent = block.operations.iter().enumerate().all(|(position, op)| {
                if empty && position + 1 == block.operations.len() {
                    return true;
                }
                (payload.is_some()
                    && if prepared {
                        crate::lift::payloads::consumer_operation(op)
                    } else {
                        crate::lift::payloads::capsule_operation(op)
                    })
                    || op.effects.is_pure()
                        && (evm_bytecode::word::arity(op.opcode).is_some()
                            || matches!(op.opcode, 0x50 | 0x56 | 0x57 | 0x5b | 0x5f..=0x9f))
            });
            if !transparent {
                continue;
            }
            if empty {
                rules[i] = Rule::Terminal(crate::LiftedGuardPayload::EmptyRevert);
                continue;
            }
            if let Some(payload) = payload {
                rules[i] = Rule::Terminal(payload);
                continue;
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
                continue;
            }
            let edge_count = match &block.terminator {
                Terminator::Jump { edges, .. } => edges.len(),
                Terminator::Branch { then_edges, .. } => then_edges.len().saturating_add(1),
                _ => 1,
            };
            if !pay(edge_count.saturating_mul(3)) {
                return None;
            }
            let outgoing = block.terminator.edges();
            let mut valid = true;
            for edge in outgoing {
                let Some(edge) = edges.get(&edge) else {
                    valid = false;
                    break;
                };
                let Some(to) = blocks.get(&edge.to).copied() else {
                    valid = false;
                    break;
                };
                if edge.from != block.id {
                    valid = false;
                    break;
                }
                successors[i].push(to);
            }
            if valid && !successors[i].is_empty() {
                rules[i] = Rule::Transparent;
                for next in &successors[i] {
                    predecessors[*next].push(i);
                }
            } else {
                successors[i].clear();
            }
        }
        Some(Self {
            blocks,
            edges,
            high,
            rules,
            successors,
            predecessors,
        })
    }
}
