//! Prepared publication is paid before optional work and adopted exactly once.
use super::{Builder, OptionalStop, PreparedPublication};
use crate::{Cancellation, Completion, Config, DiagnosticCode, Fork, Input, Progress, StageKind};

fn ignore(_: Progress) {}
type Source<P> = (
    Builder<P>,
    crate::Ir,
    std::collections::BTreeMap<crate::EntityId, Vec<crate::EntityId>>,
);
fn source() -> Source<impl FnMut(Progress)> {
    let input = Input::new(vec![0x00], Fork::Osaka);
    let config = Config::default();
    let artifact = crate::decompile(&input, &config);
    let high = artifact.current().unwrap().clone();
    let parents = artifact
        .mappings
        .origins
        .iter()
        .filter(|origin| origin.entity.stage == high.id)
        .map(|origin| {
            (
                origin.entity.entity,
                origin.parents.iter().map(|parent| parent.entity).collect(),
            )
        })
        .collect();
    let mut owner = Builder::new(&input, &config, &Cancellation::new(), ignore);
    owner.artifact = artifact;
    owner.artifact.stages.pop();
    owner.artifact.events.pop();
    let previous = owner.artifact.stages.last().unwrap();
    owner.artifact.current_stage = Some(previous.id);
    let retained =
        (crate::transform::ir_size(&high.ir) + crate::transform::ir_size(&previous.ir)) * 8;
    owner.artifact.statistics.history_entities -= retained as u64;
    owner
        .artifact
        .mappings
        .origins
        .retain(|origin| origin.entity.stage != high.id);
    owner.published_entities = owner
        .artifact
        .mappings
        .origins
        .iter()
        .map(|origin| (origin.entity.entity, origin.entity.stage))
        .collect();
    (owner, high.ir, parents)
}
fn prepare(
    owner: &mut Builder<impl FnMut(Progress)>,
    ir: crate::Ir,
    parents: std::collections::BTreeMap<crate::EntityId, Vec<crate::EntityId>>,
) -> PreparedPublication {
    owner
        .prepare_publication(
            StageKind::Lifted,
            ir,
            ("lift_semantic_expressions", "test paid transaction"),
            parents,
            Vec::new(),
            Vec::new(),
        )
        .unwrap()
}

#[test]
fn preparation_has_no_partial_artifact_and_adoption_needs_no_more_work() {
    let (mut owner, ir, parents) = source();
    let before = owner.artifact.clone();
    let published = owner.published_entities.clone();
    let transaction = prepare(&mut owner, ir, parents);
    assert!(transaction.passed());
    assert_eq!(owner.artifact, before);
    assert_eq!(owner.published_entities, published);
    let paid = owner.budget.used();
    assert!(paid > 0);
    assert!(owner.adopt_publication(transaction));
    assert_eq!(owner.budget.used(), paid);
    assert_eq!(owner.current().kind, StageKind::Lifted);
    assert_eq!(owner.artifact.stages.len(), before.stages.len() + 1);
}

#[test]
fn local_optional_refusal_keeps_real_spend_and_the_paid_base_publishable() {
    let (mut owner, ir, parents) = source();
    let base = prepare(&mut owner, ir, parents);
    let paid = owner.budget.used();
    let result = owner.optional(2, |owner| {
        assert!(owner.pay(1));
        assert!(!owner.pay(2));
    });
    assert!(matches!(
        result,
        Err(OptionalStop::Work {
            used: 1,
            requested: 2,
            limit: 2
        })
    ));
    assert_eq!(owner.budget.used(), paid + 1);
    assert!(!owner.stopped && !owner.budget.exhausted());
    assert!(owner.adopt_publication(base));
    assert_eq!(owner.artifact.completion, Completion::Complete);
}

#[test]
fn cancellation_after_preparation_never_adopts_the_candidate() {
    let (mut owner, ir, parents) = source();
    let previous = owner.artifact.current_stage;
    let transaction = prepare(&mut owner, ir, parents);
    owner.cancel.cancel();
    assert!(!owner.adopt_publication(transaction));
    assert_eq!(owner.artifact.current_stage, previous);
    assert_eq!(owner.artifact.completion, Completion::Cancelled);
}

#[test]
fn mandatory_budget_refusal_is_not_an_optional_success() {
    let (mut owner, ir, parents) = source();
    owner.budget.limit_remaining(0).unwrap();
    assert!(
        owner
            .prepare_publication(
                StageKind::Lifted,
                ir,
                ("lift_semantic_expressions", "budget test"),
                parents,
                Vec::new(),
                Vec::new()
            )
            .is_none()
    );
    assert!(owner.stopped);
    assert_eq!(owner.artifact.completion, Completion::Limited);
    assert_eq!(owner.current().kind, StageKind::Structured);
    assert!(owner.artifact.diagnostics.iter().any(|d| d.code == DiagnosticCode::Limit && d.message == "logical work limit exhausted"));
}

#[test]
fn prepared_source_kind_and_actual_ir_quote_cannot_be_replaced() {
    for change in ["source", "kind", "ir"] {
        let (mut owner, ir, parents) = source();
        let mut transaction = prepare(&mut owner, ir, parents);
        match change {
            "source" => {
                owner.artifact.current_stage = owner.artifact.stages.first().map(|stage| stage.id)
            }
            "kind" => transaction.stage.kind = StageKind::Baseline,
            "ir" => transaction.stage.ir.values.push(crate::Value {
                id: crate::EntityId(u64::MAX),
                kind: crate::ValueKind::Constant(crate::U256::ZERO),
                fact: crate::ValueFact::Unknown,
            }),
            _ => unreachable!(),
        }
        let stages = owner.artifact.stages.len();
        assert!(!owner.adopt_publication(transaction), "{change}");
        assert_eq!(owner.artifact.stages.len(), stages);
        assert_eq!(owner.artifact.completion, Completion::Failed);
    }
}

#[test]
fn optional_history_refusal_never_spends_the_paid_base_publication() {
    let (mut owner, ir, parents) = source();
    let base = prepare(&mut owner, ir, parents);
    owner.artifact.config.limits.max_history_entities = base.history_entities;
    let mut larger = base.ir().clone();
    larger.values.push(crate::Value {
        id: crate::EntityId(u64::MAX),
        kind: crate::ValueKind::Constant(crate::U256::ZERO),
        fact: crate::ValueFact::Unknown,
    });
    let result = owner.optional(usize::MAX, |owner| {
        owner.prepare_publication(
            StageKind::Lifted,
            larger,
            ("lift_semantic_expressions", "optional larger metadata"),
            base.parents(),
            Vec::new(),
            Vec::new(),
        )
    });
    assert!(matches!(result, Err(OptionalStop::History { required, limit }) if required > limit));
    assert!(!owner.stopped && !owner.budget.exhausted());
    assert!(owner.adopt_publication(base));
    assert_eq!(owner.artifact.completion, Completion::Complete);
}
