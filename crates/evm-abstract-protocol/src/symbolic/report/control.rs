//! Validate shared source geometry without manufacturing execution evidence.
mod region;
use crate::{
    AnalysisReport, ProcessControlRelation, ProcessFrameKey, ProcessLocation as Location,
    ProcessSource, ProcessValidationKind, ReportValidationFailure,
};
use std::collections::BTreeSet;

fn invalid() -> ReportValidationFailure {
    ReportValidationFailure::Process(ProcessValidationKind::Control)
}

fn source_fork(key: &ProcessFrameKey, fork: crate::Fork) -> Result<(), ReportValidationFailure> {
    if key.source.fork() != fork {
        Err(ReportValidationFailure::Process(
            ProcessValidationKind::Source,
        ))
    } else {
        Ok(())
    }
}

fn location(point: Location, blocks: usize) -> bool {
    match point {
        Location::Block(index) => index < blocks,
        Location::JumpTargets | Location::End => true,
        Location::Any => false,
    }
}

impl AnalysisReport {
    pub(super) fn validate_process_controls(&self) -> Result<(), ReportValidationFailure> {
        for (index, relation) in self.process_controls.iter().enumerate() {
            if relation.id != index
                || self.process_controls[..index].iter().any(|previous| {
                    previous.epoch == relation.epoch && previous.source == relation.source
                })
            {
                return Err(invalid());
            }
            self.control_relation(relation)?;
        }
        if let Some(minimum) = &self.minimum {
            for summary in &minimum.summaries {
                self.control_summary(&summary.relation, minimum.fork)?;
            }
        }
        for graph in [&self.global, &self.native].into_iter().flatten() {
            for summary in &graph.abstract_summaries {
                self.control_summary(summary, graph.fork)?;
            }
            for block in &graph.cfg {
                if let Some(key) = &block.process {
                    source_fork(key, graph.fork)?;
                    self.control_location(key)?;
                }
            }
            for state in &graph.states {
                for machine in std::iter::once(&state.entry).chain(state.exit.iter()) {
                    for frame in &machine.frames {
                        if let Some(key) = &frame.process {
                            source_fork(key, graph.fork)?;
                            self.control_location(key)?;
                        }
                    }
                }
            }
            for machine in &graph.pending_machines {
                for frame in &machine.frames {
                    if let Some(key) = &frame.process {
                        source_fork(key, graph.fork)?;
                        self.control_location(key)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn control_relation(
        &self,
        relation: &ProcessControlRelation,
    ) -> Result<(), ReportValidationFailure> {
        if !matches!(
            relation.source,
            ProcessSource::InputProgram(_) | ProcessSource::Runtime(_) | ProcessSource::InitCode(_)
        ) {
            return Err(invalid());
        }
        let program = relation
            .program
            .and_then(|id| self.programs.iter().find(|program| program.id == id));
        if (relation.program.is_some() && program.is_none())
            || (program.is_none() && !matches!(relation.source, ProcessSource::InitCode(_)))
        {
            return Err(invalid());
        }
        if let Some(program) = program {
            if !relation.source.matches_program(program)
                || relation.blocks.len() != program.blocks.len()
            {
                return Err(invalid());
            }
            let targets: Vec<_> = program
                .blocks
                .iter()
                .enumerate()
                .filter_map(|(index, block)| {
                    block
                        .instructions
                        .first()
                        .is_some_and(|instruction| instruction.opcode == 0x5b)
                        .then_some(index)
                })
                .collect();
            if relation.jump_targets != targets {
                return Err(invalid());
            }
        }
        let mut targets = BTreeSet::new();
        if relation
            .jump_targets
            .iter()
            .any(|target| *target >= relation.blocks.len() || !targets.insert(*target))
        {
            return Err(invalid());
        }
        for block in &relation.blocks {
            if block
                .continuation
                .is_some_and(|point| !location(point, relation.blocks.len()))
                || block.successors.iter().flatten().any(|edge| {
                    !location(edge.to, relation.blocks.len())
                        || !matches!(
                            edge.kind,
                            crate::EdgeKind::Fallthrough
                                | crate::EdgeKind::Jump
                                | crate::EdgeKind::BranchTrue
                                | crate::EdgeKind::BranchFalse
                        )
                })
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn control_location(&self, key: &ProcessFrameKey) -> Result<(), ReportValidationFailure> {
        if matches!(
            key.source,
            ProcessSource::InputProgram(_) | ProcessSource::Runtime(_) | ProcessSource::InitCode(_)
        ) {
            if key.location == Location::Any {
                return Err(invalid());
            }
            let blocks = self
                .process_program(&key.source)
                .map(|program| program.blocks.len())
                .or_else(|| {
                    // Only the conditional constructor DTO may carry source
                    // geometry without instruction bytes. Its exact epoch and
                    // source still have to provide the location's bounds.
                    if !matches!(key.source, ProcessSource::InitCode(_)) {
                        return None;
                    }
                    self.process_controls
                        .iter()
                        .find(|relation| {
                            relation.epoch == key.epoch && relation.source == key.source
                        })
                        .map(|relation| relation.blocks.len())
                })
                .ok_or_else(|| self.missing_control_source(&key.source))?;
            if !location(key.location, blocks) {
                return Err(invalid());
            }
        } else if key.location != Location::Any {
            return Err(invalid());
        }
        Ok(())
    }

    fn missing_control_source(&self, source: &ProcessSource) -> ReportValidationFailure {
        // Preserve the precise fork diagnostic when the same captured identity
        // exists only under another fork. No such program is admitted here.
        let mut candidate = source.clone();
        for program in &self.programs {
            match &mut candidate {
                ProcessSource::InputProgram(source) => source.fork = program.fork,
                ProcessSource::Runtime(source) => source.fork = program.fork,
                ProcessSource::InitCode(source) => source.fork = program.fork,
                _ => return invalid(),
            }
            if candidate.matches_program(program) {
                return ReportValidationFailure::Source(crate::SourceValidationKind::Fork);
            }
        }
        invalid()
    }
}
