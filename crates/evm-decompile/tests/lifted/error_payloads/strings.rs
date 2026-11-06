use super::*;

#[test]
fn real_constant_mstore_words_cover_twenty_plus_twelve_and_thirty_one_plus_one() {
    assert_eq!((DS_MATH.len(), TRANSFER.len()), (20, 31));
    for message in [DS_MATH, TRANSFER] {
        let input = fixtures::standalone(message, Mutation::None);
        let artifact = checked(&input);
        assert_message(&artifact, message);
        assert_eq!(lifted(&artifact).1.constructions.len(), 1);
        let text = render_text(&artifact);
        let expected = format!(
            "revert({});",
            serde_json::to_string(std::str::from_utf8(message).unwrap()).unwrap()
        );
        assert_eq!(text.matches(&expected).count(), 1, "{text}");
        let (outcome, host, capsules) = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, Vec::new()),
            1_048_576,
        );
        assert_eq!(
            oracle::reverted(&outcome.unwrap(), &host),
            Some(oracle::abi(message))
        );
        assert_eq!(capsules, 1);
    }
}

#[test]
fn empty_multiword_and_escaped_utf8_messages_keep_exact_byte_identity() {
    for message in [
        b"".as_slice(),
        b"a whole first ABI word and a second complete word plus a final suffix".as_slice(),
        "quote\" slash\\ newline\n nul\0 中文".as_bytes(),
    ] {
        let input = fixtures::standalone(message, Mutation::None);
        let artifact = checked(&input);
        assert_message(&artifact, message);
        let (outcome, host, _) = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, Vec::new()),
            1_048_576,
        );
        let bytes = oracle::reverted(&outcome.unwrap(), &host).unwrap();
        assert_eq!(bytes, oracle::abi(message));
        assert!(
            !bytes.is_empty(),
            "Error(\"\") is distinct from an empty REVERT"
        );
        let (stage, program) = lifted(&artifact);
        let rendered = program
            .render_index(&stage.ir)
            .render_revert(program.reverts[0].block, |_| true)
            .unwrap()
            .unwrap();
        assert_eq!(
            rendered,
            format!(
                "revert({});",
                serde_json::to_string(std::str::from_utf8(message).unwrap()).unwrap()
            )
        );
    }
}

#[test]
fn byte_overlaps_are_last_write_wins_and_padding_repairs_are_exact() {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    begin(&mut asm, DS_MATH);
    fixtures::mutate(&mut asm, Mutation::NonzeroPadding, DS_MATH.len());
    asm.number(0)
        .op(0x81)
        .number(68 + u64::try_from(DS_MATH.len()).unwrap())
        .op(0x01)
        .op(0x53)
        .number(u64::from(b'S'))
        .op(0x81)
        .number(69)
        .op(0x01)
        .op(0x53);
    end(&mut asm, DS_MATH.len());
    let input = asm.finish();
    let artifact = checked(&input);
    let mut message = DS_MATH.to_vec();
    message[1] = b'S';
    assert_message(&artifact, &message);
    let (outcome, host, _) = oracle::certify(
        &input,
        &artifact,
        MemoryHost::new(&input, Vec::new()),
        1_048_576,
    );
    assert_eq!(
        oracle::reverted(&outcome.unwrap(), &host),
        Some(oracle::abi(&message))
    );
}
