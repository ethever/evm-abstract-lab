//! Input provenance checks are structural, independently of model closure.
use crate::{
    AddressValue, AnalysisGraph, AnalysisReport, CodeKind, ProcessSource,
    ProcessValidationKind as Kind, ReportValidationFailure, RootInputBinding, SourceValidationKind,
};
use std::collections::BTreeSet;

impl AnalysisReport {
    pub(super) fn validate_catalog(&self) -> Result<(), ReportValidationFailure> {
        let mut ids = BTreeSet::new();
        let mut sources = BTreeSet::new();
        for program in &self.programs {
            // Kind and fork are fieldless wire choices. Their discriminants are
            // only local set keys; no persistent identity depends on them.
            if !ids.insert(program.id)
                || !sources.insert((
                    &program.code_address,
                    &program.code_hash,
                    program.kind as u8,
                    program.fork as u8,
                ))
            {
                return Err(ReportValidationFailure::Source(
                    SourceValidationKind::ProgramIdentity,
                ));
            }
            let mut blocks = BTreeSet::new();
            let mut pcs = BTreeSet::new();
            for block in &program.blocks {
                if !blocks.insert(block.id)
                    || block
                        .instructions
                        .first()
                        .is_none_or(|first| first.pc != block.start_pc)
                    || block
                        .instructions
                        .iter()
                        .any(|instruction| !pcs.insert(instruction.pc))
                {
                    return Err(ReportValidationFailure::Source(
                        SourceValidationKind::BlockIdentity,
                    ));
                }
            }
        }
        Ok(())
    }
}

impl super::Validation<'_> {
    pub(super) fn source_frame(
        &self,
        frame: &crate::FrameSnapshot,
    ) -> Result<(), ReportValidationFailure> {
        if frame.process.is_some() {
            return Ok(());
        }
        if frame.program.is_none() && matches!(frame.kind, CodeKind::Runtime | CodeKind::Initcode) {
            return Err(ReportValidationFailure::Source(SourceValidationKind::Frame));
        }
        if let Some(program) = frame
            .program
            .and_then(|id| self.programs.iter().find(|program| program.id == id))
        {
            let address_matches = program.code_address == frame.code_address
                || (program.code_address.is_none()
                    && frame.code_address.as_deref() == self.report.input_namespace());
            if !address_matches
                || Some(&program.code_hash) != frame.code_hash.as_ref()
                || program.kind != frame.kind
            {
                return Err(ReportValidationFailure::Source(SourceValidationKind::Frame));
            }
        }
        Ok(())
    }

    pub(super) fn source_instructions(
        &self,
        block: &crate::CfgBlock,
    ) -> Result<(), ReportValidationFailure> {
        let invalid = |kind| ReportValidationFailure::Source(kind);
        let frame = self
            .report
            .states
            .iter()
            .find(|state| state.state == block.id)
            .and_then(|state| state.entry.frames.last())
            .ok_or_else(|| invalid(SourceValidationKind::Frame))?;
        if frame.program != block.program || frame.basic_block != block.basic_block {
            return Err(invalid(SourceValidationKind::Frame));
        }
        let Some(source_ref) = block.source_block() else {
            if !block.executed_pcs.is_empty()
                || self
                    .report
                    .ssa
                    .blocks
                    .iter()
                    .any(|ssa| ssa.state == block.id && !ssa.instructions.is_empty())
            {
                return Err(invalid(SourceValidationKind::Instruction));
            }
            return Ok(());
        };
        let program = self
            .programs
            .iter()
            .find(|program| program.id == source_ref.program)
            .ok_or_else(|| invalid(SourceValidationKind::Block))?;
        let source = program
            .blocks
            .iter()
            .find(|source| source.id == source_ref.basic_block)
            .ok_or_else(|| invalid(SourceValidationKind::Block))?;
        if block.start_pc != Some(source.start_pc) {
            return Err(invalid(SourceValidationKind::Block));
        }
        if block.executed_pcs.iter().any(|pc| {
            !source
                .instructions
                .iter()
                .any(|instruction| instruction.pc == *pc)
        }) {
            return Err(invalid(SourceValidationKind::Instruction));
        }
        if let Some(ssa) = self
            .report
            .ssa
            .blocks
            .iter()
            .find(|ssa| ssa.state == block.id)
            && ssa.instructions.iter().any(|instruction| {
                !source.instructions.iter().any(|source| {
                    source.pc == instruction.pc
                        && source.opcode == instruction.opcode
                        && source.immediate == instruction.immediate
                })
            })
        {
            return Err(invalid(SourceValidationKind::Instruction));
        }
        Ok(())
    }
}

impl AnalysisGraph {
    pub(super) fn validate_source(
        &self,
        programs: &[crate::ProgramInfo],
    ) -> Result<(), ReportValidationFailure> {
        let invalid = || ReportValidationFailure::Process(Kind::Source);
        match (&self.metadata.source_snapshot, &self.metadata.snapshot) {
            (crate::RootTerminalSnapshot::Offline(_), None) => {}
            (crate::RootTerminalSnapshot::Chain(source), Some(header))
                if source.chain_id == header.chain_id && source.block_hash == header.block_hash => {
            }
            _ => {
                return Err(ReportValidationFailure::Source(
                    SourceValidationKind::Snapshot,
                ));
            }
        }
        if self.metadata.declared_root_source.fork() != self.fork {
            return Err(invalid());
        }
        match &self.metadata.root_binding {
            RootInputBinding::Physical(source) => {
                if self.metadata.environment.to != AddressValue::Concrete(source.address.clone())
                    || self.metadata.entry_address != source.address
                    || matches!(
                        self.metadata.declared_root_source,
                        ProcessSource::InputProgram(_) | ProcessSource::InputDelegation(_)
                    )
                {
                    return Err(invalid());
                }
            }
            RootInputBinding::Namespace(source) => {
                if self.metadata.entry_address != source.coordinate
                    || !matches!(self.metadata.environment.to, AddressValue::Symbolic(_))
                {
                    return Err(invalid());
                }
            }
        }
        if let Some(id) = self.metadata.root_program {
            let Some(program) = programs.iter().find(|program| program.id == id) else {
                return Err(invalid());
            };
            if program.fork != self.fork {
                return Err(ReportValidationFailure::Source(SourceValidationKind::Fork));
            }
            match &self.metadata.declared_root_source {
                ProcessSource::InputProgram(source)
                    if program.code_address.is_some()
                        || program.kind != CodeKind::Runtime
                        || program.code_hash != source.hash.to_string() =>
                {
                    return Err(invalid());
                }
                ProcessSource::Runtime(source)
                    if program.code_address.as_ref() != Some(&source.source)
                        || program.kind != CodeKind::Runtime
                        || program.code_hash != source.hash.to_string() =>
                {
                    return Err(invalid());
                }
                ProcessSource::InputDelegation(_) => return Err(invalid()),
                _ => {}
            }
        }
        Ok(())
    }
}
