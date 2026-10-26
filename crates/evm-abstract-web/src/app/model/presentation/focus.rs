//! Resolve original context and adapt legacy widget picks without inventing evidence.

use super::{ContextTarget, Presentation, ResolvedContext};
use crate::app::{
    Selection, minimum,
    model::{
        ReportViewId,
        navigation::{
            ContextFocus, EvidenceFocus, Focus, MinimumEvidence, NavigationError, SourceFocus,
        },
    },
};
use evm_abstract_protocol::AnalysisNamespace;

impl<'a> Presentation<'a> {
    pub(crate) fn entry_focus(&self, view: ReportViewId) -> Result<Focus, NavigationError> {
        self.validate_view(view)?;
        if view.is_minimum() {
            let model = self
                .report
                .minimum
                .as_ref()
                .ok_or(NavigationError::MissingContext)?;
            let mut selection = minimum::Selection::default();
            selection.select_summary(model, minimum::initial_summary(model));
            if let Ok(focus) = self.minimum_focus(view, selection, None) {
                return Ok(focus);
            }
            return Ok(Focus::source(
                view,
                self.process_source(&model.declared_root_source),
            ));
        }
        let graph = self
            .report
            .graph(view.namespace())
            .ok_or(NavigationError::MissingContext)?;
        let selection = Selection {
            state: graph.cfg.first().map(|state| state.id),
            ..Default::default()
        };
        self.graph_focus(
            view,
            selection,
            selection
                .state
                .is_none()
                .then_some(graph.metadata.root_program)
                .flatten(),
        )
    }

    pub(crate) fn context(
        &self,
        target: ContextTarget,
    ) -> Result<ResolvedContext<'a>, NavigationError> {
        self.validate_view(target.view)?;
        match target.context {
            ContextFocus::Minimum { summary, procedure } => {
                if !target.view.is_minimum() {
                    return Err(NavigationError::ContextMismatch);
                }
                let row = self
                    .report
                    .minimum
                    .as_ref()
                    .and_then(|model| model.summaries.get(summary))
                    .ok_or(NavigationError::MissingContext)?;
                Ok(ResolvedContext::Minimum {
                    summary: row,
                    procedure: row
                        .relation
                        .procedures
                        .get(procedure)
                        .ok_or(NavigationError::MissingContext)?,
                    bounds: row
                        .bounds
                        .get(procedure)
                        .ok_or(NavigationError::MissingContext)?,
                    source_status: self
                        .index
                        .minimum
                        .get(summary)
                        .ok_or(NavigationError::MissingContext)?
                        .status,
                })
            }
            ContextFocus::Graph { state } => {
                if target.view.is_minimum() {
                    return Err(NavigationError::ContextMismatch);
                }
                let graph = self
                    .report
                    .graph(target.view.namespace())
                    .ok_or(NavigationError::MissingContext)?;
                let positions = self
                    .index
                    .graph(target.view.namespace())
                    .ok_or(NavigationError::MissingContext)?;
                Ok(ResolvedContext::Graph {
                    graph,
                    state: graph
                        .cfg
                        .get(
                            *positions
                                .cfg
                                .get(&state)
                                .ok_or(NavigationError::MissingContext)?,
                        )
                        .ok_or(NavigationError::MissingContext)?,
                    frame: positions
                        .states
                        .get(&state)
                        .and_then(|&index| graph.states.get(index))
                        .and_then(|state| state.entry.frames.last()),
                    coverage: positions
                        .ssa
                        .get(&state)
                        .and_then(|&index| graph.ssa.blocks.get(index))
                        .map(|ssa| ssa.coverage),
                    at_current_revision: target.view.namespace() != AnalysisNamespace::Native
                        || self.native_at_current_revision(),
                    binding: (target.view.namespace() == AnalysisNamespace::Native)
                        .then(|| {
                            self.report
                                .hybrid
                                .as_ref()
                                .map(|hybrid| &hybrid.native_coverage().detail_binding)
                        })
                        .flatten(),
                })
            }
        }
    }

    pub(crate) fn focus_context(&self, target: ContextTarget) -> Result<Focus, NavigationError> {
        let context = self.context(target)?;
        let source = self.context_source(context);
        let evidence = match target.context {
            ContextFocus::Minimum { summary, procedure } => {
                let node = self.report.minimum.as_ref().unwrap().summaries[summary]
                    .relation
                    .regional_cfg
                    .nodes
                    .iter()
                    .find(|node| node.procedure == procedure)
                    .map(|node| node.id);
                EvidenceFocus::Minimum(MinimumEvidence {
                    node,
                    ..Default::default()
                })
            }
            ContextFocus::Graph { state } => EvidenceFocus::Graph(Selection {
                state: Some(state),
                ..Default::default()
            }),
        };
        let focus = Focus {
            view: target.view,
            source,
            context: Some(target.context),
            evidence,
        };
        self.validate(&focus)?;
        Ok(focus)
    }

    pub(crate) fn resolve_context(
        &self,
        focus: &Focus,
    ) -> Result<Option<ResolvedContext<'a>>, NavigationError> {
        self.validate(focus)?;
        focus
            .context
            .map(|context| {
                self.context(ContextTarget {
                    view: focus.view,
                    context,
                })
            })
            .transpose()
    }

    pub(crate) fn graph_focus(
        &self,
        view: ReportViewId,
        selection: Selection,
        program: Option<usize>,
    ) -> Result<Focus, NavigationError> {
        let Some(state) = selection.state else {
            if selection.pc.is_some() || selection.value.is_some() {
                return Err(NavigationError::MissingContext);
            }
            let focus = Focus {
                view,
                source: program
                    .map(|program| self.source(program).ok_or(NavigationError::MissingSource))
                    .transpose()?,
                context: None,
                evidence: EvidenceFocus::Source {
                    block: None,
                    pc: None,
                },
            };
            self.validate(&focus)?;
            return Ok(focus);
        };
        let mut focus = self.focus_context(ContextTarget {
            view,
            context: ContextFocus::Graph { state },
        })?;
        if let Some(program) = program {
            focus.source = Some(self.source(program).ok_or(NavigationError::MissingSource)?);
        }
        focus.evidence = EvidenceFocus::Graph(selection);
        self.validate(&focus)?;
        Ok(focus)
    }

    pub(crate) fn minimum_focus(
        &self,
        view: ReportViewId,
        selection: minimum::Selection,
        program: Option<usize>,
    ) -> Result<Focus, NavigationError> {
        let mut focus = self.focus_context(ContextTarget {
            view,
            context: ContextFocus::Minimum {
                summary: selection.summary,
                procedure: selection.procedure,
            },
        })?;
        if let Some(program) = program {
            focus.source = Some(self.source(program).ok_or(NavigationError::MissingSource)?);
        }
        focus.evidence = EvidenceFocus::Minimum(MinimumEvidence {
            node: selection.node,
            control: selection.control,
            output: selection.output,
            account: selection.account,
        });
        self.validate(&focus)?;
        Ok(focus)
    }

    pub(super) fn context_source(&self, context: ResolvedContext<'_>) -> Option<SourceFocus> {
        match context {
            ResolvedContext::Minimum { procedure, .. } => {
                if let Some(frame) = &procedure.process {
                    return Some(self.process_source(&frame.source));
                }
                super::index::procedure_program(self.report, procedure)
                    .and_then(|program| self.source(program.id))
            }
            ResolvedContext::Graph { state, .. } => {
                if let Some(frame) = &state.process {
                    return Some(self.process_source(&frame.source));
                }
                state.program.and_then(|id| self.source(id))
            }
        }
    }
}
