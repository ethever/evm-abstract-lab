//! Positions and row offsets are built once per publication, outside paint.

use super::{CodeGroup, ContextTarget, Presentation, ProgramRows, PublicationIdentity, SourceRow};
use crate::app::model::{
    ReportViewId,
    navigation::{ContextFocus, SourceFocus, SourceId},
};
use evm_abstract_protocol::{
    AnalysisGraph, AnalysisNamespace, AnalysisReport, DisasmBlock, JobId, JobPublication,
    ProcessSource, ProgramInfo, SummarySourceStatus,
};
use std::collections::BTreeMap;

impl ProgramRows {
    fn new(program: &ProgramInfo) -> Self {
        let mut starts = Vec::with_capacity(program.blocks.len() + 1);
        starts.push(0);
        for block in &program.blocks {
            starts.push(starts.last().copied().unwrap() + block.instructions.len() + 1);
        }
        Self { starts }
    }

    pub(crate) fn len(&self) -> usize {
        self.starts.last().copied().unwrap_or(0)
    }

    pub(crate) fn block_row(&self, index: usize) -> Option<usize> {
        (index < self.starts.len().saturating_sub(1)).then(|| self.starts[index])
    }

    pub(crate) fn row<'a>(&self, program: &'a ProgramInfo, row: usize) -> Option<SourceRow<'a>> {
        if row >= self.len() {
            return None;
        }
        let index = self
            .starts
            .partition_point(|&start| start <= row)
            .checked_sub(1)?;
        let block = program.blocks.get(index)?;
        let offset = row - self.starts[index];
        Some(if offset == 0 {
            SourceRow::Block(block)
        } else {
            SourceRow::Instruction {
                block,
                instruction: block.instructions.get(offset - 1)?,
            }
        })
    }
}

struct ProgramIndex {
    position: usize,
    rows: ProgramRows,
    blocks: BTreeMap<usize, usize>,
}

#[derive(Default)]
pub(super) struct GraphIndex {
    pub(super) cfg: BTreeMap<usize, usize>,
    pub(super) states: BTreeMap<usize, usize>,
    pub(super) ssa: BTreeMap<usize, usize>,
    pub(super) values: BTreeMap<usize, usize>,
}

impl GraphIndex {
    fn new(graph: &AnalysisGraph) -> Self {
        Self {
            cfg: graph
                .cfg
                .iter()
                .enumerate()
                .map(|(index, state)| (state.id, index))
                .collect(),
            states: graph
                .states
                .iter()
                .enumerate()
                .map(|(index, state)| (state.state, index))
                .collect(),
            ssa: graph
                .ssa
                .blocks
                .iter()
                .enumerate()
                .map(|(index, block)| (block.state, index))
                .collect(),
            values: graph
                .ssa
                .values
                .iter()
                .enumerate()
                .map(|(index, value)| (value.id, index))
                .collect(),
        }
    }
}

pub(super) struct MinimumIndex {
    pub(super) nodes: BTreeMap<usize, usize>,
    pub(super) status: SummarySourceStatus,
}

#[derive(Default)]
pub(crate) struct ReportIndex {
    identity: PublicationIdentity,
    ready: bool,
    programs: BTreeMap<usize, ProgramIndex>,
    groups: Vec<CodeGroup>,
    contexts: Vec<ContextTarget>,
    by_program: BTreeMap<usize, Vec<ContextTarget>>,
    pub(super) minimum: Vec<MinimumIndex>,
    baseline: Option<GraphIndex>,
    native: Option<GraphIndex>,
    uncaptured: Vec<ProcessSource>,
}

impl ReportIndex {
    pub(crate) fn identity(&self) -> PublicationIdentity {
        self.identity
    }

    /// Overall publication can grow the shared catalog while graph identity is
    /// unchanged. Namespace changes alone reuse the same catalog and offsets.
    pub(crate) fn update(
        &mut self,
        report: &AnalysisReport,
        view: ReportViewId,
        publication: Option<&JobPublication>,
        job: Option<JobId>,
    ) {
        let identity = PublicationIdentity {
            report: view.generation(),
            job,
            publication: publication.map(|value| value.version),
            minimum: publication.and_then(|value| value.minimum_version),
            global: publication.and_then(|value| value.global_version),
            native: publication.and_then(|value| value.native_version),
        };
        if self.ready && self.identity == identity {
            return;
        }
        *self = Self {
            identity,
            ready: true,
            ..Default::default()
        };
        let mut hash_groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (position, program) in report.programs.iter().enumerate() {
            self.programs.insert(
                program.id,
                ProgramIndex {
                    position,
                    rows: ProgramRows::new(program),
                    blocks: program
                        .blocks
                        .iter()
                        .enumerate()
                        .map(|(index, block)| (block.id, index))
                        .collect(),
                },
            );
            let candidates = hash_groups.entry(&program.code_hash).or_default();
            let matching = candidates.iter().copied().find(|&group| {
                let exemplar = self
                    .program(report, self.groups[group].programs[0])
                    .unwrap();
                exemplar.fork == program.fork
                    && exemplar.kind == program.kind
                    && exemplar.bytecode == program.bytecode
            });
            if let Some(group) = matching {
                self.groups[group].programs.push(program.id);
            } else {
                candidates.push(self.groups.len());
                self.groups.push(CodeGroup {
                    programs: vec![program.id],
                });
            }
        }
        if let Some(model) = &report.minimum {
            self.add_uncaptured(report, &model.declared_root_source);
            let latest: BTreeMap<_, _> = model
                .summaries
                .iter()
                .enumerate()
                .map(|(index, summary)| (summary.relation.source_state, index))
                .collect();
            let minimum_view = view.minimum(publication.and_then(|value| value.minimum_version));
            for (summary, row) in model.summaries.iter().enumerate() {
                self.minimum.push(MinimumIndex {
                    nodes: row
                        .relation
                        .regional_cfg
                        .nodes
                        .iter()
                        .enumerate()
                        .map(|(index, node)| (node.id, index))
                        .collect(),
                    status: row
                        .relation
                        .source_status(latest[&row.relation.source_state] != summary),
                });
                for (procedure, row) in row.relation.procedures.iter().enumerate() {
                    let target = ContextTarget {
                        view: minimum_view,
                        context: ContextFocus::Minimum { summary, procedure },
                    };
                    if let Some(frame) = &row.process {
                        self.add_uncaptured(report, &frame.source);
                    }
                    let source = procedure_program(report, row).map(|program| program.id);
                    self.add_context(target, source);
                }
            }
        }
        for namespace in [AnalysisNamespace::Baseline, AnalysisNamespace::Native] {
            let Some(graph) = report.graph(namespace) else {
                continue;
            };
            let graph_view = view.for_namespace(namespace, publication);
            for state in &graph.cfg {
                if let Some(frame) = &state.process {
                    self.add_uncaptured(report, &frame.source);
                }
                let program = state.program.or_else(|| {
                    state
                        .process
                        .as_ref()
                        .and_then(|frame| report.process_program(&frame.source))
                        .map(|program| program.id)
                });
                self.add_context(
                    ContextTarget {
                        view: graph_view,
                        context: ContextFocus::Graph { state: state.id },
                    },
                    program,
                );
            }
            match namespace {
                AnalysisNamespace::Baseline => self.baseline = Some(GraphIndex::new(graph)),
                AnalysisNamespace::Native => self.native = Some(GraphIndex::new(graph)),
            }
        }
    }

    fn add_context(&mut self, target: ContextTarget, program: Option<usize>) {
        self.contexts.push(target);
        if let Some(program) = program {
            self.by_program.entry(program).or_default().push(target);
        }
    }

    fn add_uncaptured(&mut self, report: &AnalysisReport, source: &ProcessSource) {
        if report.process_program(source).is_none() && !self.uncaptured.contains(source) {
            self.uncaptured.push(source.clone());
        }
    }

    pub(super) fn contains_uncaptured(&self, source: &ProcessSource) -> bool {
        self.uncaptured.contains(source)
    }

    pub(crate) fn program<'a>(
        &self,
        report: &'a AnalysisReport,
        id: usize,
    ) -> Option<&'a ProgramInfo> {
        report
            .programs
            .get(self.programs.get(&id)?.position)
            .map(AsRef::as_ref)
    }

    pub(crate) fn source_rows(&self, id: usize) -> Option<&ProgramRows> {
        Some(&self.programs.get(&id)?.rows)
    }

    pub(crate) fn block<'a>(
        &self,
        report: &'a AnalysisReport,
        program: usize,
        block: usize,
    ) -> Option<&'a DisasmBlock> {
        self.program(report, program)?
            .blocks
            .get(*self.programs.get(&program)?.blocks.get(&block)?)
    }

    pub(crate) fn code_groups(&self) -> &[CodeGroup] {
        &self.groups
    }

    pub(crate) fn contexts(&self) -> &[ContextTarget] {
        &self.contexts
    }

    pub(crate) fn contexts_for_program(&self, program: usize) -> &[ContextTarget] {
        self.by_program.get(&program).map_or(&[], Vec::as_slice)
    }

    pub(super) fn graph(&self, namespace: AnalysisNamespace) -> Option<&GraphIndex> {
        match namespace {
            AnalysisNamespace::Baseline => self.baseline.as_ref(),
            AnalysisNamespace::Native => self.native.as_ref(),
        }
    }
}

pub(super) fn procedure_program<'a>(
    report: &'a AnalysisReport,
    procedure: &evm_abstract_protocol::AbstractProcedure,
) -> Option<&'a ProgramInfo> {
    if let Some(frame) = &procedure.process {
        return report.process_program(&frame.source);
    }
    let fork = report.minimum.as_ref()?.fork;
    report
        .programs
        .iter()
        .find(|program| {
            program.code_address == procedure.code_address
                && Some(&program.code_hash) == procedure.code_hash.as_ref()
                && program.kind == procedure.kind
                && program.fork == fork
        })
        .map(AsRef::as_ref)
}

impl Presentation<'_> {
    pub(crate) fn source(&self, program: usize) -> Option<SourceFocus> {
        self.program(program)?;
        Some(SourceFocus::Captured(SourceId {
            report: self.view.generation(),
            program,
        }))
    }

    pub(super) fn process_source(&self, source: &ProcessSource) -> SourceFocus {
        self.report
            .process_program(source)
            .and_then(|program| self.source(program.id))
            .unwrap_or_else(|| SourceFocus::Uncaptured {
                report: self.view.generation(),
                source: source.clone(),
            })
    }
}
