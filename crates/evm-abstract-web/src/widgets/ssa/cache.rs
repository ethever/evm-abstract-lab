//! Rows belong to one explicitly identified report view.
//! Ordinary selection and failed jobs keep the rows;
//! display changes or revealing a hidden instruction rebuild the visible rows.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use crate::report::ReportView;

use super::{Row, build_rows, scoped_states};
use crate::{
    app::{Selection, model::ReportViewId},
    ssa_display::SsaDisplay,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Program(usize),
    NoCode(Option<usize>),
    State {
        program: Option<usize>,
        state: Option<usize>,
    },
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ScopeMode {
    #[default]
    State,
    Source,
}

#[derive(Default)]
pub(crate) struct SsaCache {
    pub(super) mode: ScopeMode,
    identity: ReportViewId,
    scope: Option<Scope>,
    display: Option<SsaDisplay>,
    hidden: BTreeSet<(usize, usize)>,
    revealed: Option<Selection>,
    rows: Vec<Row>,
    columns: usize,
}

impl SsaCache {
    pub(crate) fn select_source_scope(&mut self) {
        self.mode = ScopeMode::Source;
    }

    pub(crate) fn activate(&mut self, identity: ReportViewId) {
        if self.identity != identity {
            *self = Self {
                identity,
                mode: self.mode,
                ..Self::default()
            };
        }
    }

    pub(super) fn prepare(
        &mut self,
        report: &ReportView<'_>,
        program: Option<usize>,
        display: SsaDisplay,
        selection: Selection,
    ) {
        self.activate(report.identity());
        let mut selection = selection;
        selection.value = None;
        let scope = match (self.mode, program) {
            (ScopeMode::State, program) => Scope::State {
                program,
                state: selection.state,
            },
            (ScopeMode::Source, Some(program)) => Scope::Program(program),
            (ScopeMode::Source, None) => Scope::NoCode(selection.state),
        };
        if self.scope != Some(scope) {
            let included = scope.states(report, selection);
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
        let included = scope.states(report, selection);
        self.rows = build_rows(report, display, selection, |state| {
            included.contains(&state)
        });
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

    pub(super) fn full_text(&self, report: &ReportView<'_>, selection: Selection) -> String {
        let Some(scope) = self.scope else {
            return String::new();
        };
        let included = scope.states(report, selection);
        build_rows(report, SsaDisplay::Full, selection, |state| {
            included.contains(&state)
        })
        .iter()
        .map(|row| row.tooltip.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
    }
}

impl Scope {
    fn states(self, report: &ReportView<'_>, selection: Selection) -> BTreeSet<usize> {
        match self {
            Self::Program(program) => scoped_states(report, Some(program), selection),
            Self::NoCode(_) => scoped_states(report, None, selection),
            Self::State { program, state } => report
                .cfg
                .iter()
                .filter(|block| Some(block.id) == state && block.program == program)
                .map(|block| block.id)
                .collect(),
        }
    }
}
