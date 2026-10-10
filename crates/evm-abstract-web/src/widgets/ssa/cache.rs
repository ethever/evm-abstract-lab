//! Rows belong to one immutable analysis report. Workspace clears this cache
//! when replacing the report; selection and failed jobs do not invalidate it.

#[cfg(test)]
mod tests;

use evm_abstract_protocol::AnalysisReport;

use super::{Row, scoped_rows};
use crate::app::Selection;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Program(usize),
    NoCode(Option<usize>),
}

#[derive(Default)]
pub(crate) struct SsaCache {
    scope: Option<Scope>,
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
        selection: Selection,
    ) {
        let scope = match program {
            Some(program) => Scope::Program(program),
            None => Scope::NoCode(selection.state),
        };
        if self.scope == Some(scope) {
            return;
        }
        self.rows = scoped_rows(report, program, selection);
        self.columns = self.rows.iter().map(Row::columns).max().unwrap_or_default();
        self.scope = Some(scope);
    }

    pub(super) fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub(super) fn columns(&self) -> usize {
        self.columns
    }
}
