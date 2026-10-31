//! Raw input loading is independent of fork-aware EVM decoding.
use evm_bytecode::{DecodeError, Fork, Program, parse_hex};

#[test]
fn raw_hex_loading_defers_evm_format_interpretation_to_the_consumer() {
    assert_eq!(parse_hex(" 0x ef\n00 ").unwrap(), [0xef, 0x00]);
    assert!(matches!(
        Program::from_hex_with_fork("ef00", Fork::Osaka),
        Err(DecodeError::UnsupportedEof)
    ));
    let delegation = format!("ef0100{}", "11".repeat(20));
    assert_eq!(parse_hex(&delegation).unwrap().len(), 23);
    assert!(matches!(
        Program::from_hex_with_fork(&delegation, Fork::Osaka),
        Err(DecodeError::DelegatedCode { .. })
    ));
}

#[test]
fn shared_hex_syntax_and_program_input_keep_their_existing_rules() {
    for source in ["", "0x", "60 02\n60 03\t01 00", "\u{2003}0x600200\u{2003}"] {
        let bytes = parse_hex(source).unwrap();
        assert_eq!(Program::from_hex(source).unwrap().bytes(), bytes);
    }
    assert!(matches!(parse_hex("0"), Err(DecodeError::OddHexLength(1))));
    assert!(matches!(
        parse_hex("0xgg"),
        Err(DecodeError::InvalidHex {
            index: 0,
            character: 'g'
        })
    ));
}
