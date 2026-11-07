use super::*;
use evm_abstract_protocol::decompile_model::{EntityRef, StageKind};

#[test]
fn real_compressed_http_preserves_certified_definitions_calls_maps_and_navigation() {
    let assets = Assets::new();
    let pool = pool();
    let request = DecompileRequest {
        bytecode: include_str!(
            "../../../../evm-decompile/tests/fixtures/shared-procedures/pure-call-inputs.hex"
        )
        .trim()
        .into(),
        ..DecompileRequest::default()
    };
    let result = super::error_payloads::fetch(&request, &assets, &pool);
    let artifact = &result.artifact;
    assert_eq!(artifact.completion, Completion::Complete);
    assert_eq!(artifact.schema_version, 5);
    let stage = artifact.current().unwrap();
    let high = stage.ir.lifted.as_ref().unwrap();
    assert!(
        !high.procedures.is_empty(),
        "HTTP must preserve actual reusable definitions"
    );
    assert!(
        high.invocations.len() >= 2,
        "HTTP must preserve actual invocation sites"
    );
    high.validate_shared_sources(&stage.ir, |_| true).unwrap();
    let index = high.render_index(&stage.ir);
    assert_eq!(index.procedures().count(), high.procedures.len());
    for procedure in &high.procedures {
        assert!(index.node(procedure.body).is_some());
        let reference = EntityRef {
            stage: stage.id,
            entity: procedure.id,
        };
        assert!(
            !artifact.origin(reference).unwrap().source.is_empty(),
            "definition source navigation survives HTTP"
        );
    }
    for call in &high.invocations {
        assert_eq!(index.shared_invocation(call.source), Some(call));
        assert!(index.node(call.source).is_some());
        let reference = EntityRef {
            stage: stage.id,
            entity: call.id,
        };
        assert!(
            !artifact.origin(reference).unwrap().source.is_empty(),
            "invocation source navigation survives HTTP"
        );
    }
    let raw = artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Structured)
        .unwrap();
    let mut witness = stage.ir.clone();
    witness.lifted = None;
    assert_eq!(witness, raw.ir);
    let wire = serde_json::to_vec(&result).unwrap();
    let restored: evm_abstract_protocol::DecompileJobResult =
        serde_json::from_slice(&wire).unwrap();
    assert_eq!(
        restored, result,
        "source identities and all maps survive the HTTP result schema"
    );
    pool.shutdown().unwrap();
}
