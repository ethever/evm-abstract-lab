//! Lossless human rendering of sparse byte facts without expanding their range.

use super::write_table;
use crate::{domain::Value, world::ByteArray};
use alloy_primitives::U256;
use std::fmt::Write;

/// Keep abstract length, default, kind, and every explicitly stored byte visible.
/// Hex rows combine only consecutive exact bytes, so gaps and abstract facts
/// cannot silently become a concrete sequence. Work scales with stored facts.
pub(super) fn write_bytes(output: &mut String, indent: &str, label: &str, array: &ByteArray) {
    writeln!(
        output,
        "{indent}{label}: length={} ({})",
        array.len(),
        byte_count(array.len()),
    )
    .unwrap();
    writeln!(
        output,
        "{indent}  kind={} | default={}",
        if array.is_memory() { "memory" } else { "data" },
        array.default_byte(),
    )
    .unwrap();

    // This optional convenience is independently bounded. The sparse table
    // remains authoritative and also includes facts outside any exact length.
    if let Some(bytes) = array.exact_bytes_bounded(256) {
        let prefix = format!("{indent}  exact hex: 0x");
        output.push_str(&prefix);
        let continuation_indent = " ".repeat(prefix.chars().count());
        for (index, byte) in bytes.into_iter().enumerate() {
            if index > 0 && index % 32 == 0 {
                output.push('\n');
                output.push_str(&continuation_indent);
            }
            write!(output, "{byte:02x}").unwrap();
        }
        output.push('\n');
    }

    let mut rows = Vec::new();
    let mut stored = array.stored_bytes().peekable();
    while let Some((start, value)) = stored.next() {
        if let Some(byte) = singleton_byte(value) {
            let mut hex = format!("0x{byte:02x}");
            let mut end = start;
            let mut count = 1;
            while count < 32 {
                let Some(&(next_offset, next_value)) = stored.peek() else {
                    break;
                };
                if end.checked_add(1) != Some(next_offset) {
                    break;
                }
                let Some(next_byte) = singleton_byte(next_value) else {
                    break;
                };
                stored.next();
                write!(hex, "{next_byte:02x}").unwrap();
                end = next_offset;
                count += 1;
            }
            rows.push(vec![offset_range(start, end), hex]);
        } else {
            rows.push(vec![offset_range(start, start), value.to_string()]);
        }
    }

    let table_indent = format!("{indent}  ");
    if rows.is_empty() {
        writeln!(output, "{table_indent}stored byte facts: (none)").unwrap();
    } else {
        write_table(
            output,
            &table_indent,
            &["Offset(s)", "Stored value / hex bytes"],
            &rows,
        );
    }
}

/// Add decimal byte counts without replacing the original abstract length.
fn byte_count(length: &Value) -> String {
    let Some(lengths) = length.constants() else {
        return "unknown byte count".to_owned();
    };
    if lengths.len() == 1 {
        let length = lengths.first().expect("Value constants are nonempty");
        return format!(
            "{length} {}",
            if *length == U256::from(1) {
                "byte"
            } else {
                "bytes"
            }
        );
    }
    format!(
        "possible byte counts: {}",
        lengths
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn singleton_byte(value: &Value) -> Option<u8> {
    let constants = value.constants()?;
    if constants.len() != 1 {
        return None;
    }
    let byte = *constants.first()?;
    (byte <= U256::from(u8::MAX)).then(|| byte.to::<u8>())
}

fn offset_range(start: usize, end: usize) -> String {
    if start == end {
        format!("0x{start:04x}")
    } else {
        format!("0x{start:04x}..=0x{end:04x}")
    }
}

#[cfg(test)]
mod tests;
