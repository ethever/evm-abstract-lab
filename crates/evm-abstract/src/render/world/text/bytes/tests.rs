use super::write_bytes;
use crate::{
    domain::{Domain, Value},
    world::ByteArray,
};
use alloy_primitives::U256;

fn render(array: &ByteArray) -> String {
    let mut output = String::new();
    write_bytes(&mut output, "  ", "return data", array);
    output
}

fn constant(value: usize) -> Value {
    Value::constant(U256::from(value))
}

#[test]
fn unknown_length_and_default_do_not_erase_known_stored_bytes() {
    let mut array = ByteArray::unknown();
    array
        .write_byte(&constant(7), &constant(0xa5), 8, Domain::default())
        .unwrap();

    let output = render(&array);
    let unknown = format!("0x{}", "*".repeat(64));
    assert!(
        output.contains(&format!("length={unknown} (unknown byte count)")),
        "{output}"
    );
    assert!(
        output.contains(&format!("kind=data | default={unknown}")),
        "{output}"
    );
    assert!(output.contains("0x0007"), "{output}");
    assert!(output.contains("0xa5"), "{output}");
    assert!(!output.contains("exact hex:"), "{output}");
    // byte_at would join zero into this fact because the length is unknown.
    assert!(!output.contains("{0x0, 0xa5}"), "{output}");
}

#[test]
fn a_large_sparse_offset_does_not_expand_the_display_range() {
    let mut array = ByteArray::empty();
    let offset = usize::MAX - 1;
    array
        .write_byte(
            &constant(offset),
            &constant(0x5a),
            usize::MAX,
            Domain::default(),
        )
        .unwrap();

    let output = render(&array);
    assert!(output.contains(&format!("0x{offset:04x}")), "{output}");
    assert!(output.contains("0x5a"), "{output}");
    assert!(
        output.contains(&format!("{} bytes", usize::MAX)),
        "{output}"
    );
    assert!(!output.contains("exact hex:"), "{output}");
    assert!(
        output.len() < 1024,
        "a single sparse byte must stay compact"
    );
}

#[test]
fn a_dense_return_word_keeps_all_thirty_two_bytes_in_order() {
    let mut array = ByteArray::memory();
    array
        .write_word(&constant(0), &constant(1), 32, Domain::default())
        .unwrap();

    let output = render(&array);
    let word = format!("0x{}01", "00".repeat(31));
    assert!(output.contains("length={0x20} (32 bytes)"), "{output}");
    assert!(output.contains("kind=memory | default={0x0}"), "{output}");
    assert!(output.contains("0x0000..=0x001f"), "{output}");
    assert_eq!(output.matches(&word).count(), 2, "{output}");
}

#[test]
fn exact_data_shows_default_zero_bytes_alongside_the_sparse_facts() {
    let mut word = [0_u8; 32];
    word[31] = 1;
    let output = render(&ByteArray::exact(&word));

    assert!(
        output.contains(&format!("exact hex: 0x{}01", "00".repeat(31))),
        "{output}"
    );
    assert!(output.contains("0x001f"), "{output}");
    assert!(output.contains("kind=data | default={0x0}"), "{output}");
}

#[test]
fn empty_data_retains_its_exact_length_and_default() {
    let output = render(&ByteArray::empty());
    assert!(output.contains("length={0x0} (0 bytes)"), "{output}");
    assert!(output.contains("kind=data | default={0x0}"), "{output}");
    assert!(output.contains("exact hex: 0x\n"), "{output}");
    assert!(output.contains("stored byte facts: (none)"), "{output}");
}

#[test]
fn a_set_of_possible_byte_values_does_not_become_one_hex_byte() {
    let domain = Domain::default();
    let value = domain.join(&constant(1), &constant(2));
    let mut array = ByteArray::empty();
    array.write_byte(&constant(0), &value, 1, domain).unwrap();

    let output = render(&array);
    assert!(output.contains("{0x1, 0x2}"), "{output}");
    assert!(!output.contains("exact hex:"), "{output}");
}

#[test]
fn gaps_do_not_join_separate_stored_bytes_into_a_hex_run() {
    let mut array = ByteArray::unknown();
    array
        .write_byte(&constant(0), &constant(1), 3, Domain::default())
        .unwrap();
    array
        .write_byte(&constant(2), &constant(2), 3, Domain::default())
        .unwrap();

    let output = render(&array);
    assert!(output.contains("0x0000"), "{output}");
    assert!(output.contains("0x0002"), "{output}");
    assert!(!output.contains("0x0000..=0x0002"), "{output}");
    assert!(!output.contains("0x0102"), "{output}");
    assert!(!output.contains("0x010002"), "{output}");
}

#[test]
fn long_contiguous_facts_are_chunked_without_dropping_the_tail() {
    let array = ByteArray::exact(&[0xab; 65]);
    let output = render(&array);
    assert!(output.contains("0x0000..=0x001f"), "{output}");
    assert!(output.contains("0x0020..=0x003f"), "{output}");
    assert!(output.contains("0x0040"), "{output}");
    assert_eq!(
        output.matches(&format!("0x{}", "ab".repeat(32))).count(),
        3,
        "{output}"
    );
}

#[test]
fn all_possible_lengths_are_visible_in_hex_and_decimal() {
    let domain = Domain::default();
    let array = ByteArray::exact(&[0x12]).join(&ByteArray::exact(&[0x12, 0x34]), domain);
    let output = render(&array);
    assert!(
        output.contains("length={0x1, 0x2} (possible byte counts: 1, 2)"),
        "{output}"
    );
    assert!(output.contains("{0x0, 0x34}"), "{output}");
}

#[test]
fn a_full_256_byte_sequence_wraps_without_losing_any_exact_bytes() {
    let bytes: Vec<_> = (0_u8..=u8::MAX).collect();
    let output = render(&ByteArray::exact(&bytes));
    let lines: Vec<_> = output
        .lines()
        .skip_while(|line| !line.contains("exact hex: 0x"))
        .take(8)
        .collect();
    assert_eq!(lines.len(), 8, "{output}");

    let mut hex = String::new();
    for (index, line) in lines.into_iter().enumerate() {
        assert!(line.chars().count() <= 90, "{line}");
        let chunk = if index == 0 {
            line.split_once("exact hex: 0x").unwrap().1
        } else {
            line.trim_start()
        };
        assert_eq!(chunk.len(), 64, "{line}");
        assert!(!chunk.contains("0x"), "{line}");
        hex.push_str(chunk);
    }
    let reconstructed: Vec<_> = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
        .collect();
    assert_eq!(reconstructed, bytes);
}
