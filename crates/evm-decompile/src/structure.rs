//! Stable source facts -> shared region plans -> validated owned syntax.
//! Planning never allocates AST identities; all expansion is quoted before the
//! materializer publishes source occurrences through the existing stage boundary.
use crate::{
    Completion, DiagnosticCode, Progress, Severity, StageKind, Strategy, StructuredProgram,
    artifact::Builder,
    transform::{dominators, ir_size, irreducible},
};

mod facts;
mod materialize;
mod plan;
mod verify;

pub(crate) fn run(owner: &mut Builder<impl FnMut(Progress)>) {
    if owner.stopped || !owner.pay(ir_size(&owner.current().ir).saturating_mul(4)) {
        return;
    }
    let mut ir = owner.current().ir.clone();
    let Some(dom) = dominators(&ir, owner) else {
        return;
    };
    if owner.artifact.config.strategy == Strategy::CodeDuplication
        && irreducible(&ir, &dom).is_some()
    {
        owner.partial();
        owner.diagnostic(DiagnosticCode::Unsupported,Severity::Warning,None,None,"selected code duplication did not discharge irreducibility; prior CFG retained without dispatcher fallback".to_owned());
        return;
    }
    let Some(facts) = facts::Facts::new(&ir, &dom, owner) else {
        return;
    };
    let Some(plan) = plan::build(&ir, &facts, owner) else {
        return;
    };
    if let Err(error) = verify::verify(&ir, &plan, &mut |n| owner.pay(n)) {
        refused(
            owner,
            error,
            "source region plan refused before materialization",
        );
        return;
    }
    let materialized = match materialize::materialize(&plan, owner) {
        Ok(result) => result,
        Err(error) => {
            refused(owner, error, "structured materialization admission refused");
            return;
        }
    };
    let materialize::Materialized { root, parents } = materialized;
    let has_dispatch = match plan.has_dispatch(&mut |n| owner.pay(n)) {
        Ok(value) => value,
        Err(error) => {
            refused(owner, error, "structure plan inspection refused");
            return;
        }
    };
    ir.structured = Some(StructuredProgram { root });
    if owner.artifact.config.strategy==Strategy::Dispatcher && !owner.commit(
        StageKind::ControlFlow,ir.clone(),
        (if has_dispatch {"dispatcher_lowering"} else {"natural_regions"},
         "validated source-scoped region plans preserve primitive bodies and ordered edge transfers; admitted natural regions retain if/while and expanding regions use compact source dispatch"),
        parents.clone(),Vec::new(),Vec::new(),
    ) { return; }
    owner.commit(StageKind::Structured,ir,("structure_regions","materialize admitted region occurrences after source-control verification and exact expansion quotation"),parents,Vec::new(),Vec::new());
}

fn refused(owner: &mut Builder<impl FnMut(Progress)>, error: plan::PlanError, context: &str) {
    if owner.stopped {
        return;
    }
    let (completion, code) = match error {
        plan::PlanError::Limit => (Completion::Limited, DiagnosticCode::Limit),
        plan::PlanError::Invalid(_) => (Completion::Failed, DiagnosticCode::Validation),
    };
    owner.stop(completion, code, &format!("{context}: {error}"));
}

#[cfg(test)]
mod tests;
