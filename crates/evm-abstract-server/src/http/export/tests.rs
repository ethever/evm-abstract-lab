use super::*;
#[test]
fn actual_report_encoding_exceeds_64_mib_and_cancellation_retains_cached_facts() {
    const FORMER_BYTE_LIMIT: usize = 64 * 1024 * 1024;
    let mut source = crate::analyze::analyze(api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: "366000803700".into(),
            address: None,
        }),
        ..api::AnalyzeRequest::default()
    })
    .unwrap();
    let retained = source.clone();
    // Enlarge only a presentation annotation, retaining the analyzed numeric
    // facts, graph, exact payload and their original export limits/usage.
    source.metadata.environment.call_value.summary = "a".repeat(FORMER_BYTE_LIMIT);
    source.validate().unwrap();
    {
        let bytes = report(&source, &Control::default()).unwrap();
        assert!(bytes.len() > FORMER_BYTE_LIMIT);
        let parsed: api::AnalyzeReply = serde_json::from_slice(&bytes).unwrap();
        let decoded = parsed.result.unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded, source);
    }
    let control = Control::new(Default::default());
    control.cancellation().cancel();
    let error = roundtrip_error_reply(report(&source, &control).unwrap_err());
    assert_eq!(error.code, api::ApiErrorCode::Cancelled);
    let api::ErrorDetails::Export(detail) = error.details else {
        panic!("expected an export failure");
    };
    assert!(matches!(
        *detail,
        api::ExportFailure {
            kind: api::ExportFailureKind::Cancelled,
            required: 0,
            written: Some(0),
            ..
        }
    ));
    assert_eq!(detail.limits, source.metadata.export_limits);
    assert_eq!(detail.usage, source.metadata.export_usage);
    let annotation = std::mem::replace(
        &mut source.metadata.environment.call_value.summary,
        retained.metadata.environment.call_value.summary.clone(),
    );
    assert_eq!(annotation.len(), FORMER_BYTE_LIMIT);
    assert!(annotation.bytes().all(|byte| byte == b'a'));
    assert_eq!(source, retained);
}

fn roundtrip_error_reply(error: api::ApiError) -> api::ApiError {
    let reply = api::AnalyzeReply { result: Err(error) };
    let bytes = serde_json::to_vec(&reply).unwrap();
    let decoded: api::AnalyzeReply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, reply);
    decoded.result.unwrap_err()
}
