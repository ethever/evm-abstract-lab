use super::{LiftedByteSegment, LiftedByteSource, LiftedDataWord, LiftedRevertDataKind};
use crate::EntityId;
use crate::{Ir, LiftedModelError as E, LiftedProgram, Terminator, ValueKind};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::lifted) fn check(
    program: &LiftedProgram,
    ir: &Ir,
    nodes: &BTreeMap<EntityId, &crate::StructuredNode>,
) -> Result<(), E> {
    if program.revert_data.is_empty() {
        return Ok(());
    }
    let values: BTreeMap<_, _> = ir.values.iter().map(|v| (v.id, &v.kind)).collect();
    let blocks: BTreeMap<_, _> = ir.blocks.iter().map(|b| (b.id, b)).collect();
    let operations: BTreeMap<_, _> = ir
        .blocks
        .iter()
        .flat_map(|b| {
            b.operations
                .iter()
                .enumerate()
                .map(move |(i, o)| (o.id, (b.id, i, o)))
        })
        .collect();
    let statements: BTreeMap<_, _> = program
        .blocks
        .iter()
        .flat_map(|b| {
            b.statements
                .iter()
                .map(move |s| (s.operation, (b.block, s)))
        })
        .collect();
    let expressions: BTreeMap<_, _> = program.expressions.iter().map(|e| (e.id, e)).collect();
    let live_parameters: BTreeSet<_> = program
        .blocks
        .iter()
        .flat_map(|b| b.parameters.iter().copied())
        .collect();
    let hash = ir
        .runtime_bytecode
        .as_ref()
        .filter(|code| code.len() == ir.bytecode_len)
        .map(alloy_primitives::keccak256);
    for data in &program.revert_data {
        data.validate()?;
        let fail = || E::Payload(data.id);
        let block = blocks.get(&data.block).ok_or_else(fail)?;
        let op = block.operations.last().ok_or_else(fail)?;
        if data.execution_domain != ir.execution_domain
            || data.execution_fork != ir.execution_fork
            || !ir.has_valid_execution_scope()
            || op.id != data.operation
            || op.opcode != 0xfd
            || op.memory_input != Some(data.memory)
            || op.inputs.as_slice() != [data.offset, data.size]
            || !matches!(&block.terminator,Terminator::Exit {kind:crate::ExitKind::Revert,values} if *values==op.inputs)
        {
            return Err(fail());
        }
        let mut seen = BTreeSet::new();
        let mut positions = BTreeMap::new();
        let mut code = false;
        for writer in &data.writes {
            let (owner, pos, op) = operations.get(writer).ok_or_else(fail)?;
            if !seen.insert(writer)
                || op.memory_input.is_none()
                || op.memory_output.is_none()
                || !op.effects.memory_write
                || positions
                    .insert(*owner, *pos)
                    .is_some_and(|old| old >= *pos)
            {
                return Err(fail());
            }
            code |= op.opcode == 0x39;
            if op.opcode == 0x39 {
                let span = op.source.ok_or_else(fail)?;
                let code = ir.runtime_bytecode.as_ref().ok_or_else(fail)?;
                if span.pc.checked_add(1) != Some(span.end_pc) || code.get(span.pc) != Some(&0x39) {
                    return Err(fail());
                }
            }
        }
        if data.code_hash
            != if code {
                Some(hash.ok_or_else(fail)?)
            } else {
                None
            }
        {
            return Err(fail());
        }
        for part in data.content.segments() {
            let (words, opcode) = match part {
                LiftedByteSegment::Word { value, .. } => ([Some(value), None], None),
                LiftedByteSegment::Slice {
                    source,
                    offset,
                    size,
                } => (
                    [Some(offset), Some(size)],
                    Some(match source {
                        LiftedByteSource::Calldata => 0x37,
                        _ => 0x3e,
                    }),
                ),
                _ => ([None, None], None),
            };
            for (slot, word) in words
                .into_iter()
                .enumerate()
                .filter_map(|(n, v)| v.map(|v| (n, v)))
            {
                if let LiftedDataWord::SourceValue(id) = word {
                    if !values.get(id).is_some_and(|kind| {
                        !matches!(
                            kind,
                            ValueKind::Memory { .. }
                                | ValueKind::MemoryParameter { .. }
                                | ValueKind::Effect { .. }
                                | ValueKind::EffectParameter { .. }
                        )
                    }) {
                        return Err(fail());
                    }
                    if !data.writes.iter().any(|writer| {
                        operations.get(writer).is_some_and(|(owner, _, op)| {
                            let position = if opcode.is_some() { slot + 1 } else { 1 };
                            opcode.map_or(matches!(op.opcode, 0x52 | 0x53), |opcode| {
                                op.opcode == opcode
                            }) && op.inputs.get(position) == Some(id)
                                && statements.get(writer).is_some_and(|(block, statement)| {
                                    block == owner
                                        && statement
                                            .inputs
                                            .get(position)
                                            .and_then(|expr| expressions.get(expr))
                                            .is_some_and(|expr| {
                                                expr.block == *owner
                                                    && expr.ty == crate::LiftedType::Word256
                                                    && !(live_parameters.contains(id)
                                                        && matches!(
                                                            expr.kind,
                                                            crate::LiftedExpressionKind::Word(_)
                                                        ))
                                            })
                                })
                        })
                    }) {
                        return Err(fail());
                    }
                }
            }
            if let LiftedByteSegment::Slice {
                source: LiftedByteSource::ReturnData(call),
                ..
            } = part
                && !operations.get(call).is_some_and(|(_, _, o)| {
                    matches!(o.opcode, 0xf0 | 0xf1 | 0xf2 | 0xf4 | 0xf5 | 0xfa)
                })
            {
                return Err(fail());
            }
        }
    }
    let data: BTreeMap<_, _> = program.revert_data.iter().map(|d| (d.id, d)).collect();
    let high: BTreeMap<_, _> = program.blocks.iter().map(|b| (b.block, b)).collect();
    let constants: BTreeMap<_, _> = ir
        .values
        .iter()
        .filter_map(|v| {
            if let ValueKind::Constant(word) = v.kind {
                Some((v.id, word))
            } else {
                None
            }
        })
        .collect();
    let mut bodies = BTreeMap::<EntityId, usize>::new();
    let mut exits = BTreeMap::<EntityId, usize>::new();
    for node in nodes.values() {
        match &node.kind {
            crate::StructuredKind::Block { block } => *bodies.entry(*block).or_default() += 1,
            crate::StructuredKind::Exit { block } => *exits.entry(*block).or_default() += 1,
            crate::StructuredKind::Dispatcher { cases, .. } => {
                for case in cases {
                    *bodies.entry(case.block).or_default() += 1;
                    *exits.entry(case.block).or_default() += 1;
                }
            }
            _ => {}
        }
    }
    for plan in &program.constructions {
        let Some(data) = data.get(&plan.revert) else {
            continue;
        };
        let fail = || E::Payload(plan.id);
        let body = high.get(&plan.block).ok_or_else(fail)?;
        if data.block != plan.block
            || matches!(data.content, LiftedRevertDataKind::Raw)
            || !bodies.get(&plan.block).is_some_and(|n| *n > 0)
            || bodies.get(&plan.block) != exits.get(&plan.block)
            || plan.start >= plan.end
            || plan.end != body.statements.len()
            || body
                .statements
                .last()
                .is_none_or(|s| s.operation != data.operation)
        {
            return Err(fail());
        }
        let legacy = crate::LiftedRevert {
            id: data.id,
            execution_domain: data.execution_domain,
            execution_fork: data.execution_fork,
            code_hash: data.code_hash,
            block: data.block,
            operation: data.operation,
            memory: data.memory,
            offset: data.offset,
            size: data.size,
            payload: data.id,
            writes: data.writes.clone(),
        };
        let deps = crate::lifted::payload::construction_dependencies(
            blocks[&data.block],
            &legacy,
            &constants,
        );
        let mut members = BTreeSet::new();
        let mut retained = BTreeSet::new();
        let mut previous = None;
        let mut start = 0;
        for (index, statement) in body.statements.iter().enumerate() {
            let (owner, pos, op) = operations.get(&statement.operation).ok_or_else(fail)?;
            if *owner != data.block || previous.is_some_and(|p| p >= *pos) {
                return Err(fail());
            }
            previous = Some(*pos);
            retained.insert(op.id);
            if !crate::lifted::payload::capsule_operation(op)
                || (matches!(op.opcode, 0x39 | 0x51..=0x53) && !deps.contains(&op.id))
            {
                start = index + 1;
            }
            if index >= plan.start {
                members.insert(op.id);
            }
        }
        if start != plan.start
            || data.writes.iter().any(|id| !members.contains(id))
            || blocks[&data.block].operations.iter().any(|op| {
                (!op.effects.is_pure() || !crate::lifted::payload::capsule_operation(op))
                    && !retained.contains(&op.id)
            })
        {
            return Err(fail());
        }
    }
    Ok(())
}
