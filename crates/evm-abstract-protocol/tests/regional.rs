//! Regional evidence keeps its own strict namespace and lossless source revisions.
use evm_abstract_protocol::*;

fn frame() -> RegionalFrame {
    RegionalFrame {
        code_address: "0x0000000000000000000000000000000000000102".into(),
        code_hash: Some(format!("0x{}", "ab".repeat(32))),
        storage_address: "0x0000000000000000000000000000000000000101".into(),
        address_value: AddressValue::Symbolic(InputSymbol {
            kind: InputSymbolKind::To,
            index: None,
        }),
        caller: AddressValue::Symbolic(InputSymbol {
            kind: InputSymbolKind::Caller,
            index: None,
        }),
        kind: CodeKind::Runtime,
        is_static: true,
        basic_block: Some(7),
        stack_height: 2,
        context: vec![41, 9],
    }
}

#[test]
fn source_current_is_required_and_distinct_from_observed_snapshot_closure() {
    for source_current in [false, true] {
        let summary = AbstractSummary {
            source_state: 0,
            source_current,
            kind: ProcedureSummaryKind::KnownCall,
            root_procedure: None,
            closed: true,
            procedures: vec![],
            regional_cfg: RegionalCfg::default(),
            state_count: 0,
            transfers: 0,
            propagations: 0,
            reused: 0,
        };
        strict(summary.clone());
        assert!(summary.closed);
        assert_eq!(summary.source_status(true), SummarySourceStatus::Historical);
        assert_eq!(
            summary.source_status(false),
            if source_current {
                SummarySourceStatus::Current
            } else {
                SummarySourceStatus::Uncertified
            }
        );
    }
}

fn strict<T>(value: T)
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de> + PartialEq + std::fmt::Debug,
{
    let encoded = serde_json::to_value(&value).unwrap();
    assert_eq!(serde_json::from_value::<T>(encoded.clone()).unwrap(), value);
    for key in encoded.as_object().unwrap().keys() {
        let mut missing = encoded.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<T>(missing).is_err(),
            "missing {key}"
        );
        let original = serde_json::to_string(&value).unwrap();
        let duplicate = format!("{{\"{key}\":{},{}", encoded[key], &original[1..]);
        assert!(
            serde_json::from_str::<T>(&duplicate).is_err(),
            "duplicate {key}"
        );
    }
    let mut unknown = encoded;
    unknown["obsolete"] = serde_json::json!(true);
    assert!(serde_json::from_value::<T>(unknown).is_err());
}

#[test]
fn regional_records_are_strict_including_nullable_and_empty_evidence() {
    let edge = RegionalEdge {
        to: 1,
        kind: EdgeKind::BranchFalse,
    };
    let transfer = RegionalTransfer {
        semantics: TransferSemantics::Native,
        process: None,
        input_revision: DecimalU64(u64::MAX),
        current: false,
        complete: true,
        visited_block: true,
        successors: vec![edge.clone()],
        calls: vec![0],
        outcomes: vec![OutcomeKind::Failure],
    };
    let node = RegionalNode {
        id: 0,
        procedure: 1,
        frame: frame(),
        input_revision: DecimalU64(u64::MAX),
        excluded: false,
        successors: vec![1],
        transfer: Some(transfer.clone()),
    };
    let delivery = RegionalReturn {
        kind: OutcomeKind::Revert,
        input_revision: DecimalU64(u64::MAX),
        output_generation: 17,
        current: false,
        successors: vec![RegionalEdge {
            to: 0,
            kind: EdgeKind::SummaryRevert,
        }],
    };
    let call = RegionalCall {
        action: None,
        target: None,
        id: 0,
        from: 0,
        callee: 1,
        entries: vec![1],
        input_revision: DecimalU64(u64::MAX),
        deliveries: vec![delivery.clone()],
    };
    strict(frame());
    strict(edge);
    strict(transfer);
    strict(node.clone());
    strict(RegionalNode {
        transfer: None,
        ..node.clone()
    });
    strict(delivery);
    strict(call.clone());
    strict(RegionalCfg {
        nodes: vec![node],
        calls: vec![call],
    });
    strict(RegionalCfg::default());
}

#[test]
fn regional_revisions_are_canonical_decimal_strings_and_edge_kinds_are_lossless() {
    for kind in [
        EdgeKind::Fallthrough,
        EdgeKind::Jump,
        EdgeKind::BranchTrue,
        EdgeKind::BranchFalse,
        EdgeKind::Call,
        EdgeKind::Return,
        EdgeKind::Revert,
        EdgeKind::Failure,
        EdgeKind::SummaryReturn,
        EdgeKind::SummaryRevert,
        EdgeKind::SummaryFailure,
    ] {
        strict(RegionalEdge { to: 19, kind });
    }
    let call = RegionalCall {
        action: None,
        target: None,
        id: 0,
        from: 0,
        callee: 0,
        entries: vec![],
        input_revision: DecimalU64(u64::MAX),
        deliveries: vec![],
    };
    let json = serde_json::to_value(&call).unwrap();
    assert_eq!(json["input_revision"], u64::MAX.to_string());
    for invalid in [
        serde_json::json!(u64::MAX),
        serde_json::json!("01"),
        serde_json::json!("18446744073709551616"),
    ] {
        let mut malformed = json.clone();
        malformed["input_revision"] = invalid;
        assert!(serde_json::from_value::<RegionalCall>(malformed).is_err());
    }
}

#[test]
fn process_rules_and_targets_keep_strict_typed_boundaries() {
    let actions = vec![
        ProcessAction::Call,
        ProcessAction::StaticCall,
        ProcessAction::DelegateCall,
        ProcessAction::CallCode,
        ProcessAction::Create,
        ProcessAction::Create2,
    ];
    let mut unresolved = vec![
        ProcessBoundary::CooperativeInputs,
        ProcessBoundary::SymbolicOwner,
        ProcessBoundary::NativeInstantiation,
    ];
    for action in &actions {
        unresolved.push(ProcessBoundary::ResidualOwner(*action));
        unresolved.push(ProcessBoundary::Creation(*action));
    }
    let process = ProcessTransfer {
        repeat: true,
        actions,
        unresolved,
    };
    strict(process.clone());
    strict(RegionalTransfer {
        semantics: TransferSemantics::KnownProcess,
        process: Some(process),
        input_revision: DecimalU64(u64::MAX),
        current: true,
        complete: false,
        visited_block: false,
        successors: vec![],
        calls: vec![],
        outcomes: vec![OutcomeKind::Failure],
    });
    for target in [
        ProcessOwner::Named(frame().code_address),
        ProcessOwner::Residual,
    ] {
        strict(RegionalCall {
            action: Some(ProcessAction::StaticCall),
            target: Some(target),
            id: 0,
            from: 0,
            callee: 1,
            entries: vec![],
            input_revision: DecimalU64(1),
            deliveries: vec![],
        });
    }
    for boundary in ["CooperativeInputs", "SymbolicOwner", "NativeInstantiation"] {
        let encoded = serde_json::json!({"Process": boundary});
        let decoded: FrontierDetails = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    }
    assert_eq!(
        serde_json::to_value(ProcessOwner::Residual).unwrap(),
        "Residual"
    );
    assert!(
        serde_json::from_value::<ProcessBoundary>(serde_json::json!({"Unsupported": "Call"}))
            .is_err()
    );
}
