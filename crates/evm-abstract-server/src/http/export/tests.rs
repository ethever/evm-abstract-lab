use super::*;

fn encode(source: &api::AnalysisReport, control: &Control) -> Result<Vec<u8>, api::ApiError> {
    publication(
        &crate::jobs::PublishedResult {
            id: api::JobId(1),
            publication: api::JobPublication {
                version: api::PublicationVersion(api::DecimalU64(1)),
                global_version: source
                    .global
                    .as_ref()
                    .map(|_| api::PublicationVersion(api::DecimalU64(1))),
                native_version: source
                    .native
                    .as_ref()
                    .map(|_| api::PublicationVersion(api::DecimalU64(1))),
                model_closed: source.status == api::AnalysisStatus::Converged,
            },
            report: std::sync::Arc::new(source.clone()),
        },
        control,
    )
}

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
    assert!(
        !source
            .selected_graph()
            .unwrap()
            .abstract_summaries
            .is_empty()
    );
    assert!(
        source
            .selected_graph()
            .unwrap()
            .abstract_summaries
            .iter()
            .any(|summary| !summary.regional_cfg.calls.is_empty())
    );
    let bytes = encode(&source, &Control::default()).unwrap();
    let parsed: api::JobResultReply = serde_json::from_slice(&bytes).unwrap();
    let parsed = parsed.result.unwrap();
    assert_eq!(parsed.id, api::JobId(1));
    let parsed = parsed.report;
    parsed.validate().unwrap();
    assert_eq!(parsed, source);
    assert_eq!(parsed.schema_version, 21);
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
    std::sync::Arc::make_mut(source.global.as_mut().unwrap())
        .metadata
        .environment
        .call_value
        .summary = "a".repeat(FORMER_BYTE_LIMIT);
    source.validate().unwrap();
    {
        let bytes = encode(&source, &Control::default()).unwrap();
        assert!(bytes.len() > FORMER_BYTE_LIMIT);
        let parsed: api::JobResultReply = serde_json::from_slice(&bytes).unwrap();
        let decoded = parsed.result.unwrap().report;
        decoded.validate().unwrap();
        assert_eq!(decoded, source);
    }
    let control = Control::new(Default::default());
    control.cancellation().cancel();
    let error = roundtrip_error_reply(encode(&source, &control).unwrap_err());
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
    assert_eq!(
        detail.limits,
        source.selected_graph().unwrap().metadata.export_limits
    );
    assert_eq!(
        detail.usage,
        source.selected_graph().unwrap().metadata.export_usage
    );
    let annotation = std::mem::replace(
        &mut std::sync::Arc::make_mut(source.global.as_mut().unwrap())
            .metadata
            .environment
            .call_value
            .summary,
        retained
            .selected_graph()
            .unwrap()
            .metadata
            .environment
            .call_value
            .summary
            .clone(),
    );
    assert_eq!(annotation.len(), FORMER_BYTE_LIMIT);
    assert!(annotation.bytes().all(|byte| byte == b'a'));
    assert_eq!(source, retained);
}

fn roundtrip_error_reply(error: api::ApiError) -> api::ApiError {
    let reply = api::JobResultReply { result: Err(error) };
    let bytes = serde_json::to_vec(&reply).unwrap();
    let decoded: api::JobResultReply = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, reply);
    decoded.result.unwrap_err()
}
