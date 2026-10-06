//! Aligned tables retain every cell character and wrap long values.

use std::fmt::Write;

/// A table wraps long cells without dropping characters or changing row order.
/// Full 256-bit word patterns stay on one line, including partial-bit nibbles
/// and an immediately preceding `bits=` label.
pub(super) fn write_table(
    output: &mut String,
    indent: &str,
    headers: &[&str],
    rows: &[Vec<String>],
) {
    if rows.is_empty() {
        writeln!(output, "{indent}(none)").unwrap();
        return;
    }
    let widths = headers
        .iter()
        .enumerate()
        .map(|(column, header)| {
            let (cell_width, word_width) = rows
                .iter()
                .map(|row| row[column].as_str())
                .chain([*header])
                .fold((1, 0), |(cell_width, word_width), cell| {
                    (
                        cell_width.max(cell.chars().count()),
                        word_width.max(max_word_pattern_width(cell)),
                    )
                });
            cell_width.clamp(1, 72).max(word_width)
        })
        .collect::<Vec<_>>();
    write_row(
        output,
        indent,
        &headers
            .iter()
            .map(|header| (*header).to_owned())
            .collect::<Vec<_>>(),
        &widths,
    );
    write_row(
        output,
        indent,
        &widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>(),
        &widths,
    );
    for row in rows {
        let cells = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| wrap(cell, (*width).min(72)))
            .collect::<Vec<_>>();
        let height = cells.iter().map(Vec::len).max().unwrap_or(1);
        for line in 0..height {
            let visible = cells
                .iter()
                .map(|cell| cell.get(line).cloned().unwrap_or_default())
                .collect::<Vec<_>>();
            write_row(output, indent, &visible, &widths);
        }
    }
}

fn write_row(output: &mut String, indent: &str, cells: &[String], widths: &[usize]) {
    output.push_str(indent);
    for (index, (cell, width)) in cells.iter().zip(widths).enumerate() {
        if index > 0 {
            output.push_str(" | ");
        }
        write!(output, "{cell:<width$}").unwrap();
    }
    output.push('\n');
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut count = 0;
    let mut remaining = text;
    while !remaining.is_empty() {
        let word_length = word_pattern_length(remaining);
        let length = word_length.unwrap_or_else(|| remaining.chars().next().unwrap().len_utf8());
        let part = &remaining[..length];
        let part_width = word_length.unwrap_or(1);
        remaining = &remaining[length..];
        if part == "\n" || (count > 0 && count + part_width > width) {
            lines.push(std::mem::take(&mut line));
            count = 0;
            if part == "\n" {
                continue;
            }
        }
        line.push_str(part);
        count += part_width;
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

fn max_word_pattern_width(text: &str) -> usize {
    let mut maximum = 0;
    let mut remaining = text;
    while !remaining.is_empty() {
        let length = if let Some(length) = word_pattern_length(remaining) {
            maximum = maximum.max(length);
            length
        } else {
            remaining.chars().next().unwrap().len_utf8()
        };
        remaining = &remaining[length..];
    }
    maximum
}

/// ASCII word tokens have exactly 64 nibble positions; each partial position
/// occupies six characters. Keep this unit intact without changing ordinary
/// Unicode text wrapping or treating a longer hex sequence as a word prefix.
/// The optional `bits=` label belongs to the same token for readable values.
fn word_pattern_length(text: &str) -> Option<usize> {
    let (text, label_length) = text
        .strip_prefix("bits=")
        .map_or((text, 0), |pattern| (pattern, 5));
    let bytes = text.strip_prefix("0x")?.as_bytes();
    let mut length = 0;
    for _ in 0..64 {
        match *bytes.get(length)? {
            b'0'..=b'9' | b'a'..=b'f' | b'*' => length += 1,
            b'[' => {
                let nibble = bytes.get(length..length + 6)?;
                if nibble[5] != b']'
                    || !nibble[1..5]
                        .iter()
                        .all(|bit| matches!(bit, b'0' | b'1' | b'*'))
                {
                    return None;
                }
                length += 6;
            }
            _ => return None,
        }
    }
    if bytes
        .get(length)
        .is_some_and(|next| matches!(next, b'0'..=b'9' | b'a'..=b'f' | b'*' | b'['))
    {
        return None;
    }
    Some(length + 2 + label_length)
}
