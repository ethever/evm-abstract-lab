//! Strict wire identities and terminal records need no execution engine.
use evm_abstract_protocol::*;

#[test]
fn process_identities_are_canonical_decimal_strings_over_the_full_u64_interval() {
    for value in [
        0,
        1,
        u64::from(u32::MAX) + 1,
        9_007_199_254_740_991,
        9_007_199_254_740_992,
        9_007_199_254_740_993,
        u64::MAX,
    ] {
        let identity = DecimalU64(value);
        let encoded = serde_json::to_string(&identity).unwrap();
        assert_eq!(encoded, format!("\"{value}\""));
        assert_eq!(
            serde_json::from_str::<DecimalU64>(&encoded).unwrap(),
            identity
        );
        let input = InputIdentity {
            scope: identity,
            symbol: InputSymbol {
                kind: InputSymbolKind::CallValue,
                index: None,
            },
        };
        let encoded = serde_json::to_string(&input).unwrap();
        assert!(encoded.contains(&format!("\"scope\":\"{value}\"")));
        assert_eq!(
            serde_json::from_str::<InputIdentity>(&encoded).unwrap(),
            input
        );
    }
    let adjacent = [
        9_007_199_254_740_991,
        9_007_199_254_740_992,
        9_007_199_254_740_993,
    ]
    .map(|value| InputIdentity {
        scope: DecimalU64(value),
        symbol: InputSymbol {
            kind: InputSymbolKind::CallValue,
            index: None,
        },
    });
    let encoded = adjacent
        .each_ref()
        .map(|input| serde_json::to_string(input).unwrap());
    for first in 0..adjacent.len() {
        for second in first + 1..adjacent.len() {
            assert_ne!(adjacent[first], adjacent[second]);
            assert_ne!(encoded[first], encoded[second]);
            assert_ne!(
                serde_json::from_str::<InputIdentity>(&encoded[first]).unwrap(),
                serde_json::from_str::<InputIdentity>(&encoded[second]).unwrap()
            );
        }
    }
    for invalid in [
        "0",
        "18446744073709551615",
        "null",
        "true",
        "\"\"",
        "\"00\"",
        "\"01\"",
        "\"+1\"",
        "\"-1\"",
        "\"1.0\"",
        "\"1e0\"",
        "\" 1\"",
        "\"1 \"",
        "\"0x1\"",
        "\"١\"",
        "\"18446744073709551616\"",
    ] {
        assert!(
            serde_json::from_str::<DecimalU64>(invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn offline_labels_do_not_replace_namespace_identity_and_records_remain_strict() {
    let snapshot = RootTerminalSnapshot::Offline(RootTerminalOfflineSnapshot {
        label: "same label".into(),
        namespace: DecimalU64(u64::MAX),
    });
    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert_eq!(
        serde_json::from_str::<RootTerminalSnapshot>(&encoded).unwrap(),
        snapshot
    );
    for (invalid, expected) in [
        (
            encoded.replace("\"namespace\":\"18446744073709551615\"", "\"unused\":1"),
            "unknown RootTerminalOfflineSnapshot field: unused",
        ),
        (
            encoded.replace(",\"namespace\":\"18446744073709551615\"", ""),
            "missing field: namespace",
        ),
        (
            encoded.replace("\"label\":", "\"namespace\":\"1\",\"label\":"),
            "duplicate field: namespace",
        ),
    ] {
        let error = serde_json::from_str::<RootTerminalSnapshot>(&invalid).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    let other = RootTerminalSnapshot::Offline(RootTerminalOfflineSnapshot {
        label: "same label".into(),
        namespace: DecimalU64(u64::MAX - 1),
    });
    assert_ne!(snapshot, other);
}

#[test]
fn terminal_triggers_and_joint_outcomes_use_strict_typed_records_without_state_pcs() {
    for trigger in [
        RootTerminalTrigger::Work,
        RootTerminalTrigger::SummaryWork,
        RootTerminalTrigger::States,
        RootTerminalTrigger::Transfers,
        RootTerminalTrigger::MandatoryRetention(RootTerminalRetentionTrigger {
            state: 12,
            pc: 34,
            kind: RootTerminalRetention::Expression,
        }),
        RootTerminalTrigger::MandatoryRetention(RootTerminalRetentionTrigger {
            state: 56,
            pc: 78,
            kind: RootTerminalRetention::State,
        }),
    ] {
        let encoded = serde_json::to_string(&trigger).unwrap();
        assert_eq!(
            serde_json::from_str::<RootTerminalTrigger>(&encoded).unwrap(),
            trigger
        );
    }
    let valid = r#"{"kind":"Return","data":1,"store":2}"#;
    let outcome: RootTerminalOutcome = serde_json::from_str(valid).unwrap();
    assert_eq!(outcome.kind, OutcomeKind::Return);
    for (invalid, expected) in [
        (valid.replace("\"data\":1,", ""), "missing field: data"),
        (
            valid.replace("\"store\":2", "\"store\":2,\"state\":0"),
            "unknown RootTerminalOutcome field: state",
        ),
        (
            valid.replace("\"store\":2", "\"store\":2,\"pc\":0"),
            "unknown RootTerminalOutcome field: pc",
        ),
    ] {
        let error = serde_json::from_str::<RootTerminalOutcome>(&invalid).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    let three = format!("[{valid},{valid},{valid}]");
    assert!(serde_json::from_str::<[RootTerminalOutcome; 3]>(&three).is_ok());
    for invalid in [
        format!("[{valid},{valid}]"),
        format!("[{valid},{valid},{valid},{valid}]"),
    ] {
        assert!(serde_json::from_str::<[RootTerminalOutcome; 3]>(&invalid).is_err());
    }
}

#[test]
fn opt_in_is_required_on_the_wire_and_defaults_to_false_only_in_rust_defaults() {
    assert!(!AnalyzeRequest::default().limits.root_terminal_coverage);
    let encoded = serde_json::to_string(&AnalysisLimits::default()).unwrap();
    let missing = encoded.replace("\"root_terminal_coverage\":false,", "");
    let error = serde_json::from_str::<AnalysisLimits>(&missing).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("missing field: root_terminal_coverage")
    );
}
