//! Linear indexed source admission for payload and statement omission metadata.
use super::construction::{capsule_operation, construction_dependencies};
use super::failures::check_failure_payloads;
use crate::{EntityId, Ir, LiftedModelError, LiftedProgram, StructuredKind, StructuredNode};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::lifted) fn check(
    program: &LiftedProgram,
    ir: &Ir,
    nodes: &BTreeMap<EntityId, &StructuredNode>,
) -> Result<(), LiftedModelError> {
    if !program.has_payloads() {
        return Ok(());
    }
    program.validate()?;
    let raw: BTreeMap<_, _> = ir.blocks.iter().map(|b| (b.id, b)).collect();
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
    let high: BTreeMap<_, _> = program.blocks.iter().map(|b| (b.block, b)).collect();
    let constants: BTreeMap<_, _> = ir
        .values
        .iter()
        .filter_map(|v| {
            if let crate::ValueKind::Constant(c) = v.kind {
                Some((v.id, c))
            } else {
                None
            }
        })
        .collect();
    let reverts: BTreeMap<_, _> = program.reverts.iter().map(|r| (r.id, r)).collect();
    let code_hash = ir
        .runtime_bytecode
        .as_ref()
        .filter(|code| code.len() == ir.bytecode_len)
        .map(alloy_primitives::keccak256);
    let terminal_blocks: BTreeSet<_> = program.reverts.iter().map(|r| r.block).collect();
    let mut occurrences = BTreeMap::<EntityId, usize>::new();
    let mut exits = BTreeMap::<EntityId, usize>::new();
    for node in nodes.values() {
        match node.kind {
            StructuredKind::Block { block } => *occurrences.entry(block).or_default() += 1,
            StructuredKind::Exit { block } => *exits.entry(block).or_default() += 1,
            StructuredKind::Dispatcher { ref cases, .. } => {
                for case in cases {
                    *occurrences.entry(case.block).or_default() += 1;
                    *exits.entry(case.block).or_default() += 1;
                    if terminal_blocks.contains(&case.block) && !case.transitions.is_empty() {
                        return Err(LiftedModelError::Payload(case.block));
                    }
                }
            }
            _ => {}
        }
    }
    for proof in &program.reverts {
        let fail = || LiftedModelError::Payload(proof.id);
        if proof.execution_domain != ir.execution_domain
            || proof.execution_fork != ir.execution_fork
            || !ir.has_valid_execution_scope()
        {
            return Err(fail());
        }
        let block = raw.get(&proof.block).ok_or_else(fail)?;
        let op = block.operations.last().ok_or_else(fail)?;
        if op.opcode != 0xfd
            || op.id != proof.operation
            || op.inputs.as_slice() != [proof.offset, proof.size]
            || op.memory_input != Some(proof.memory)
            || !matches!(&block.terminator,crate::Terminator::Exit {kind:crate::ExitKind::Revert,values} if *values==op.inputs)
        {
            return Err(fail());
        }
        let mut seen = BTreeSet::new();
        let mut positions = BTreeMap::new();
        let mut copies_code = false;
        for id in &proof.writes {
            let (owner, position, writer) = operations.get(id).ok_or_else(fail)?;
            if !seen.insert(id)
                || !matches!(writer.opcode, 0x39 | 0x52 | 0x53)
                || writer.inputs.len() != if writer.opcode == 0x39 { 3 } else { 2 }
                || writer.memory_input.is_none()
                || writer.memory_output.is_none()
                || positions
                    .insert(*owner, *position)
                    .is_some_and(|previous| previous >= *position)
            {
                return Err(fail());
            }
            if writer.opcode == 0x39 {
                let source = writer.source.ok_or_else(fail)?;
                let code = ir.runtime_bytecode.as_ref().ok_or_else(fail)?;
                if source.pc.checked_add(1) != Some(source.end_pc)
                    || code.get(source.pc) != Some(&0x39)
                {
                    return Err(fail());
                }
            }
            copies_code |= writer.opcode == 0x39;
        }
        let expected_code_hash = if copies_code {
            Some(code_hash.ok_or_else(fail)?)
        } else {
            None
        };
        if proof.code_hash != expected_code_hash {
            return Err(fail());
        }
    }
    for plan in &program.constructions {
        let fail = || LiftedModelError::Payload(plan.id);
        let body = high.get(&plan.block).ok_or_else(fail)?;
        let proof = reverts.get(&plan.revert).ok_or_else(fail)?;
        if proof.block != plan.block
            || !occurrences.get(&plan.block).is_some_and(|count| *count > 0)
            || occurrences.get(&plan.block) != exits.get(&plan.block)
            || plan.start >= plan.end
            || plan.end != body.statements.len()
            || body
                .statements
                .last()
                .is_none_or(|s| s.operation != proof.operation)
        {
            return Err(fail());
        }
        let dependencies = construction_dependencies(raw[&plan.block], proof, &constants);
        let mut previous = None;
        let mut members = BTreeSet::new();
        let mut expected_start = 0;
        let mut retained = BTreeSet::new();
        for (index, statement) in body.statements.iter().enumerate() {
            let (owner, position, op) = operations.get(&statement.operation).ok_or_else(fail)?;
            if *owner != plan.block || previous.is_some_and(|p| p >= *position) {
                return Err(fail());
            }
            previous = Some(*position);
            retained.insert(statement.operation);
            if !capsule_operation(op)
                || (matches!(op.opcode, 0x39 | 0x51..=0x53) && !dependencies.contains(&op.id))
            {
                expected_start = index + 1;
            }
            if index >= plan.start {
                members.insert(statement.operation);
            }
        }
        if raw[&plan.block].operations.iter().any(|op| {
            (!op.effects.is_pure() || !capsule_operation(op)) && !retained.contains(&op.id)
        }) || plan.start != expected_start
            || proof.writes.iter().any(|id| !members.contains(id))
            || !body.statements[plan.start..].iter().any(|s| {
                operations
                    .get(&s.operation)
                    .is_some_and(|(_, _, o)| matches!(o.opcode, 0x39 | 0x52 | 0x53))
            })
        {
            return Err(fail());
        }
    }
    check_failure_payloads(program, ir, &raw, &high)?;
    Ok(())
}
