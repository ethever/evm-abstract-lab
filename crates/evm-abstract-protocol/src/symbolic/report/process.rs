//! Structural process checks never turn receipts into semantic closure certificates.
use super::Validation;
use crate::{
    CreationContinuation, FrameSnapshot, ProcessCompletionCase, ProcessCompletionReason,
    ProcessCreationResult, ProcessFrameKey, ProcessReturnedData, ProcessSource,
    ProcessStoreDisposition, ProcessValidationKind as Kind, ReportValidationFailure,
};

pub(super) fn creation(source: &CreationContinuation) -> Result<(), Kind> {
    if let CreationContinuation::Process(descriptor) = source
        && !matches!(
            descriptor.action,
            crate::ProcessAction::Create | crate::ProcessAction::Create2
        )
    {
        return Err(Kind::Creation);
    }
    Ok(())
}

pub(super) fn completion(case: &ProcessCompletionCase) -> Result<(), Kind> {
    let valid = match (&case.result, case.reason) {
        (ProcessCreationResult::Address(owner), ProcessCompletionReason::Deployed) => {
            case.returndata == ProcessReturnedData::Empty
                && case.store == ProcessStoreDisposition::CommitConstructor
                && case
                    .deployment
                    .as_ref()
                    .is_some_and(|deployment| &deployment.owner == owner)
        }
        (ProcessCreationResult::Zero, ProcessCompletionReason::Exceptional) => {
            case.returndata == ProcessReturnedData::Empty
                && case.store == ProcessStoreDisposition::RestorePostNonceCheckpoint
                && case.deployment.is_none()
        }
        (ProcessCreationResult::Zero, ProcessCompletionReason::Reverted) => {
            case.returndata == ProcessReturnedData::ConstructorRevert
                && case.store == ProcessStoreDisposition::RestorePostNonceCheckpoint
                && case.deployment.is_none()
        }
        _ => false,
    };
    if valid { Ok(()) } else { Err(Kind::Creation) }
}

impl Validation<'_> {
    pub(super) fn process_program(
        &self,
        key: &ProcessFrameKey,
        program: Option<usize>,
    ) -> Result<(), ReportValidationFailure> {
        self.program(program)?;
        let Some(id) = program else { return Ok(()) };
        let Some(program) = self.report.programs.iter().find(|program| program.id == id) else {
            unreachable!("program reference checked")
        };
        let valid = match &key.source {
            ProcessSource::Runtime(source) => {
                program.code_address.as_ref() == Some(&source.source)
                    && program.code_hash == source.hash.to_string()
                    && program.kind == crate::CodeKind::Runtime
            }
            ProcessSource::InitCode(source) => {
                program.code_address.is_none()
                    && program.code_hash == source.hash.to_string()
                    && program.kind == crate::CodeKind::Initcode
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(ReportValidationFailure::Process(Kind::Source))
        }
    }

    pub(super) fn process_frame(
        &self,
        frame: &FrameSnapshot,
    ) -> Result<(), ReportValidationFailure> {
        let error = ReportValidationFailure::Process;
        if let Some(key) = &frame.process {
            if frame.kind != key.source.kind() || frame.is_static != key.is_static {
                return Err(error(Kind::Identity));
            }
            if frame.code_address.is_some()
                || frame.code_hash.is_some()
                || frame.storage_address.is_some()
                || frame.address_value.is_some()
                || frame.caller.is_some()
                || frame.basic_block.is_some()
                || !frame.context.is_empty()
                || !frame.stack.is_empty()
                || frame.memory.is_some()
                || frame.calldata.is_some()
                || frame.returndata.is_some()
                || frame.continuation.is_some()
                || frame.address_operand.is_some()
                || frame.caller_operand.is_some()
            {
                return Err(error(Kind::PrivateState));
            }
            let operands = frame
                .process_operands
                .as_ref()
                .ok_or(error(Kind::Operands))?;
            if operands.value != frame.call_value {
                return Err(error(Kind::Operands));
            }
            for value in [&operands.address, &operands.caller, &operands.value] {
                self.value(value)?;
            }
            if let Some(descriptor) = &frame.process_creation {
                creation(descriptor).map_err(error)?;
            }
            self.process_program(key, frame.program)?;
        } else {
            if frame.process_operands.is_some()
                || frame.process_creation.is_some()
                || frame.code_address.is_none()
                || frame.code_hash.is_none()
                || frame.storage_address.is_none()
                || frame.address_value.is_none()
                || frame.caller.is_none()
                || frame.basic_block.is_none()
                || frame.memory.is_none()
                || frame.calldata.is_none()
                || frame.returndata.is_none()
            {
                return Err(error(Kind::PrivateState));
            }
            if let Some(creation_descriptor) = frame
                .continuation
                .as_ref()
                .and_then(|next| next.creation.as_ref())
            {
                creation(creation_descriptor).map_err(error)?;
            }
        }
        Ok(())
    }
}
