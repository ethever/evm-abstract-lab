//! Regional evidence keeps its own strict namespace and lossless source revisions.
use evm_abstract_protocol::*;

fn frame() -> RegionalFrame {
    RegionalFrame {
        process: None,
        code_address: Some("0x0000000000000000000000000000000000000102".into()),
        code_hash: Some(format!("0x{}", "ab".repeat(32))),
        storage_address: Some("0x0000000000000000000000000000000000000101".into()),
        address_value: Some(AddressValue::Symbolic(InputSymbol {
            kind: InputSymbolKind::To,
            index: None,
        })),
        caller: Some(AddressValue::Symbolic(InputSymbol {
            kind: InputSymbolKind::Caller,
            index: None,
        })),
        kind: CodeKind::Runtime,
        is_static: true,
        basic_block: Some(7),
        stack_height: Some(2),
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
        creation: vec![],
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
        scope: None,
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
        EdgeKind::ProcessControl,
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
        scope: None,
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
fn process_locations_and_shared_control_records_have_one_required_typed_shape() {
    for point in [
        ProcessLocation::Any,
        ProcessLocation::Block(7),
        ProcessLocation::JumpTargets,
        ProcessLocation::End,
    ] {
        let wire = serde_json::to_vec(&point).unwrap();
        assert_eq!(
            serde_json::from_slice::<ProcessLocation>(&wire).unwrap(),
            point
        );
    }
    for invalid in [
        r#"{"Block":-1}"#,
        r#"{"Block":"7"}"#,
        r#""NativePC""#,
        r#"{"Block":7,"End":null}"#,
    ] {
        assert!(serde_json::from_str::<ProcessLocation>(invalid).is_err());
    }
    strict(ProcessControlEdge {
        to: ProcessLocation::JumpTargets,
        kind: EdgeKind::Jump,
    });
    let block = ProcessControlBlock {
        successors: [
            Some(ProcessControlEdge {
                to: ProcessLocation::Block(1),
                kind: EdgeKind::BranchTrue,
            }),
            None,
        ],
        continuation: Some(ProcessLocation::End),
        boundary: false,
    };
    strict(block.clone());
    strict(ProcessControlRelation {
        id: 0,
        epoch: ProcessDigest([1; 32]),
        source: ProcessSource::InputProgram(ProcessInputProgramSource {
            hash: ProcessDigest([2; 32]),
            fork: Fork::Osaka,
        }),
        program: Some(3),
        blocks: vec![block],
        jump_targets: vec![0],
    });
    strict(ProcessControlRegion {
        relation: 0,
        entry: ProcessLocation::Block(0),
        through: vec![ProcessLocation::Block(0)],
        boundaries: vec![ProcessLocation::JumpTargets],
    });
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
        control: None,
        creations: vec![],
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
        ProcessOwner::Named(frame().code_address.unwrap()),
        ProcessOwner::Residual,
    ] {
        strict(RegionalCall {
            scope: None,
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

#[test]
fn process_frames_preserve_full_epoch_and_source_without_native_anchors() {
    let fork = ProcessForkSource { fork: Fork::Osaka };
    let captured = ProcessCapturedSource {
        source: frame().code_address.unwrap(),
        hash: ProcessDigest([0xab; 32]),
        fork: Fork::Osaka,
    };
    let sources = [
        ProcessSource::Runtime(captured.clone()),
        ProcessSource::InitCode(ProcessInitCodeSource {
            hash: captured.hash,
            fork: Fork::Osaka,
        }),
        ProcessSource::AnyRuntime(fork.clone()),
        ProcessSource::AnyInitCode(fork.clone()),
        ProcessSource::Empty(fork),
        ProcessSource::Precompile(ProcessPrecompileSource {
            address: "0x0000000000000000000000000000000000000004".into(),
            fork: Fork::Osaka,
        }),
        ProcessSource::InvalidDelegation(captured),
    ];
    for source in sources {
        let key = ProcessFrameKey {
            location: ProcessLocation::Any,
            epoch: ProcessDigest([0xfe; 32]),
            owner: ProcessOwner::Residual,
            source,
            is_static: true,
        };
        strict(key.clone());
        let value = serde_json::to_value(&key).unwrap();
        assert_eq!(value["owner"], "Residual");
        assert_eq!(value["epoch"], format!("0x{}", "fe".repeat(32)));
        for forbidden in [
            "code_address",
            "code_hash",
            "storage_address",
            "address_value",
            "caller",
            "basic_block",
            "stack_height",
            "context",
        ] {
            assert!(value.get(forbidden).is_none());
            let mut invalid = value.clone();
            invalid[forbidden] = serde_json::json!(0);
            assert!(
                serde_json::from_value::<ProcessFrameKey>(invalid).is_err(),
                "{forbidden}"
            );
        }
        let encoded = serde_json::to_value(&key.source).unwrap();
        if matches!(
            key.source,
            ProcessSource::AnyRuntime(_)
                | ProcessSource::AnyInitCode(_)
                | ProcessSource::Empty(_)
                | ProcessSource::Precompile(_)
        ) {
            let payload = encoded.as_object().unwrap().values().next().unwrap();
            assert!(payload.get("hash").is_none() && payload.get("source").is_none());
        }
    }
}

#[test]
fn process_epoch_never_truncates_and_constructor_results_are_class_valued() {
    let mut other = [0u8; 32];
    other[0] = 1;
    assert_ne!(ProcessDigest([0; 32]), ProcessDigest(other));
    for digest in [
        ProcessDigest([0; 32]),
        ProcessDigest(other),
        ProcessDigest([0xff; 32]),
    ] {
        assert_eq!(
            serde_json::from_value::<ProcessDigest>(serde_json::to_value(digest).unwrap()).unwrap(),
            digest
        );
    }
    for invalid in [
        serde_json::json!(1),
        serde_json::json!("0x01"),
        serde_json::json!(format!("0x{}", "AB".repeat(32))),
        serde_json::json!("0x😀"),
    ] {
        assert!(serde_json::from_value::<ProcessDigest>(invalid).is_err());
    }
    strict(CreationContinuation::Native(NativeCreation {
        destination: frame().storage_address.unwrap(),
    }));
    strict(CreationContinuation::Process(ProcessCreation {
        action: ProcessAction::Create2,
        creator: ProcessOwner::Residual,
        destination: ProcessOwner::Residual,
        epoch: ProcessDigest(other),
    }));
    let source = ProcessSource::AnyInitCode(ProcessForkSource { fork: Fork::Osaka });
    assert_eq!(source.kind(), CodeKind::Initcode);
    assert_eq!(source.semantics(), TransferSemantics::ConstructorProcess);
    for result in [
        ProcessCreationResult::Address(ProcessOwner::Residual),
        ProcessCreationResult::Zero,
    ] {
        assert_eq!(
            serde_json::from_value::<ProcessCreationResult>(serde_json::to_value(&result).unwrap())
                .unwrap(),
            result
        );
    }
    for invalid in [
        serde_json::json!(1),
        serde_json::json!(true),
        serde_json::json!("0x1"),
    ] {
        assert!(serde_json::from_value::<ProcessCreationResult>(invalid).is_err());
    }
}
