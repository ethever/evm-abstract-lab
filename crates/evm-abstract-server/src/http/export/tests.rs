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
    let error = roundtrip_error_reply(report(&source, &Control::default()).unwrap_err());
    let api::ErrorDetails::Export(detail) = error.details else {
        panic!("expected an export failure");
    };
    assert!(
        matches!(*detail, api::ExportFailure {kind:api::ExportFailureKind::OutputBytes, written:Some(written), ..} if written <= 128)
    );
    assert_eq!(detail.limits, source.metadata.export_limits);
    assert_eq!(detail.usage, source.metadata.export_usage);
    assert!(detail.required > detail.limits.max_output_bytes as u64);
    assert_eq!(source, retained);
    let control = Control::new(Default::default());
    control.cancellation().cancel();
    let error = roundtrip_error_reply(report(&source, &control).unwrap_err());
    let api::ErrorDetails::Export(detail) = error.details else {
        panic!("expected an export failure");
    };
    assert!(matches!(
        *detail,
        api::ExportFailure {
            kind: api::ExportFailureKind::Cancelled,
            written: Some(0),
            ..
        }
    ));
    assert_eq!(detail.limits, source.metadata.export_limits);
    assert_eq!(detail.usage, source.metadata.export_usage);
    assert_eq!(source, retained);
}

fn roundtrip_error_reply(error: api::ApiError) -> api::ApiError {
    let reply = api::AnalyzeReply { result: Err(error) };
    let bytes = serde_json::to_vec(&reply).unwrap();
    let decoded: api::AnalyzeReply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, reply);
    decoded.result.unwrap_err()
}
