//! Display the cached evidence without promoting warnings or unobserved exits
//! into a terminal result. Counts name native states, including mixed states.

use egui::Color32;

use super::super::termination::{TerminationCounts, TerminationSummary};
use crate::palette;

pub(super) fn badges(summary: TerminationSummary) -> Vec<(String, Color32)> {
    if !summary.has_termination() {
        return vec![];
    }
    let mut rows = Vec::with_capacity(3);
    if summary.single_kind().is_none() {
        let text = if summary.continuing > 0 {
            format!("May terminate · continuing {}", summary.continuing)
        } else {
            "Mixed / incomplete termination".into()
        };
        rows.push((text, palette::MUTED));
    }
    for (label, counts) in [("Root end", summary.root), ("Frame return", summary.frame)] {
        let kinds = counted_kinds(counts, false);
        if !kinds.is_empty() {
            rows.push((format!("{label}: {kinds}"), palette::TEXT));
        }
    }
    rows
}

pub(super) fn tooltip(summary: TerminationSummary) -> String {
    format!(
        "Termination evidence (native state counts)\nRoot end: {}\nFrame return: {}\nContinuing {} · unresolved {} · non-current {} · unobserved {}\nRoot outcomes end the root execution. Frame returns resume their caller. Warnings describe incomplete coverage, independently of termination.",
        counted_kinds(summary.root, true),
        counted_kinds(summary.frame, true),
        summary.continuing,
        summary.unresolved,
        summary.non_current,
        summary.unobserved,
    )
}

fn counted_kinds(counts: TerminationCounts, include_zero: bool) -> String {
    [
        ("normal", counts.returns),
        ("REVERT", counts.reverts),
        ("exception", counts.failures),
    ]
    .into_iter()
    .filter(|(_, count)| include_zero || *count > 0)
    .map(|(label, count)| format!("{label} {count}"))
    .collect::<Vec<_>>()
    .join(" · ")
}
