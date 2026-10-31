//! Structural and SSA checks run before a candidate can replace current.
use crate::{
    Cancellation, EntityId, Ir, Stage, StageKind, StructuredKind, StructuredNode, Terminator,
    Validation, ValidationCheck, ValidationStatus, Value, ValueKind,
};
use std::collections::{BTreeMap, BTreeSet};

/// Validate identity, SSA scope, memory order, edge assignments and AST coverage.
pub fn validate_stage(stage: &Stage) -> Validation {
    validate_with_cancel(stage, &Cancellation::default())
}
pub(crate) fn validate_with_cancel(stage: &Stage, cancel: &Cancellation) -> Validation {
    let ir = &stage.ir;
    let decoded = stage.kind == StageKind::Decoded;
    let mut checks = Vec::new();
    let mut add = |name: &str, errors: Vec<String>| {
        checks.push(ValidationCheck {
            name: name.to_owned(),
            passed: errors.is_empty(),
            message: if errors.is_empty() {
                "checked".to_owned()
            } else {
                errors.join("; ")
            },
        })
    };
    let blocks: BTreeMap<_, _> = ir.blocks.iter().map(|b| (b.id, b)).collect();
    let values: BTreeMap<_, _> = ir.values.iter().map(|v| (v.id, v)).collect();
    let edges: BTreeMap<_, _> = ir.edges.iter().map(|e| (e.id, e)).collect();
    let mut all = Vec::new();
    for b in &ir.blocks {
        if cancel.is_cancelled() {
            return cancelled();
        }
        all.push(b.id);
        all.extend(b.operations.iter().map(|o| o.id));
    }
    all.extend(ir.values.iter().map(|v| v.id));
    all.extend(ir.edges.iter().map(|e| e.id));
    if let Some(s) = &ir.structured {
        collect_nodes(&s.root, &mut all);
    }
    let mut seen = BTreeSet::new();
    add(
        "identity_uniqueness",
        all.iter()
            .filter(|id| id.0 == 0 || !seen.insert(**id))
            .map(|id| format!("duplicate {id:?}"))
            .collect(),
    );
    let mut refs = Vec::new();
    if ir.entry.is_some_and(|id| !blocks.contains_key(&id)) {
        refs.push("entry block missing".to_owned());
    }
    let operation_ids: BTreeSet<_> = ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .map(|o| o.id)
        .collect();
    for v in &ir.values {
        if cancel.is_cancelled() {
            return cancelled();
        }
        match v.kind {
            ValueKind::Operation { operation, .. }
            | ValueKind::Effect {
                operation: Some(operation),
            }
            | ValueKind::Memory {
                operation: Some(operation),
            } => {
                if !operation_ids.contains(&operation) {
                    refs.push(format!("definition {:?} has missing operation", v.id));
                }
            }
            ValueKind::Parameter { block, .. }
            | ValueKind::MemoryParameter { block }
            | ValueKind::EffectParameter { block } => {
                if !blocks.contains_key(&block) {
                    refs.push(format!("parameter {:?} has missing block", v.id));
                }
            }
            _ => {}
        }
    }
    add("defined_references", refs);
    let mut cfg = Vec::new();
    let mut arity = Vec::new();
    let mut ssa = Vec::new();
    let mut mem = Vec::new();
    let mut ordered = Vec::new();
    let mut spans = Vec::new();
    for b in &ir.blocks {
        if cancel.is_cancelled() {
            return cancelled();
        }
        if b.source.is_some_and(|s| s.end_pc < s.pc) {
            spans.push(format!("reversed block span {:?}", b.id));
        }
        let mut available: BTreeSet<_> = b.parameters.iter().copied().collect();
        let mut memory = b.memory_parameter;
        let mut effect = b.effect_parameter;
        if !is_effect(values.get(&effect).copied()) {
            ordered.push(format!("missing effect parameter {:?}", b.id));
        }
        if !is_memory(values.get(&memory).copied()) {
            mem.push(format!("missing memory parameter {:?}", b.id));
        }
        for (slot, p) in b.parameters.iter().enumerate() {
            if !matches!(values.get(p).map(|v|&v.kind),Some(ValueKind::Parameter { block,index }) if *block==b.id && *index==slot)
            {
                ssa.push(format!("invalid parameter {p:?}"));
            }
        }
        for (position, o) in b.operations.iter().enumerate() {
            if cancel.is_cancelled() {
                return cancelled();
            }
            if o.source.is_some_and(|s| s.end_pc < s.pc) {
                spans.push(format!("reversed operation span {:?}", o.id));
            }
            let fault = position + 1 == b.operations.len()
                && matches!(b.terminator, Terminator::Trap { .. });
            if !decoded && !matches!(b.terminator, Terminator::Pending) && !fault {
                if !o.effects.is_pure() {
                    if o.effect_input != Some(effect) {
                        ordered.push(format!("effect order mismatch at {:?}", o.id));
                    }
                    if o.effect_output.is_none() {
                        ordered.push(format!("missing effect result at {:?}", o.id));
                    }
                }
                for input in &o.inputs {
                    if !available.contains(input) {
                        ssa.push(format!("out of scope operand {input:?} at {:?}", o.id));
                    }
                }
                if o.effects.memory_read || o.effects.memory_write {
                    if o.memory_input != Some(memory) {
                        mem.push(format!("memory order mismatch at {:?}", o.id));
                    }
                }
            }
            for output in &o.outputs {
                if !values.contains_key(output) {
                    ssa.push(format!("undefined output {output:?}"));
                }
                available.insert(*output);
            }
            if let Some(next) = o.effect_output {
                if !is_effect(values.get(&next).copied()) {
                    ordered.push(format!("undefined effect result {next:?}"));
                }
                effect = next;
            }
            if let Some(next) = o.memory_output {
                if !is_memory(values.get(&next).copied()) {
                    mem.push(format!("undefined memory output {next:?}"));
                }
                memory = next;
            }
        }
        let term_values: Vec<_> = match &b.terminator {
            Terminator::Jump { target, .. } => vec![*target],
            Terminator::Branch {
                condition, target, ..
            } => vec![*condition, *target],
            Terminator::Exit { values, .. } => values.clone(),
            _ => Vec::new(),
        };
        for v in term_values {
            if !available.contains(&v) {
                ssa.push(format!("out of scope terminator value {v:?}"));
            }
        }
        for id in b.terminator.edges() {
            match edges.get(&id) {
                Some(e) if e.from == b.id => {}
                _ => cfg.push(format!("terminator references wrong edge {id:?}")),
            }
        }
        for e in ir.edges.iter().filter(|e| e.from == b.id) {
            if !b.terminator.edges().contains(&e.id) {
                cfg.push(format!("orphan outgoing edge {:?}", e.id));
            }
            if let Some(target) = blocks.get(&e.to) {
                if target.parameters.len() != e.arguments.len() {
                    arity.push(format!(
                        "edge {:?}: {} arguments for {} parameters",
                        e.id,
                        e.arguments.len(),
                        target.parameters.len()
                    ));
                }
            } else {
                cfg.push(format!("edge {:?} target missing", e.id));
            }
            for argument in &e.arguments {
                if !available.contains(argument) {
                    ssa.push(format!(
                        "edge {:?} argument {argument:?} out of scope",
                        e.id
                    ));
                }
            }
            if e.effect != effect {
                ordered.push(format!("edge {:?} loses effect sequence", e.id));
            }
            if e.memory != memory {
                mem.push(format!("edge {:?} loses final memory token", e.id));
            }
        }
    }
    for e in &ir.edges {
        if !blocks.contains_key(&e.from) {
            cfg.push(format!("missing source block for {:?}", e.id));
        }
    }
    add("edge_terminator_consistency", cfg);
    add("phi_argument_arity", arity);
    add("ssa_block_scope", ssa);
    add("memory_dependency_order", mem);
    add("observable_effect_order", ordered);
    add("source_ranges", spans);
    let mut structural = Vec::new();
    if let Some(s) = &ir.structured {
        let mut covered = BTreeSet::new();
        check_node(&s.root, ir, 0, &mut covered, &mut structural);
        for id in blocks.keys() {
            if !covered.contains(id) {
                structural.push(format!("block {id:?} omitted from structured AST"));
            }
        }
    } else if stage.kind == StageKind::Structured {
        structural.push("structured stage has no AST".to_owned());
    }
    add("structured_references_and_coverage", structural);
    Validation {
        status: if checks.iter().all(|c| c.passed) {
            ValidationStatus::Passed
        } else {
            ValidationStatus::Failed
        },
        checks,
    }
}
fn is_memory(value: Option<&Value>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.kind,
            ValueKind::Memory { .. } | ValueKind::MemoryParameter { .. }
        )
    })
}
fn collect_nodes(n: &StructuredNode, ids: &mut Vec<EntityId>) {
    ids.push(n.id);
    match &n.kind {
        StructuredKind::Sequence(v) => {
            for n in v {
                collect_nodes(n, ids);
            }
        }
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            collect_nodes(then_branch, ids);
            collect_nodes(else_branch, ids);
        }
        StructuredKind::While { header, body, .. } => {
            collect_nodes(header, ids);
            collect_nodes(body, ids);
        }
        StructuredKind::Indirect { arms, .. } => {
            for a in arms {
                collect_nodes(&a.body, ids);
            }
        }
        StructuredKind::Dispatcher { cases, .. } => {
            for c in cases {
                ids.extend(c.transitions.iter().map(|t| t.id));
            }
        }
        _ => {}
    }
}
fn check_node(
    n: &StructuredNode,
    ir: &Ir,
    loops: usize,
    covered: &mut BTreeSet<EntityId>,
    errors: &mut Vec<String>,
) {
    match &n.kind {
        StructuredKind::Sequence(v) => {
            for n in v {
                check_node(n, ir, loops, covered, errors);
            }
        }
        StructuredKind::Block { block } => {
            if ir.block(*block).is_none() {
                errors.push(format!("AST block {block:?} missing"));
            }
            covered.insert(*block);
        }
        StructuredKind::Transfer { edge } => {
            if ir.edge(*edge).is_none() {
                errors.push(format!("AST edge {edge:?} missing"));
            }
        }
        StructuredKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            if ir.value(*condition).is_none() {
                errors.push(format!("AST condition {condition:?} missing"));
            }
            check_node(then_branch, ir, loops, covered, errors);
            check_node(else_branch, ir, loops, covered, errors);
        }
        StructuredKind::While {
            header,
            body,
            condition,
            exit_edge,
            ..
        } => {
            if condition.is_some_and(|id| ir.value(id).is_none()) {
                errors.push("loop condition missing".to_owned());
            }
            if exit_edge.is_some_and(|id| ir.edge(id).is_none()) {
                errors.push("loop exit edge missing".to_owned());
            }
            check_node(header, ir, loops + 1, covered, errors);
            check_node(body, ir, loops + 1, covered, errors);
        }
        StructuredKind::Break | StructuredKind::Continue => {
            if loops == 0 {
                errors.push("loop transfer outside loop".to_owned());
            }
        }
        StructuredKind::Exit { block } | StructuredKind::PartialExit { block } => {
            if ir.block(*block).is_none() {
                errors.push("AST exit block missing".to_owned());
            }
        }
        StructuredKind::Indirect { block, arms, .. } => {
            if ir.block(*block).is_none() {
                errors.push("indirect source missing".to_owned());
            }
            for a in arms {
                check_node(&a.body, ir, loops, covered, errors);
            }
        }
        StructuredKind::Dispatcher { entry, cases } => {
            if !cases.iter().any(|c| c.block == *entry) {
                errors.push("dispatcher entry has no case".to_owned());
            }
            let mut unique = BTreeSet::new();
            for c in cases {
                if !unique.insert(c.block) {
                    errors.push("duplicate dispatcher case".to_owned());
                }
                covered.insert(c.block);
                if let Some(b) = ir.block(c.block) {
                    let actual: BTreeSet<_> = c.transitions.iter().map(|t| t.edge).collect();
                    if actual != b.terminator.edges().into_iter().collect() {
                        errors.push(format!("dispatcher omits transfer from {:?}", c.block));
                    }
                    for t in &c.transitions {
                        if ir.edge(t.edge).is_none_or(|e| e.to != t.target) {
                            errors.push("dispatcher assignment mismatch".to_owned());
                        }
                    }
                } else {
                    errors.push("dispatcher case block missing".to_owned());
                }
            }
        }
    }
}

fn cancelled() -> Validation {
    Validation {
        status: ValidationStatus::NotRun,
        checks: vec![ValidationCheck {
            name: "cancellation".to_owned(),
            passed: false,
            message: "candidate validation cancelled".to_owned(),
        }],
    }
}
fn is_effect(value: Option<&Value>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.kind,
            ValueKind::Effect { .. } | ValueKind::EffectParameter { .. }
        )
    })
}
/// Check operation/effect order across preserved blocks and real copies. This
/// is an explicit structural invariant; semantic parity is tested by execution.
pub(crate) fn validate_transition(
    before: &Stage,
    after: &Stage,
    copies: &[(EntityId, EntityId)],
) -> ValidationCheck {
    let ancestry: BTreeMap<_, _> = copies.iter().map(|(a, b)| (*b, *a)).collect();
    let mut errors = Vec::new();
    for b in &after.ir.blocks {
        let original = ancestry.get(&b.id).copied().unwrap_or(b.id);
        let Some(old) = before.ir.block(original) else {
            errors.push(format!("block {:?} has no prior body ancestry", b.id));
            continue;
        };
        if before.kind == StageKind::Decoded {
            continue;
        }
        if old.operations.len() != b.operations.len() {
            errors.push(format!("operation count changed at {:?}", b.id));
            continue;
        }
        for (a, z) in old.operations.iter().zip(&b.operations) {
            if ancestry.get(&z.id).copied().unwrap_or(z.id) != a.id
                || a.opcode != z.opcode
                || a.effects != z.effects
                || a.source != z.source
            {
                errors.push(format!("operation/effect order changed at {:?}", z.id));
            }
            let normalize = |id: &EntityId| ancestry.get(id).copied().unwrap_or(*id);
            if a.inputs != z.inputs.iter().map(normalize).collect::<Vec<_>>()
                || a.outputs != z.outputs.iter().map(normalize).collect::<Vec<_>>()
                || a.memory_input != z.memory_input.as_ref().map(normalize)
                || a.memory_output != z.memory_output.as_ref().map(normalize)
                || a.effect_input != z.effect_input.as_ref().map(normalize)
                || a.effect_output != z.effect_output.as_ref().map(normalize)
            {
                errors.push(format!("definition/effect remap changed at {:?}", z.id));
            }
        }
    }
    ValidationCheck {
        name: "preserved_operation_effect_order_and_copy_remap".to_owned(),
        passed: errors.is_empty(),
        message: if errors.is_empty() {
            "checked body sequence, operand/result remaps and memory/effect dependencies".to_owned()
        } else {
            errors.join("; ")
        },
    }
}
