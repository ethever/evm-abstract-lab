//! Aligned tables retain every cell character and wrap long values.

use std::fmt::Write;

/// A table wraps long cells without dropping characters or changing row order.
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
            rows.iter()
                .map(|row| row[column].chars().count())
                .chain([header.chars().count()])
                .max()
                .unwrap_or(1)
                .clamp(1, 72)
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
            .map(|(cell, width)| wrap(cell, *width))
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
    for character in text.chars() {
        if character == '\n' || count == width {
            lines.push(std::mem::take(&mut line));
            count = 0;
            if character == '\n' {
                continue;
            }
        }
        line.push(character);
        count += 1;
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}
