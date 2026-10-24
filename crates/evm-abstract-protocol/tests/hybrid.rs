//! Strict native prefix phases and owner-condition boundaries.
use evm_abstract_protocol::{HybridInstruction, HybridLocalScope, InstructionProgress};

#[test]
fn native_prefix_phases_and_owner_scope_roundtrip_strictly() {
    for phase in [
        InstructionProgress::Started,
        InstructionProgress::OperandsConsumed,
        InstructionProgress::Completed,
        InstructionProgress::Dispatched,
        InstructionProgress::Faulted,
    ] {
        let instruction = HybridInstruction { pc: 42, phase };
        let json = serde_json::to_string(&instruction).unwrap();
        assert_eq!(
            serde_json::from_str::<HybridInstruction>(&json).unwrap(),
            instruction
        );
    }
    for scope in [
        HybridLocalScope::RetainedInputPrefix,
        HybridLocalScope::NativeOwnerNamespacePrefix,
    ] {
        let json = serde_json::to_string(&scope).unwrap();
        assert_eq!(
            serde_json::from_str::<HybridLocalScope>(&json).unwrap(),
            scope
        );
    }
    for json in [
        r#"{"pc":42}"#,
        r#"{"pc":42,"phase":"Complete"}"#,
        r#"{"pc":42,"phase":"Started","phase":"Dispatched"}"#,
        r#"{"pc":42,"phase":"Dispatched","complete":true}"#,
    ] {
        assert!(
            serde_json::from_str::<HybridInstruction>(json).is_err(),
            "{json}"
        );
    }
}
