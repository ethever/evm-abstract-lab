//! Admission failures cannot publish candidate structure or overwrite cancellation.
use super::*;
use crate::{Cancellation, Config, EntityId, Fork, Input, StructuredKind};
use plan::{MAX_SOURCE_REFERENCES, Plan, PlanError, PlanId, PlanNode};

#[test]
fn local_resource_refusal_is_limited_and_invalid_control_is_failed() {
    for (error, expected, code) in [
        (PlanError::Limit, Completion::Limited, DiagnosticCode::Limit),
        (
            PlanError::Invalid(EntityId(7)),
            Completion::Failed,
            DiagnosticCode::Validation,
        ),
    ] {
        let mut owner = Builder::new(
            &Input::new(vec![], Fork::Osaka),
            &Config::default(),
            &Cancellation::new(),
            |_| {},
        );
        refused(&mut owner, error, "test plan admission");
        assert_eq!(owner.artifact.completion, expected);
        assert_eq!(owner.artifact.diagnostics[0].code, code);
        assert!(owner.artifact.stages.is_empty());
    }
}

#[test]
fn cancellation_keeps_its_original_refusal_classification() {
    let cancel = Cancellation::new();
    let mut owner = Builder::new(
        &Input::new(vec![], Fork::Osaka),
        &Config::default(),
        &cancel,
        |_| {},
    );
    cancel.cancel();
    assert!(!owner.pay(0));
    refused(
        &mut owner,
        PlanError::Invalid(EntityId(7)),
        "cancelled plan",
    );
    assert_eq!(owner.artifact.completion, Completion::Cancelled);
    assert_eq!(owner.artifact.diagnostics.len(), 1);
    assert_eq!(
        owner.artifact.diagnostics[0].code,
        DiagnosticCode::Cancelled
    );
}

#[test]
fn a_small_syntax_with_excessive_source_references_is_refused_before_ast_ids() {
    let plan = Plan {
        root: PlanId(0),
        regions: vec![],
        nodes: vec![PlanNode {
            kind: StructuredKind::Sequence(vec![]),
            sources: vec![EntityId(7); MAX_SOURCE_REFERENCES + 1],
        }],
    };
    let mut owner = Builder::new(
        &Input::new(vec![], Fork::Osaka),
        &Config::default(),
        &Cancellation::new(),
        |_| {},
    );
    assert!(matches!(
        materialize::materialize(&plan, &mut owner),
        Err(PlanError::Limit)
    ));
    assert_eq!(
        owner.id(),
        EntityId(1),
        "no owned syntax/transition identity was allocated"
    );
    assert!(owner.artifact.stages.is_empty());
}

#[test]
fn abandoned_dispatcher_candidates_do_not_change_the_published_rule() {
    let plan = Plan {
        root: PlanId(0),
        regions: vec![],
        nodes: vec![
            PlanNode {
                kind: StructuredKind::Sequence(vec![]),
                sources: vec![],
            },
            PlanNode {
                kind: StructuredKind::Dispatcher {
                    entry: EntityId(1),
                    cases: vec![],
                },
                sources: vec![EntityId(1)],
            },
        ],
    };
    assert_eq!(plan.has_dispatch(&mut |_| true), Ok(false));
}
