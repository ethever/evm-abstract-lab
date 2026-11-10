//! Execution premises travel with requests and snapshots, never analysis limits.
use evm_decompile_model::{Artifact, Config, ExecutionDomain as Domain, Fork, Ir, SCHEMA_VERSION};
use serde_json::json;

fn artifact(domain: Domain) -> Artifact {
    let mut artifact: Artifact =
        serde_json::from_str(include_str!("fixtures/partial-artifact.json")).unwrap();
    artifact.schema_version = SCHEMA_VERSION;
    artifact.config.execution_domain = domain;
    for stage in &mut artifact.stages {
        stage.ir.execution_domain = domain;
        stage.ir.execution_fork = (!domain.is_generic()).then_some(artifact.input.fork);
    }
    artifact
}

#[test]
fn generic_legacy_shape_stays_exact_and_new_fields_remain_strict() {
    let config = serde_json::to_value(Config::default()).unwrap();
    let ir = serde_json::to_value(Ir::default()).unwrap();
    assert!(config.get("execution_domain").is_none());
    assert!(ir.get("execution_domain").is_none() && ir.get("execution_fork").is_none());
    for invalid in [
        json!(null),
        json!([]),
        json!("ethereum"),
        json!({"ethereum_transactions": null}),
    ] {
        let mut wire = config.clone();
        wire["execution_domain"] = invalid;
        assert!(serde_json::from_value::<Config>(wire).is_err());
    }
    let old = include_str!("fixtures/partial-artifact.json").trim_end();
    let decoded: Artifact = serde_json::from_str(old).unwrap();
    assert!(decoded.admits_schema());
    assert_eq!(serde_json::to_string(&decoded).unwrap(), old);
}

#[test]
fn scoped_proofs_require_schema_eight_and_the_same_frozen_scope_everywhere() {
    let declared = artifact(Domain::EthereumTransactions);
    assert!(declared.admits_schema());
    assert_eq!(
        serde_json::from_slice::<Artifact>(&serde_json::to_vec(&declared).unwrap()).unwrap(),
        declared
    );
    for version in 1..8 {
        let mut old = declared.clone();
        old.schema_version = version;
        assert!(
            !old.admits_schema(),
            "bounded proof hidden in schema {version}"
        );
    }
    for mutation in ["config", "domain", "missing_fork", "wrong_fork"] {
        let mut wrong = declared.clone();
        match mutation {
            "config" => wrong.config.execution_domain = Domain::Generic,
            "domain" => wrong.stages.last_mut().unwrap().ir.execution_domain = Domain::Generic,
            "missing_fork" => wrong.stages.last_mut().unwrap().ir.execution_fork = None,
            "wrong_fork" => {
                wrong.stages.last_mut().unwrap().ir.execution_fork =
                    Some(if wrong.input.fork == Fork::Cancun {
                        Fork::Prague
                    } else {
                        Fork::Cancun
                    });
            }
            _ => unreachable!(),
        }
        assert!(
            !wrong.admits_schema(),
            "scope mutation admitted: {mutation}"
        );
    }
    let mut generic = artifact(Domain::Generic);
    generic.stages.last_mut().unwrap().ir.execution_fork = Some(generic.input.fork);
    assert!(!generic.admits_schema());
}

#[test]
fn old_reading_profiles_still_belong_to_schema_seven() {
    let mut generic = artifact(Domain::Generic);
    generic.config.readability.profile = evm_decompile_model::ReadabilityProfile::Compact;
    generic.schema_version = 7;
    let mut original = generic.input.fork.to_string().into_bytes();
    original.push(0);
    original.extend_from_slice(&generic.input.runtime_bytecode);
    original.extend_from_slice(
        format!(
            "recovery-schema=7;strategy={:?};limits={:?}",
            generic.config.strategy, generic.config.limits
        )
        .as_bytes(),
    );
    let old_source =
        evm_decompile_model::ArtifactId(format!("{:x}", alloy_primitives::keccak256(original)));
    assert_eq!(generic.readability_source(), old_source);
    assert_ne!(
        generic.readability_source(),
        evm_decompile_model::readability_source(&generic.input, &generic.config)
    );
    generic
        .readability
        .push(evm_decompile_model::RegionDecision {
            region: evm_decompile_model::RegionAnchor {
                source: old_source,
                entry: evm_decompile_model::EntityId(1),
                continuation: None,
                loop_header: None,
            },
            preference: evm_decompile_model::StructurePreference::Auto,
            selected: evm_decompile_model::StructureForm::Natural,
            reason: evm_decompile_model::ReadabilityReason::NaturalControl,
            natural: None,
            dispatcher: None,
        });
    assert!(generic.admits_schema());
    generic.schema_version = 6;
    assert!(!generic.admits_schema());
}

#[test]
fn local_reading_choices_cannot_cross_execution_premises() {
    let input = evm_decompile_model::Input::new(vec![0], Fork::Osaka);
    let generic = Config::default();
    let ethereum = Config {
        execution_domain: Domain::EthereumTransactions,
        ..generic.clone()
    };
    assert_ne!(
        evm_decompile_model::readability_source(&input, &generic),
        evm_decompile_model::readability_source(&input, &ethereum)
    );
}
