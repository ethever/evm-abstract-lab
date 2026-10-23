use super::*;

#[test]
fn regional_evidence_survives_the_actual_pooled_reply_encoder() {
    let source = crate::analyze::analyze(api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: "5f5f5f5f5f305af150602a5f5260205ff3".into(),
            address: None,
        }),
        ..api::AnalyzeRequest::default()
    })
    .unwrap();
    assert!(!source.abstract_summaries.is_empty());
    assert!(
        source
            .abstract_summaries
            .iter()
            .any(|summary| !summary.regional_cfg.calls.is_empty())
    );
    let bytes = report(&source, &Control::default()).unwrap();
    let parsed: api::AnalyzeReply = serde_json::from_slice(&bytes).unwrap();
    let parsed = parsed.result.unwrap();
    parsed.validate().unwrap();
    assert_eq!(parsed, source);
    assert_eq!(parsed.schema_version, 19);
    // These are compact pooled reply bytes, before HTTP compression/framing.
    assert!(bytes.starts_with(b"{\"result\":{\"Ok\":"));
}
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
