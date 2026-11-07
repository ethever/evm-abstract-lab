//! Export must not execute untrusted guard source overlays, even on failed views.
use evm_decompile::*;

fn fixture(join: bool) -> Artifact {
    let code = if join {
        vec![
            0x34, 0x60, 7, 0x57, 0x60, 12, 0x56, 0x5b, 0x60, 42, 0x5f, 0x55, 0x5b, 0x5f, 0x5f, 0xfd,
        ]
    } else {
        vec![0x34, 0x60, 7, 0x57, 0x5f, 0x5f, 0xfd, 0x5b, 0x00]
    };
    let artifact = decompile(&Input::new(code, Fork::Osaka), &Config::default());
    let stage = artifact.current().unwrap();
    assert_eq!(stage.kind, StageKind::Lifted);
    assert_eq!(stage.validation.status, ValidationStatus::Passed);
    let program = stage.ir.lifted.as_ref().unwrap();
    program.validate_guard_sources(&stage.ir).unwrap();
    assert_eq!(
        export_text(&artifact, &Cancellation::default()).status,
        TextExportStatus::Complete
    );
    artifact
}

fn corrupt(artifact: &Artifact, field: &str) -> Artifact {
    let mut changed = artifact.clone();
    let current = changed.current_stage.unwrap();
    let stage = changed.stages.iter_mut().find(|s| s.id == current).unwrap();
    let root = &stage.ir.structured.as_ref().unwrap().root;
    let StructuredKind::Sequence(children) = &root.kind else {
        panic!("Sequence fixture")
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
            assert!(
                tail.pass_falls_through,
                "fixture pass really reaches the joined REVERT"
            );
            tail.pass_falls_through = false;
        }
        _ => panic!("unknown mutation"),
    }
    changed
}

#[test]
fn cli_export_reports_invalid_guard_topology_without_recursive_reentry_or_fake_work_error() {
    let base = fixture(true);
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
            let mut changed = corrupt(&base, field);
            let current = changed.current_stage.unwrap();
            let stage = changed.stages.iter_mut().find(|s| s.id == current).unwrap();
            stage.validation.status = status;
            assert_eq!(validate_stage(stage).status, ValidationStatus::Failed);
            let ir = &stage.ir;
            let program = ir.lifted.as_ref().unwrap();
            assert!(program.render_index(ir).validate_guard_sources().is_err());
            let result = export_text(&changed, &Cancellation::default());
            assert_eq!(
                result.status,
                TextExportStatus::Limited,
                "{field}: {}",
                result.text
            );
            assert!(result.text.contains("invalid lifted guard source topology"));
            assert!(!result.text.contains("work limit exhausted"));
            assert!(!result.text.contains("require("));
            // The export error changes neither primitive nor raw source history.
            assert_eq!(
                changed.current().unwrap().ir.blocks,
                base.current().unwrap().ir.blocks
            );
            assert_eq!(
                changed.current().unwrap().ir.structured,
                base.current().unwrap().ir.structured
            );
        }
    }
}

#[test]
fn valid_direct_and_shared_join_guards_continue_to_export_completely() {
    for join in [false, true] {
        let artifact = fixture(join);
        let export = export_text(&artifact, &Cancellation::default());
        assert!(export.text.contains("require("));
        if join {
            assert!(export.text.find("require(") < export.text.find("evm.sstore("));
            assert!(export.text.find("evm.sstore(") < export.text.rfind("evm.revert("));
        }
    }
}
