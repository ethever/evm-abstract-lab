//! Input provenance checks are structural, independently of model closure.
use crate::{
    AddressValue, AnalysisReport, CodeKind, ProcessSource, ProcessValidationKind as Kind,
    ReportValidationFailure, RootInputBinding,
};

impl AnalysisReport {
    pub(super) fn validate_source(&self) -> Result<(), ReportValidationFailure> {
        let invalid = || ReportValidationFailure::Process(Kind::Source);
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
                    || self.programs.iter().any(|program| {
                        program.kind == CodeKind::Runtime
                            && program.code_address.as_ref() == Some(&source.coordinate)
                    })
                {
                    return Err(invalid());
                }
            }
        }
        if let Some(id) = self.metadata.root_program {
            let Some(program) = self.programs.iter().find(|program| program.id == id) else {
                return Err(invalid());
            };
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
