//! Export must account for expanded semantic expressions independently of the
//! small source program/DAG, including cancellation inside a shared expression.
use super::{export_text, export_text_with_observer};
use crate::{Artifact, Cancellation, StageKind, TextExportStatus};
use crate::{Completion, Config, Fork, Input, decompile};

fn shared_expression() -> Artifact {
    let mut bytes = vec![0x5f, 0x35];
    for _ in 0..20 {
        bytes.extend([0x80, 0x01]);
    }
    bytes.extend([0x5f, 0x55, 0x00]);
    let artifact = decompile(&Input::new(bytes, Fork::Osaka), &Config::default());
    assert_eq!(artifact.completion, Completion::Complete);
    assert_eq!(artifact.current().unwrap().kind, StageKind::Lifted);
    artifact
}
#[test]
fn expression_text_ceiling_cannot_report_complete_export() {
    let artifact = shared_expression();
    let result = export_text(&artifact, &Cancellation::default());
    assert_eq!(result.status, TextExportStatus::Limited);
    assert!(result.work > 4_096);
    assert!(result.text.contains("work limit exhausted"));
    assert!(!result.text.contains("sstore("));
    assert_eq!(result.text.matches("calldataload(").count(), 1);
}
#[test]
fn semantic_expression_expansion_charges_work_and_checks_cancellation() {
    let artifact = shared_expression();
    let cancel = Cancellation::new();
    let mut checkpoints = 0;
    let result = export_text_with_observer(&artifact, &cancel, usize::MAX, |_| {
        checkpoints += 1;
        if checkpoints == 64 {
            cancel.cancel();
        }
    });
    assert!(cancel.is_cancelled());
    assert_eq!(result.status, TextExportStatus::Cancelled);
    assert!(result.work > 0 && result.work < crate::MAX_LIFTED_EXPRESSION_TEXT);
    let result = export_text_with_observer(&artifact, &Cancellation::new(), 2_048, |_| {});
    assert_eq!(result.status, TextExportStatus::Limited);
    assert!(result.work <= 2_048);
}
