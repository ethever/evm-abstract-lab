//! The Global view uses canonical procedure/control identities throughout rich
//! export. Its selection never contains a Native state, instruction or SSA ID.

use evm_abstract_protocol::{
    AnalysisReport, MinimumModel, MinimumSummary, ProcessFrameKey, ProcessLocation, ProcessSource,
    SummarySourceStatus,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) summary: usize,
    pub(crate) node: Option<usize>,
    /// A MAY source coordinate inside this regional receipt, never a Native PC.
    pub(crate) control: Option<ProcessLocation>,
    pub(crate) procedure: usize,
    pub(crate) output: Option<usize>,
    pub(crate) account: Option<usize>,
}

impl Selection {
    pub(crate) fn select_summary(&mut self, model: &MinimumModel, summary: usize) {
        self.summary = summary;
        self.procedure = model
            .summaries
            .get(summary)
            .and_then(|summary| summary.relation.root_procedure)
            .unwrap_or(0);
        self.node = model
            .summaries
            .get(summary)
            .and_then(|summary| {
                summary
                    .relation
                    .regional_cfg
                    .nodes
                    .iter()
                    .find(|node| node.procedure == self.procedure)
            })
            .map(|node| node.id);
        self.output = None;
        self.control = None;
    }

    pub(crate) fn select_node(&mut self, model: &MinimumModel, id: usize) {
        if let Some(node) = model.summaries.get(self.summary).and_then(|summary| {
            summary
                .relation
                .regional_cfg
                .nodes
                .iter()
                .find(|node| node.id == id)
        }) {
            self.node = Some(node.id);
            self.procedure = node.procedure;
            self.output = None;
            self.control = None;
            self.account = None;
        }
    }

    pub(crate) fn summary<'a>(&self, model: &'a MinimumModel) -> Option<&'a MinimumSummary> {
        model.summaries.get(self.summary)
    }

    pub(crate) fn source<'a>(
        &self,
        model: &'a MinimumModel,
    ) -> Option<(&'a ProcessFrameKey, ProcessLocation)> {
        let summary = self.summary(model)?;
        let frame = self
            .node
            .and_then(|id| {
                summary
                    .relation
                    .regional_cfg
                    .nodes
                    .iter()
                    .find(|node| node.id == id)
            })
            .and_then(|node| node.frame.process.as_ref())
            .or_else(|| {
                summary
                    .relation
                    .procedures
                    .get(self.procedure)?
                    .process
                    .as_ref()
            })?;
        Some((frame, self.control.unwrap_or(frame.location)))
    }
}

pub(crate) fn initial_summary(model: &MinimumModel) -> usize {
    model
        .summaries
        .iter()
        .rposition(|summary| {
            summary.relation.source_current
                && summary
                    .relation
                    .root_procedure
                    .and_then(|index| summary.relation.procedures.get(index))
                    .and_then(|procedure| procedure.process.as_ref())
                    .is_some_and(|frame| frame.source == model.declared_root_source)
        })
        .or_else(|| {
            model
                .summaries
                .iter()
                .rposition(|summary| summary.relation.source_current)
        })
        .unwrap_or(0)
}

pub(crate) fn source_status(model: &MinimumModel, index: usize) -> SummarySourceStatus {
    let summary = &model.summaries[index].relation;
    summary.source_status(
        model.summaries[index + 1..]
            .iter()
            .any(|later| later.relation.source_state == summary.source_state),
    )
}

pub(super) fn source_program(report: &AnalysisReport, source: &ProcessSource) -> Option<usize> {
    report.process_program(source).map(|program| program.id)
}

pub(crate) fn location_label(location: ProcessLocation) -> String {
    match location {
        ProcessLocation::Block(index) => format!("block index {index}"),
        ProcessLocation::JumpTargets => "unknown-jump hub · all legal JUMPDEST targets".into(),
        ProcessLocation::End => "end of source · no instruction".into(),
        ProcessLocation::Any => "unobserved code · arbitrary actions".into(),
    }
}

pub(super) fn rich_status(report: &AnalysisReport) -> String {
    match &report.rich_export {
        evm_abstract_protocol::RichExportOutcome::NotRequested => {
            "Detailed presentation pending".into()
        }
        evm_abstract_protocol::RichExportOutcome::Complete => "Detailed presentation ready".into(),
        evm_abstract_protocol::RichExportOutcome::Cancelled => {
            "Detailed presentation cancelled".into()
        }
        evm_abstract_protocol::RichExportOutcome::Refused(error) => {
            format!("Detailed presentation refused: {:?}", error.code)
        }
    }
}

impl super::Workspace {
    pub(super) fn minimum_status(&self, report: &AnalysisReport, model: &MinimumModel) -> String {
        format!(
            "{}: Global model {:?}; {} qualified relations; {} original accounts; {} programs; {}; {}; {}; Selected MAY source: {}",
            self.task_label(),
            model.status,
            model.summaries.len(),
            model.accounts.len(),
            report.programs.len(),
            rich_status(report),
            self.graph.accessible_summary(),
            self.inspector.minimum_summary(&self.minimum_selection),
            self.minimum_selection
                .source(model)
                .map_or_else(|| "none".into(), |(_, location)| location_label(location))
        )
    }
}

#[cfg(test)]
mod tests;
