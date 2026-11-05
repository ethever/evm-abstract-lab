//! Certification checks the typed semantic projection against its raw witness.
use super::{project, remap_program};
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
    let mut canonical = program.clone();
    let ids = canonical
        .entity_ids()
        .enumerate()
        .map(|(i, id)| (id, EntityId(i as u64 + 1)))
        .collect();
    remap_program(&mut canonical, &ids);
    let passed = canonical == expected.program && !cancel.is_cancelled();
    ValidationCheck { name: "lifted_semantic_projection".to_owned(), passed,
        message: if passed { "checked typed scoped expression normal forms, primitive bindings, control operands and simultaneous live transfers against raw witness" } else { "semantic projection differs from the certified normal form of its primitive witness" }.to_owned() }
}
