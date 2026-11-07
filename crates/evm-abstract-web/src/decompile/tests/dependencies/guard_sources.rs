//! Native admission and diagnostic rendering do not trust stored Passed flags.
use super::*;
use crate::decompile::index::StageIndex;
use evm_abstract_protocol::decompile_model::ValidationStatus;

const JOIN: &str = "34600757600c565b602a5f555b5f5ffd";
const NESTED: &str = "346007576018565b5f356010576018565b602a5f556018565b5f5ffd";

#[test]
fn web_rejects_forged_sources_and_direct_failed_stage_rows_fall_back_without_reentry() {
    let base = artifact(JOIN);
    let original = base.current().unwrap();
    let raw = rows(&structured::lines_indexed(
        original,
        &StageIndex::new(&original.ir),
    ));
    for field in [
        "self",
        "ancestor",
        "other",
        "absent",
        "failure",
        "scope",
        "start",
        "end",
        "fallthrough",
    ] {
        for status in [ValidationStatus::Passed, ValidationStatus::Failed] {
            let mut changed = base.clone();
            let current = changed.current_stage.unwrap();
            let stage = changed.stages.iter_mut().find(|s| s.id == current).unwrap();
            let root = &stage.ir.structured.as_ref().unwrap().root;
            let StructuredKind::Sequence(children) = &root.kind else {
                unreachable!()
            };
            let ancestor = root.id;
            let other = children[0].id;
            let program = stage.ir.lifted.as_mut().unwrap();
            let control = program
                .controls
                .iter_mut()
                .find(|c| c.guard.is_some())
                .unwrap();
            let guard = control.guard.as_mut().unwrap();
            match field {
                "self" => guard.pass_node = control.node,
                "ancestor" => guard.pass_node = ancestor,
                "other" => guard.pass_node = other,
                "absent" => guard.pass_node = EntityId(u64::MAX),
                "failure" => guard.failure_node = guard.pass_node,
                "scope" => guard.continuation.as_mut().unwrap().sequence = guard.pass_node,
                "start" => guard.continuation.as_mut().unwrap().start += 1,
                "end" => guard.continuation.as_mut().unwrap().end -= 1,
                "fallthrough" => {
                    let tail = guard.continuation.as_mut().unwrap();
                    assert!(tail.pass_falls_through);
                    tail.pass_falls_through = false;
                }
                _ => unreachable!(),
            }
            stage.validation.status = status;
            let index = StageIndex::new(&stage.ir);
            let direct = high(stage, false); // Deliberately bypass check_artifact.
            assert!(
                direct.iter().any(|row| row.text.starts_with("if (")),
                "{field}"
            );
            assert!(
                !direct.iter().any(|row| row.text.starts_with("require(")),
                "{field}"
            );
            assert!(
                direct.iter().any(|row| row.text.contains("evm.sstore(")),
                "pass witness lost"
            );
            assert!(
                direct.iter().any(|row| row.text.contains("evm.revert(")),
                "join witness lost"
            );
            assert_eq!(rows(&structured::lines_indexed(stage, &index)), raw);
            let error = model::check_artifact(&changed).unwrap_err();
            assert!(error.contains("guard source topology"), "{field}: {error}");
        }
    }
}

#[test]
fn valid_inherited_and_immediate_continuations_and_legacy_projection_stay_admitted() {
    for code in [JOIN, NESTED] {
        let artifact = artifact(code);
        model::check_artifact(&artifact).unwrap();
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        program.validate_guard_sources(&stage.ir).unwrap();
        assert!(
            high(stage, false)
                .iter()
                .any(|row| row.text.starts_with("if ("))
        );
        if code == NESTED {
            assert_eq!(
                program
                    .controls
                    .iter()
                    .filter(|c| c.guard.is_some())
                    .count(),
                2
            );
            assert!(
                program
                    .controls
                    .iter()
                    .any(|c| c.guard.as_ref().is_some_and(|g| g.continuation.is_none()))
            );
        }
    }
    let mut old = artifact("00");
    old.schema_version = 2;
    assert!(
        old.stages
            .iter()
            .filter_map(|s| s.ir.lifted.as_ref())
            .all(|p| !p.has_guards())
    );
    model::check_artifact(&old).unwrap();
}

#[test]
fn inherited_guard_cannot_acquire_an_ancestor_omission_plan() {
    let mut changed = artifact(NESTED);
    let current = changed.current_stage.unwrap();
    let stage = changed.stages.iter_mut().find(|s| s.id == current).unwrap();
    let program = stage.ir.lifted.as_mut().unwrap();
    let mut ancestor = program
        .controls
        .iter()
        .filter_map(|c| c.guard.as_ref())
        .find_map(|g| g.continuation.clone())
        .unwrap();
    ancestor.pass_falls_through = false;
    let inherited = program
        .controls
        .iter_mut()
        .filter_map(|c| c.guard.as_mut())
        .find(|g| g.continuation.is_none())
        .unwrap();
    inherited.continuation = Some(ancestor);
    let lines = high(stage, false); // No admission or stored-status gate here.
    assert!(lines.iter().any(|r| r.text.contains("evm.revert(")));
    assert!(lines.iter().all(|r| !r.text.starts_with("require(")));
    assert!(
        model::check_artifact(&changed)
            .unwrap_err()
            .contains("guard source topology")
    );
}
