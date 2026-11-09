//! Actual HTTP requests select backend reading policy and source-bound regions.
use super::{Assets, DecompileRequest, pool};
use evm_abstract_protocol::decompile_model::{
    ArtifactId, Completion, DiagnosticCode, ProcedurePreference, ReadabilityProfile,
    ReadabilityReason, RegionAnchor, RegionPreference, Severity, StructureForm,
    StructurePreference, StructuredKind,
};

#[test]
fn compressed_http_applies_global_reading_and_procedure_preferences() {
    let assets = Assets::new();
    let pool = pool();
    for profile in [
        ReadabilityProfile::Balanced,
        ReadabilityProfile::Linear,
        ReadabilityProfile::Compact,
    ] {
        for preference in [ProcedurePreference::Shared, ProcedurePreference::Inline] {
            let mut request = DecompileRequest {
                bytecode: include_str!("../../../../evm-decompile/tests/fixtures/shared-procedures/pure-call-inputs.hex").trim().into(),
                ..DecompileRequest::default()
            };
            request.readability.profile = profile;
            request.readability.procedures = preference;
            let result = super::error_payloads::fetch(&request, &assets, &pool);
            let artifact = &result.artifact;
            assert_eq!(artifact.config.readability, request.readability);
            assert_eq!(
                artifact.completion,
                Completion::Complete,
                "{:?}",
                artifact.diagnostics
            );
            let high = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
            high.validate_shared_sources(&artifact.current().unwrap().ir, |_| true)
                .unwrap();
            if preference == ProcedurePreference::Inline {
                assert!(high.procedures.is_empty());
                assert!(high.invocations.is_empty());
            } else {
                assert!(!high.procedures.is_empty());
                assert!(high.invocations.len() >= 2);
            }
        }
    }
    pool.shutdown().unwrap();
}

#[test]
fn http_region_choice_is_taken_from_a_retained_source_and_stale_requests_are_not_retargeted() {
    let assets = Assets::new();
    let pool = pool();
    // Both branches write a distinct word to the same slot and then stop.
    let mut request = DecompileRequest {
        bytecode: "5f35600a5760015f55005b60025f5500".into(),
        ..DecompileRequest::default()
    };
    request.readability.procedures = ProcedurePreference::Inline;
    let initial = super::error_payloads::fetch(&request, &assets, &pool);
    let entry = initial.artifact.current().unwrap().ir.entry.unwrap();
    let anchor: RegionAnchor = initial
        .artifact
        .readability
        .iter()
        .find(|choice| {
            choice.region.entry == entry
                && choice.region.continuation.is_none()
                && choice.region.loop_header.is_none()
        })
        .expect("source-scoped root choice supplied to the frontend")
        .region
        .clone();
    for (preference, form) in [
        (StructurePreference::Dispatcher, StructureForm::Dispatcher),
        (StructurePreference::Natural, StructureForm::Natural),
    ] {
        request.readability.profile = ReadabilityProfile::Linear;
        request.readability.regions = vec![RegionPreference {
            region: anchor.clone(),
            structure: preference,
        }];
        let result = super::error_payloads::fetch(&request, &assets, &pool);
        assert_eq!(result.artifact.completion, Completion::Complete);
        assert_eq!(result.artifact.config.readability, request.readability);
        let choice = result
            .artifact
            .readability
            .iter()
            .find(|choice| choice.region == anchor)
            .unwrap();
        assert_eq!(choice.preference, preference);
        assert_eq!(choice.selected, form);
        assert_eq!(choice.reason, ReadabilityReason::UserPreference);
        let root = &result
            .artifact
            .current()
            .unwrap()
            .ir
            .structured
            .as_ref()
            .unwrap()
            .root;
        assert_eq!(
            matches!(root.kind, StructuredKind::Dispatcher { .. }),
            form == StructureForm::Dispatcher
        );
    }
    request.readability.regions[0].structure = StructurePreference::Dispatcher;
    request.readability.regions[0].region.source = ArtifactId("another-source".into());
    let stale = super::error_payloads::fetch(&request, &assets, &pool);
    assert_eq!(stale.artifact.completion, Completion::Complete);
    assert_eq!(stale.artifact.config.readability, request.readability);
    assert!(
        stale
            .artifact
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::Unsupported && d.severity == Severity::Info)
    );
    assert!(
        !stale
            .artifact
            .readability
            .iter()
            .any(|choice| choice.reason == ReadabilityReason::UserPreference)
    );
    assert!(!matches!(
        stale
            .artifact
            .current()
            .unwrap()
            .ir
            .structured
            .as_ref()
            .unwrap()
            .root
            .kind,
        StructuredKind::Dispatcher { .. }
    ));
    pool.shutdown().unwrap();
}
