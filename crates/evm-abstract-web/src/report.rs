//! A borrowed display boundary over one immutable publication. Machine, SSA and
//! pool IDs resolve only in the selected graph; source code uses the shared catalog.

use crate::app::model::ReportViewId;
use evm_abstract_protocol::{
    AnalysisGraph, AnalysisNamespace, AnalysisReport, CfgBlock, DisasmBlock, DisasmInstruction,
    HybridReport, JobPublication, ProgramInfo,
};

#[derive(Clone, Copy)]
pub(crate) struct ReportView<'a> {
    pub(crate) model: &'a AnalysisReport,
    pub(crate) programs: &'a [std::sync::Arc<ProgramInfo>],
    pub(crate) acquisition: &'a Option<evm_abstract_protocol::AcquisitionReport>,
    pub(crate) disassembly: &'a [DisasmBlock],
    pub(crate) hybrid: &'a Option<HybridReport>,
    graph: &'a AnalysisGraph,
    identity: ReportViewId,
    publication: Option<&'a JobPublication>,
}

impl<'a> ReportView<'a> {
    pub(crate) fn new(
        model: &'a AnalysisReport,
        identity: ReportViewId,
        publication: Option<&'a JobPublication>,
    ) -> Option<Self> {
        if identity.is_minimum() {
            return None;
        }
        let namespace = identity.namespace();
        Some(Self {
            model,
            programs: &model.programs,
            acquisition: &model.acquisition,
            disassembly: model.disassembly(namespace).unwrap_or_default(),
            hybrid: &model.hybrid,
            graph: model.graph(namespace)?,
            identity,
            publication,
        })
    }

    pub(crate) fn data(&self) -> &'a AnalysisGraph {
        self.graph
    }

    pub(crate) fn identity(&self) -> ReportViewId {
        self.identity
    }

    pub(crate) fn graph(&self, namespace: AnalysisNamespace) -> Option<Self> {
        Self::new(
            self.model,
            self.identity.for_namespace(namespace, self.publication),
            self.publication,
        )
    }

    pub(crate) fn instructions(&self, state: &CfgBlock) -> &'a [DisasmInstruction] {
        state
            .source_block()
            .and_then(|source| self.model.source_block(source))
            .map_or(&[], |block| block.instructions.as_slice())
    }
}

impl std::ops::Deref for ReportView<'_> {
    type Target = AnalysisGraph;
    fn deref(&self) -> &Self::Target {
        self.graph
    }
}

pub(crate) fn regional_status(
    node: &evm_abstract_protocol::RegionalNode,
    source: evm_abstract_protocol::SummarySourceStatus,
) -> &'static str {
    match source {
        evm_abstract_protocol::SummarySourceStatus::Historical => return "Historical observation",
        evm_abstract_protocol::SummarySourceStatus::Uncertified => {
            return "Final source uncertified";
        }
        evm_abstract_protocol::SummarySourceStatus::Current => {}
    }
    if node.excluded {
        return "Excluded input";
    }
    match &node.transfer {
        None => "Admitted; transfer pending",
        Some(transfer) if !transfer.current || transfer.input_revision != node.input_revision => {
            "Historical transfer"
        }
        Some(transfer)
            if !transfer.complete
                && transfer.semantics == evm_abstract_protocol::TransferSemantics::KnownProcess =>
        {
            "Current partial conservative process"
        }
        Some(transfer) if !transfer.complete => "Current partial transfer",
        Some(transfer)
            if transfer.semantics == evm_abstract_protocol::TransferSemantics::KnownProcess =>
        {
            "Current conservative process · possible control"
        }
        Some(transfer) if transfer.visited_block => "Current block transfer",
        Some(_) if node.frame.kind == evm_abstract_protocol::CodeKind::AbstractEnvironment => {
            "Current effect/environment abstraction"
        }
        Some(_) => "Current transfer; no captured block visit",
    }
}
