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
    let Some(mut plan) = plan::build(&ir, &facts, owner) else {
        return;
    };
    let mut rejected = None;
    if let Err(error) = verify::verify(&ir, &plan, &mut |n| owner.pay(n)) {
        if !owner.stopped
            && matches!(error, plan::PlanError::Invalid(_))
            && owner.artifact.config.strategy == Strategy::Dispatcher
        {
            let Some(fallback) = plan::compact_fallback(&ir, &facts, owner) else {
                refused(
                    owner,
                    error,
                    "source region plan and compact fallback refused",
                );
                return;
            };
            if let Err(error) = verify::verify(&ir, &fallback, &mut |n| owner.pay(n)) {
                refused(owner, error, "compact source fallback validation refused");
                return;
            }
            if !owner.pay(256) {
                return;
            }
            rejected = Some(format!(
                "readability candidate rejected by source-control verification ({error}); independently verified whole-source dispatcher retained"
            ));
            plan = fallback;
        } else {
            refused(
                owner,
                error,
                "source region plan refused before materialization",
            );
            return;
        }
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
    let decisions = match plan.decisions(&mut |n| owner.pay(n)) {
        Ok(decisions) => decisions,
        Err(error) => {
            refused(
                owner,
                error,
                "retained readability receipt admission refused",
            );
            return;
        }
    };
    let preferences = &owner.artifact.config.readability.regions;
    let lookup_work = decisions
        .len()
        .saturating_add(preferences.len())
        .saturating_mul(decisions.len().max(1).ilog2() as usize + 1)
        .saturating_mul(128);
    let message_work = preferences.iter().fold(0_usize, |work, preference| {
        work.saturating_add(preference.region.source.0.len().saturating_mul(4))
            .saturating_add(512)
    });
    if !owner.pay(lookup_work.saturating_add(message_work)) {
        return;
    }
    let selected: std::collections::BTreeMap<_, _> = decisions
        .iter()
        .map(|decision| (&decision.region, decision.preference))
        .collect();
    let unmatched: Vec<_> = owner.artifact.config.readability.regions.iter()
        .filter(|preference| selected.get(&preference.region) != Some(&preference.structure))
        .map(|preference| format!(
            "readability preference for source {} entry {} continuation {:?} loop {:?} did not match a retained source region (or conflicted with another preference); unchanged verified choice retained",
            preference.region.source.0, preference.region.entry.0,
            preference.region.continuation, preference.region.loop_header,
        ))
        .collect();
    ir.structured = Some(StructuredProgram { root });
    if owner.artifact.config.strategy==Strategy::Dispatcher && !owner.commit(
        StageKind::ControlFlow,ir.clone(),
        (if has_dispatch {"dispatcher_lowering"} else {"natural_regions"},
         "validated source-scoped region plans preserve primitive bodies and ordered edge transfers; admitted natural regions retain if/while and expanding regions use compact source dispatch"),
        parents.clone(),Vec::new(),Vec::new(),
    ) { return; }
    if owner.commit(StageKind::Structured,ir,("structure_regions","materialize admitted region occurrences after source-control verification and exact expansion quotation"),parents,Vec::new(),Vec::new()) {
        owner.artifact.readability = decisions;
        if let Some(message) = rejected {
            owner.diagnostic(DiagnosticCode::Unsupported, Severity::Info, None, None, message);
        }
        for message in unmatched {
            owner.diagnostic(DiagnosticCode::Unsupported, Severity::Info, None, None, message);
        }
    }
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
