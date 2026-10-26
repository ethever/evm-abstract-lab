//! Original observations are an inspector subject, independently of execution
//! focus. The reference is qualified by the visible report before borrowing it.

use crate::app::{Workspace, inspector, model::ReportViewId};

pub(in crate::app) enum OriginalSubject {
    Root,
    Minimum(usize),
    Address(String),
}

pub(in crate::app) struct OriginalInspection {
    pub(in crate::app) view: ReportViewId,
    pub(in crate::app) account: OriginalSubject,
}

impl OriginalInspection {
    pub(super) fn borrowed(&self) -> inspector::OriginalAccount<'_> {
        match &self.account {
            OriginalSubject::Root => inspector::OriginalAccount::Root,
            OriginalSubject::Minimum(index) => inspector::OriginalAccount::Minimum(*index),
            OriginalSubject::Address(address) => inspector::OriginalAccount::Address(address),
        }
    }
}

impl Workspace {
    pub(super) fn inspect_original(&mut self, account: OriginalSubject) {
        self.original_inspection = Some(OriginalInspection {
            view: self.report_view.identity(),
            account,
        });
        self.inspector.select_original();
        self.inspector.expanded = true;
    }
}
