//! Reject coordinates that do not belong to the exact source/context/publication.

use super::{ContextTarget, Presentation, ResolvedContext};
use crate::app::{
    Selection,
    model::{
        ReportViewId,
        navigation::{
            ContextFocus, EvidenceFocus, Focus, MinimumEvidence, NavigationError, SourceFocus,
        },
    },
};
use evm_abstract_protocol::{ProcessLocation, ProcessSource, SsaValueOrigin};

impl<'a> Presentation<'a> {
    pub(crate) fn validate(&self, focus: &Focus) -> Result<(), NavigationError> {
        self.validate_view(focus.view)?;
        if let Some(source) = &focus.source {
            if source.generation() != self.view.generation() {
                return Err(NavigationError::StaleReport);
            }
            if let SourceFocus::Captured(id) = source
                && self.program(id.program).is_none()
            {
                return Err(NavigationError::MissingSource);
            }
            if let SourceFocus::Uncaptured { source, .. } = source
                && !self.index.contains_uncaptured(source)
            {
                return Err(NavigationError::MissingSource);
            }
        }
        let context = focus
            .context
            .map(|context| {
                self.context(ContextTarget {
                    view: focus.view,
                    context,
                })
            })
            .transpose()?;
        if let Some(context) = context
            && !self.source_matches_context(focus.source.as_ref(), context)
        {
            return Err(NavigationError::SourceMismatch);
        }
        match focus.evidence {
            EvidenceFocus::Source { block, pc } => {
                if block.is_none() && pc.is_none() {
                    return Ok(());
                }
                let program = focus
                    .source
                    .as_ref()
                    .and_then(SourceFocus::program)
                    .and_then(|id| self.program(id))
                    .ok_or(NavigationError::MissingSource)?;
                if let Some(block) = block {
                    let block = self
                        .index
                        .block(self.report, program.id, block)
                        .ok_or(NavigationError::MissingEvidence)?;
                    if pc.is_some_and(|pc| {
                        !block
                            .instructions
                            .iter()
                            .any(|instruction| instruction.pc == pc)
                    }) {
                        return Err(NavigationError::MissingEvidence);
                    }
                } else if pc.is_some_and(|pc| {
                    !program.blocks.iter().any(|block| {
                        block
                            .instructions
                            .iter()
                            .any(|instruction| instruction.pc == pc)
                    })
                }) {
                    return Err(NavigationError::MissingEvidence);
                }
            }
            EvidenceFocus::Minimum(evidence) => self.validate_minimum(focus, evidence)?,
            EvidenceFocus::Graph(selection) => self.validate_graph(focus, selection, context)?,
        }
        Ok(())
    }

    pub(super) fn validate_view(&self, view: ReportViewId) -> Result<(), NavigationError> {
        if view.generation() != self.view.generation()
            || self.index.identity().report != self.view.generation()
        {
            return Err(NavigationError::StaleReport);
        }
        let canonical = if view.is_minimum() {
            self.report
                .minimum
                .as_ref()
                .ok_or(NavigationError::StaleView)?;
            self.view.minimum(
                self.publication
                    .and_then(|publication| publication.minimum_version),
            )
        } else {
            self.report
                .graph(view.namespace())
                .ok_or(NavigationError::StaleView)?;
            self.view.for_namespace(view.namespace(), self.publication)
        };
        if view != canonical {
            return Err(NavigationError::StaleView);
        }
        Ok(())
    }

    fn source_matches_context(
        &self,
        source: Option<&SourceFocus>,
        context: ResolvedContext<'_>,
    ) -> bool {
        let process = match context {
            ResolvedContext::Minimum { procedure, .. } => procedure.process.as_ref(),
            ResolvedContext::Graph { state, .. } => state.process.as_ref(),
        };
        if let Some(process) = process {
            return match source {
                Some(SourceFocus::Captured(id)) => self
                    .program(id.program)
                    .is_some_and(|program| process.source.matches_program(program)),
                Some(SourceFocus::Uncaptured { source, .. }) => *source == process.source,
                None => false,
            };
        }
        source == self.context_source(context).as_ref()
    }

    fn validate_minimum(
        &self,
        focus: &Focus,
        evidence: MinimumEvidence,
    ) -> Result<(), NavigationError> {
        let Some(ContextFocus::Minimum { summary, procedure }) = focus.context else {
            return Err(NavigationError::ContextMismatch);
        };
        if !focus.view.is_minimum() {
            return Err(NavigationError::ContextMismatch);
        }
        let model = self
            .report
            .minimum
            .as_ref()
            .ok_or(NavigationError::MissingContext)?;
        let row = model
            .summaries
            .get(summary)
            .ok_or(NavigationError::MissingContext)?;
        let node = evidence
            .node
            .map(|id| {
                self.index.minimum[summary]
                    .nodes
                    .get(&id)
                    .and_then(|&index| row.relation.regional_cfg.nodes.get(index))
                    .ok_or(NavigationError::MissingEvidence)
            })
            .transpose()?;
        if node.is_some_and(|node| node.procedure != procedure) {
            return Err(NavigationError::ContextMismatch);
        }
        if let Some(location) = evidence.control {
            let node = node.ok_or(NavigationError::MissingEvidence)?;
            let frame = node
                .frame
                .process
                .as_ref()
                .ok_or(NavigationError::MissingEvidence)?;
            let region = node
                .transfer
                .as_ref()
                .and_then(|transfer| transfer.process.as_ref())
                .and_then(|process| process.control.as_ref());
            if location != frame.location
                && !region.is_some_and(|region| {
                    region.through.contains(&location) || region.boundaries.contains(&location)
                })
            {
                return Err(NavigationError::MissingEvidence);
            }
            self.validate_location(&frame.source, location)?;
        }
        if evidence.output.is_some_and(|output| {
            row.bounds
                .get(procedure)
                .and_then(|bounds| bounds.outputs.get(output))
                .is_none()
        }) || evidence
            .account
            .is_some_and(|account| model.accounts.get(account).is_none())
        {
            return Err(NavigationError::MissingEvidence);
        }
        Ok(())
    }

    fn validate_location(
        &self,
        source: &ProcessSource,
        location: ProcessLocation,
    ) -> Result<(), NavigationError> {
        let valid = match location {
            ProcessLocation::Block(index) => self
                .report
                .process_program(source)
                .is_some_and(|program| index < program.blocks.len()),
            ProcessLocation::JumpTargets | ProcessLocation::End => matches!(
                source,
                ProcessSource::InputProgram(_)
                    | ProcessSource::Runtime(_)
                    | ProcessSource::InitCode(_)
            ),
            ProcessLocation::Any => matches!(
                source,
                ProcessSource::InputDelegation(_)
                    | ProcessSource::AnyRuntime(_)
                    | ProcessSource::AnyInitCode(_)
            ),
        };
        if valid {
            Ok(())
        } else {
            Err(NavigationError::MissingEvidence)
        }
    }

    fn validate_graph(
        &self,
        focus: &Focus,
        selection: Selection,
        context: Option<ResolvedContext<'_>>,
    ) -> Result<(), NavigationError> {
        let Some(ContextFocus::Graph { state }) = focus.context else {
            return Err(NavigationError::ContextMismatch);
        };
        if selection.state != Some(state) || focus.view.is_minimum() {
            return Err(NavigationError::ContextMismatch);
        }
        let Some(ResolvedContext::Graph {
            graph,
            state: block,
            ..
        }) = context
        else {
            return Err(NavigationError::MissingContext);
        };
        let positions = self
            .index
            .graph(focus.view.namespace())
            .ok_or(NavigationError::MissingContext)?;
        let ssa = positions
            .ssa
            .get(&state)
            .and_then(|&index| graph.ssa.blocks.get(index));
        if let Some(pc) = selection.pc
            && (block.process.is_some()
                || !ssa.is_some_and(|ssa| {
                    ssa.instructions
                        .iter()
                        .any(|instruction| instruction.pc == pc)
                }))
        {
            return Err(NavigationError::MissingEvidence);
        }
        if let Some(value) = selection.value {
            let value = positions
                .values
                .get(&value)
                .and_then(|&index| graph.ssa.values.get(index))
                .ok_or(NavigationError::MissingEvidence)?;
            let origin_here = match &value.origin {
                SsaValueOrigin::Phi(origin) => origin.state == state,
                SsaValueOrigin::Instruction(origin) => origin.state == state,
                SsaValueOrigin::Transition(origin) => origin.from == state || origin.to == state,
            };
            let used_here = ssa.is_some_and(|ssa| {
                ssa.phis.iter().any(|phi| {
                    phi.result == value.id || phi.inputs.iter().any(|input| input.value == value.id)
                }) || ssa.instructions.iter().any(|instruction| {
                    instruction.operands.contains(&value.id)
                        || instruction.results.contains(&value.id)
                }) || ssa.exit_frames.iter().flatten().any(|&id| id == value.id)
            });
            if block.process.is_some() || (!origin_here && !used_here) {
                return Err(NavigationError::ContextMismatch);
            }
            if selection.pc.is_some_and(|pc| {
                !ssa.is_some_and(|ssa| {
                    ssa.instructions.iter().any(|instruction| {
                        instruction.pc == pc
                            && (instruction.operands.contains(&value.id)
                                || instruction.results.contains(&value.id))
                    })
                })
            }) {
                return Err(NavigationError::MissingEvidence);
            }
        }
        Ok(())
    }
}
