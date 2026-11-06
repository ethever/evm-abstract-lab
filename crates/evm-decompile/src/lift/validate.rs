//! Certification checks the typed semantic projection against its raw witness.
use super::{Kind, Projection, project, remap_program};
use crate::{Cancellation, ValidationCheck};
use crate::{EntityId, Ir};

pub(crate) fn certify(ir: &Ir, cancel: &Cancellation) -> ValidationCheck {
    let fail = |message: String| ValidationCheck {
        name: "lifted_semantic_projection".to_owned(),
        passed: false,
        message,
    };
    let Some(program) = &ir.lifted else {
        return fail("Lifted stage has no semantic projection".to_owned());
    };
    if ir.structured.is_none() {
        return fail("Lifted stage has no source structured topology".to_owned());
    }
    if let Err(error) = program.validate() {
        return fail(error.to_string());
    }
    if super::guards::verify(ir, program, &mut |_| !cancel.is_cancelled()) != Some(true) {
        return fail(
            "empty-REVERT rank, source route or fallthrough certificate failed".to_owned(),
        );
    }
    // Rebuild the scoped normal form from primitive definitions. Only arena IDs
    // are alpha-renamed; source references, operand order and every semantic
    // record remain in the comparison. This checks reconstruction of the same
    // normal form and joint parameter/edge projection, not an independent proof
    // of the rewrite algebra. Rule equivalences and differential high-expression
    // tests supply that separate semantic evidence.
    let Some(expected) = project(ir, |_| !cancel.is_cancelled()) else {
        return fail(
            "projection certification cancelled or witness operands unavailable".to_owned(),
        );
    };
    // Unlike normal-form equality, these assertions inspect actual leaf
    // bindings and primitive positions. Model-level arena scope alone cannot
    // prove that a foreign raw definition exists at the consumer's entry, or
    // that an inlined pure result has a retained materialization.
    if expected
        .proof
        .check_program(ir, program, &mut |_| !cancel.is_cancelled())
        != Some(true)
    {
        return fail(
            "raw semantic binding, primitive order or entry availability proof failed".to_owned(),
        );
    }
    let mut canonical = program.clone();
    let ids = canonical
        .entity_ids()
        .enumerate()
        .map(|(i, id)| (id, EntityId(i as u64 + 1)))
        .collect();
    remap_program(&mut canonical, &ids);
    let passed = canonical == expected.program && !cancel.is_cancelled();
    ValidationCheck { name: "lifted_semantic_projection".to_owned(), passed,
        message: if passed { "checked typed scoped normal forms, anchored identity-copy equations, strict raw entry availability, retained primitive bindings, simultaneous live transfers and independent empty-REVERT rank/source/fallthrough certificates against raw witness" } else { "semantic projection differs from the certified normal form of its primitive witness" }.to_owned() }
}

/// Publication has no independent wire proof record: validate these direct
/// source correspondences before passing parents to Builder's occurrence checks.
/// Keep ancestry local to a retained transfer rather than expanding alias chains.
pub(super) fn source_correspondences(
    projection: &Projection<'_>,
    ir: &Ir,
    pay: &mut impl FnMut(usize) -> bool,
) -> Option<bool> {
    for expression in &projection.program.expressions {
        if !pay(1) {
            return None;
        }
        if let Kind::Value(value) = expression.kind
            && !projection
                .parents
                .get(&expression.id)
                .is_some_and(|parents| parents.contains(&value))
        {
            return Some(false);
        }
    }
    for (transfer, edge) in projection.program.transfers.iter().zip(&ir.edges) {
        if !pay(edge.arguments.len().saturating_add(1)) {
            return None;
        }
        let parents = projection.parents.get(&transfer.id)?;
        if !parents.contains(&edge.id) {
            return Some(false);
        }
        let target = &ir.blocks[projection.proof.blocks[&edge.to]];
        for (parameter, argument) in target.parameters.iter().zip(&edge.arguments) {
            if projection.proof.aliases.contains_key(parameter)
                && (!parents.contains(parameter)
                    || !parents.contains(argument)
                    || !parents.contains(&projection.proof.canonical(*parameter)))
            {
                return Some(false);
            }
        }
    }
    for control in &projection.program.controls {
        if let Some(guard) = &control.guard {
            if !pay(8) {
                return None;
            }
            let parents = projection.parents.get(&control.id)?;
            if !guard.source_references().all(|id| parents.contains(&id)) {
                return Some(false);
            }
        }
    }
    Some(true)
}
