//! Consumer fixtures keep graph mutation explicit while encoding real envelopes.
use evm_abstract_protocol::{AnalysisGraph, AnalysisNamespace, AnalysisReport};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fixture {
    pub(crate) model: AnalysisReport,
}

impl Fixture {
    fn into_report(mut self) -> AnalysisReport {
        if self.model.hybrid.is_none() {
            let graph = self.model.selected_graph().unwrap();
            let counters = (
                graph.status,
                graph.metadata.work,
                graph.transfers,
                graph.metadata.postprocessing_work,
            );
            self.model.status = counters.0;
            self.model.work = counters.1;
            self.model.transfers = counters.2;
            self.model.postprocessing_work = counters.3;
        }
        self.model
    }
    pub(crate) fn view(&self) -> crate::report::ReportView<'_> {
        self.view_at(crate::app::model::ReportViewId::fixture(
            1,
            self.model.selected_view,
        ))
    }
    pub(crate) fn view_at(
        &self,
        identity: crate::app::model::ReportViewId,
    ) -> crate::report::ReportView<'_> {
        crate::report::ReportView::new(&self.model, identity, None).unwrap()
    }
    pub(crate) fn graph(
        &self,
        namespace: AnalysisNamespace,
    ) -> Option<crate::report::ReportView<'_>> {
        self.view().graph(namespace)
    }
    pub(crate) fn source(&self, position: usize) -> &evm_abstract_protocol::DisasmBlock {
        self.model
            .source_block(self.cfg[position].source_block().unwrap())
            .unwrap()
    }
    pub(crate) fn source_mut(
        &mut self,
        position: usize,
    ) -> &mut evm_abstract_protocol::DisasmBlock {
        let source = self.cfg[position].source_block().unwrap();
        std::sync::Arc::make_mut(
            self.model
                .programs
                .iter_mut()
                .find(|program| program.id == source.program)
                .unwrap(),
        )
        .blocks
        .iter_mut()
        .find(|block| block.id == source.basic_block)
        .unwrap()
    }
    pub(crate) fn validate(&self) -> Result<(), evm_abstract_protocol::ReportValidationFailure> {
        self.clone().into_report().validate()
    }
}

impl std::ops::Deref for Fixture {
    type Target = AnalysisGraph;
    fn deref(&self) -> &Self::Target {
        self.model.selected_graph().unwrap()
    }
}
impl std::ops::DerefMut for Fixture {
    fn deref_mut(&mut self) -> &mut Self::Target {
        std::sync::Arc::make_mut(
            match self.model.selected_view {
                AnalysisNamespace::Baseline => self.model.global.as_mut(),
                AnalysisNamespace::Native => self.model.native.as_mut(),
            }
            .unwrap(),
        )
    }
}
impl From<AnalysisReport> for Fixture {
    fn from(model: AnalysisReport) -> Self {
        Self { model }
    }
}
impl From<Fixture> for AnalysisReport {
    fn from(fixture: Fixture) -> Self {
        fixture.into_report()
    }
}
impl serde::Serialize for Fixture {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.clone().into_report().serialize(serializer)
    }
}
impl<'de> serde::Deserialize<'de> for Fixture {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        AnalysisReport::deserialize(deserializer).map(Self::from)
    }
}

use evm_abstract_protocol as api;
fn scalar(value: usize) -> api::ValueInfo {
    let text = format!("0x{value:x}");
    api::ValueInfo {
        nonzero: false,
        summary: format!("{{{text}}}"),
        constants: Some(vec![text.clone()]),
        known_zero: "0x0".into(),
        known_one: text.clone(),
        unsigned: api::WordBounds {
            lower: text.clone(),
            upper: text.clone(),
        },
        signed: api::WordBounds {
            lower: text.clone(),
            upper: text.clone(),
        },
        congruence: api::CongruenceValue::Exact(text),
        origins: Some(vec![api::ValueOrigin::Constant]),
        code_address_role: false,
        identity: None,
        expression: None,
        symbolic_limit: false,
    }
}

pub(super) fn attach_snapshots(report: &mut Fixture) {
    let top = report.metadata.environment.call_value.clone();
    report.stores = vec![api::StoreSnapshot {
        storage_facts: vec![],
        transient_facts: vec![],
        storage_default: top.clone(),
        transient_default: scalar(0),
        storage_root: api::SlotStateRef { node: 3 },
        transient_root: api::SlotStateRef { node: 3 },
        nonce_default: top.clone(),
        code_default: None,
        existence_default: api::AccountExistence::Unknown,
        created_default: Some(false),
        pending_destruction_default: Some(false),
        balance_default: top,
        touched_default: Some(false),
        accounts: vec![],
        logs: vec![],
        logs_unknown: false,
    }];
    report.byte_arrays.truncate(1);
    for memory in [true, false] {
        report.byte_arrays.push(api::ByteArraySnapshot {
            symbolic_facts: vec![],
            symbolic: evm_abstract_protocol::ByteViewRef { node: 2 },
            length: scalar(0),
            default: scalar(0),
            values: vec![],
            cells: vec![],
            memory,
        });
    }
    report.states = report
        .cfg
        .iter()
        .map(|block| {
            let program = report
                .model
                .programs
                .iter()
                .find(|program| Some(program.id) == block.program)
                .unwrap();
            let entry = api::MachineSnapshot {
                path: evm_abstract_protocol::PathRef { node: 4 },
                frames: vec![api::FrameSnapshot {
                    process: None,
                    process_operands: None,
                    process_creation: None,
                    index: 0,
                    program: block.program,
                    code_address: block.code_address.clone(),
                    code_hash: Some(program.code_hash.clone()),
                    storage_address: block.storage_address.clone(),
                    address_value: Some(api::AddressValue::Concrete(
                        block.storage_address.clone().unwrap(),
                    )),
                    caller: Some(report.metadata.environment.caller.clone()),
                    address_operand: None,
                    caller_operand: None,
                    call_value: report.metadata.environment.call_value.clone(),
                    is_static: false,
                    kind: program.kind,
                    basic_block: block.basic_block,
                    context: block.context.clone(),
                    stack: vec![scalar(block.id)],
                    memory: Some(1),
                    calldata: Some(0),
                    returndata: Some(2),
                    rollback_store: 0,
                    continuation: None,
                }],
                store: 0,
            };
            api::StateDetails {
                state: block.id,
                exit: Some(entry.clone()),
                entry,
            }
        })
        .collect();
}
