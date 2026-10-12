use super::*;
#[test]
fn actual_report_encoding_is_bounded_cancellable_and_retains_cached_facts() {
    let mut source = crate::analyze::analyze(api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: "366000803700".into(),
            address: None,
        }),
        ..api::AnalyzeRequest::default()
    })
    .unwrap();
    let bytes = report(&source, &Control::default()).unwrap();
    let parsed: api::AnalyzeReply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed.result.unwrap(), source);
    source.metadata.export_limits.max_output_bytes = 128;
    let retained = source.clone();
    let error = report(&source, &Control::default()).unwrap_err();
    assert!(
        matches!(error.details, api::ErrorDetails::Export(api::ExportFailure {kind:api::ExportFailureKind::OutputBytes, written:Some(written), ..}) if written <= 128)
    );
    assert_eq!(source, retained);
    let control = Control::new(Default::default());
    control.cancellation().cancel();
    let error = report(&source, &control).unwrap_err();
    assert!(matches!(
        error.details,
        api::ErrorDetails::Export(api::ExportFailure {
            kind: api::ExportFailureKind::Cancelled,
            written: Some(0),
            ..
        })
    ));
    assert_eq!(source, retained);
}
