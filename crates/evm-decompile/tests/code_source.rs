//! Runtime witnesses survive decoding, recovery, transforms and transport.
use evm_decompile::{
    Artifact, Completion, Config, Fork, Input, ValidationStatus, decompile, validate_stage,
};

#[test]
fn decoded_codecopy_captures_unchanged_runtime_in_every_snapshot() {
    for (code, captures) in [
        (vec![0x5f, 0x5f, 0x5f, 0x39, 0x00], true),
        (vec![0x60, 0x39, 0x50, 0x00], false),
    ] {
        let artifact = decompile(&Input::new(code.clone(), Fork::Osaka), &Config::default());
        assert_eq!(artifact.completion, Completion::Complete);
        assert!(!artifact.stages.is_empty());
        for stage in &artifact.stages {
            assert_eq!(
                stage.ir.runtime_bytecode.as_deref(),
                captures.then_some(code.as_slice())
            );
            assert_eq!(validate_stage(stage).status, ValidationStatus::Passed);
        }
        assert!(artifact.admits_schema());
        assert_eq!(
            serde_json::from_slice::<Artifact>(&serde_json::to_vec(&artifact).unwrap()).unwrap(),
            artifact
        );
        if captures {
            let mut corrupt = artifact.clone();
            corrupt.stages[0].ir.runtime_bytecode.as_mut().unwrap()[0] ^= 1;
            assert!(!corrupt.admits_schema());
            let mut truncated = artifact.current().unwrap().clone();
            truncated.ir.runtime_bytecode.as_mut().unwrap().pop();
            let validation = validate_stage(&truncated);
            assert_eq!(validation.status, ValidationStatus::Failed);
            assert!(
                validation
                    .checks
                    .iter()
                    .any(|c| c.name == "runtime_source" && !c.passed)
            );
        }
    }
}
