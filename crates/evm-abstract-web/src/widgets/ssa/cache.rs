//! Rows belong to one immutable analysis report. Workspace clears this cache
//! when replacing the report. Ordinary selection and failed jobs keep the rows;
//! display changes or revealing a hidden instruction rebuild the visible rows.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use evm_abstract_protocol::AnalysisReport;

use super::{Row, scoped_rows, scoped_states};
use crate::{app::Selection, ssa_display::SsaDisplay};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Program(usize),
    NoCode(Option<usize>),
}

#[derive(Default)]
pub(crate) struct SsaCache {
    scope: Option<Scope>,
    display: Option<SsaDisplay>,
    hidden: BTreeSet<(usize, usize)>,
    revealed: Option<Selection>,
    rows: Vec<Row>,
    columns: usize,
}

impl SsaCache {
    /// Must be called before using a newly accepted report, even when its
    /// program IDs match the previous report. Retained reports keep their rows.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn prepare(
        &mut self,
        report: &AnalysisReport,
        program: Option<usize>,
        display: SsaDisplay,
        selection: Selection,
    ) {
        let mut selection = selection;
        selection.value = None;
        let scope = match program {
            Some(program) => Scope::Program(program),
            None => Scope::NoCode(selection.state),
        };
        if self.scope != Some(scope) {
            let included = scoped_states(report, program, selection);
            // Index only instructions the compact view actually suppresses.
            // A normal selection then needs a lookup, not a full report scan.
            self.hidden = report
                .ssa
                .blocks
                .iter()
                .filter(|block| included.contains(&block.state))
                .flat_map(|block| {
                    block.instructions.iter().filter_map(|instruction| {
                        (!SsaDisplay::Compact.instruction_visible(
                            block.coverage,
                            instruction,
                            false,
                        ))
                        .then_some((block.state, instruction.pc))
                    })
                })
                .collect();
        }
        let revealed = if display == SsaDisplay::Compact {
            selection
                .state
                .zip(selection.pc)
                .filter(|target| self.hidden.contains(target))
                .map(|_| selection)
        } else {
            None
        };
        if self.scope == Some(scope) && self.display == Some(display) && self.revealed == revealed {
            return;
        }
        self.rows = scoped_rows(report, program, display, selection);
        self.columns = self.rows.iter().map(Row::columns).max().unwrap_or_default();
        self.scope = Some(scope);
        self.display = Some(display);
        self.revealed = revealed;
    }

    pub(super) fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub(super) fn columns(&self) -> usize {
        self.columns
    }
}
