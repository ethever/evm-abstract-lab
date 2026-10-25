//! Strict JSON contracts around standard derived and retained codecs.

use evm_abstract_protocol::{BytecodeFailure, Fork, Frontier, HexDigitFailure};

#[test]
fn external_variant_wire_shapes_remain_exact() {
    for (wire, expected) in [
        (r#""UnsupportedEof""#, BytecodeFailure::UnsupportedEof),
        (
            r#"{"UnsupportedEof":null}"#,
            BytecodeFailure::UnsupportedEof,
        ),
        (r#"{"OddHexLength":3}"#, BytecodeFailure::OddHexLength(3)),
        (
            r#"{"InvalidHex":{"index":2,"character":"x"}}"#,
            BytecodeFailure::InvalidHex(HexDigitFailure {
                index: 2,
                character: 'x',
            }),
        ),
    ] {
        assert_eq!(
            serde_json::from_str::<BytecodeFailure>(wire).unwrap(),
            expected
        );
    }
    assert_eq!(
        serde_json::to_string(&BytecodeFailure::UnsupportedEof).unwrap(),
        r#""UnsupportedEof""#,
    );
    assert_eq!(
        serde_json::to_string(&BytecodeFailure::OddHexLength(3)).unwrap(),
        r#"{"OddHexLength":3}"#,
    );
}

#[test]
fn external_variants_reject_missing_extra_duplicate_and_wrong_payloads() {
    for wire in [
        r#""OddHexLength""#,
        r#"{"OddHexLength":null}"#,
        r#"{"OddHexLength":"3"}"#,
        r#"{"UnsupportedEof":0}"#,
        r#"{"UnsupportedEof":null,"UnsupportedEof":null}"#,
        r#"{"UnsupportedEof":null,"OddHexLength":3}"#,
        r#"{"Unknown":null}"#,
        r#"{"InvalidHex":{"index":2}}"#,
        r#"{"InvalidHex":{"index":2,"index":3,"character":"x"}}"#,
        r#"{"InvalidHex":{"index":2,"character":"x","extra":0}}"#,
        r#"{"InvalidHex":[2,"x"]}"#,
        "{}",
        "null",
        "0",
        r#"["UnsupportedEof"]"#,
    ] {
        assert!(
            serde_json::from_str::<BytecodeFailure>(wire).is_err(),
            "{wire}"
        );
    }
}

#[test]
fn records_remain_map_only_and_nullable_fields_remain_required() {
    let valid = r#"{"from":null,"pc":null,"kind":"Memory","reason":"Memory","detail":"bounded","range":null,"payload":null}"#;
    assert!(serde_json::from_str::<Frontier>(valid).is_ok());
    for invalid in [
        r#"[null,null,"Memory","Memory","bounded",null,null]"#.to_owned(),
        valid.replace(r#""pc":null,"#, ""),
        valid.replace(r#""pc":null"#, r#""pc":null,"pc":null"#),
        valid.replace(r#""pc":null"#, r#""pc":null,"extra":0"#),
    ] {
        assert!(
            serde_json::from_str::<Frontier>(&invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn unit_choices_remain_strings_only() {
    assert_eq!(
        serde_json::from_str::<Fork>(r#""Cancun""#).unwrap(),
        Fork::Cancun
    );
    for invalid in [
        r#"{"Cancun":null}"#,
        r#"["Cancun"]"#,
        r#""cancun""#,
        "null",
        "0",
    ] {
        assert!(serde_json::from_str::<Fork>(invalid).is_err(), "{invalid}");
    }
}
