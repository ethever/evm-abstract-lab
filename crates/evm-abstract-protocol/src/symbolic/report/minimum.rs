//! Minimum and rich artifacts share source qualification, never private IDs.
use crate::{
    AnalysisReport, CodeKind, MinimumModel, MinimumStore, MinimumValidationKind as Kind,
    ProcessSource, ReportValidationFailure, RichExportOutcome, RootInputBinding,
};

fn invalid(kind: Kind) -> ReportValidationFailure {
    ReportValidationFailure::Minimum(kind)
}

impl AnalysisReport {
    pub(super) fn validate_minimum(&self) -> Result<(), ReportValidationFailure> {
        let rich = self.global.is_some() || self.native.is_some();
        if matches!(self.rich_export, RichExportOutcome::Complete) {
            if !rich {
                return Err(invalid(Kind::Identity));
            }
        } else if self.minimum.is_none() {
            return Err(invalid(Kind::Identity));
        }
        let Some(minimum) = &self.minimum else {
            return Ok(());
        };
        minimum.validate()?;
        let hybrid = self
            .hybrid
            .as_ref()
            .ok_or_else(|| invalid(Kind::Identity))?;
        let coverage = &hybrid.coverage;
        if minimum.current != coverage.baseline_current
            || minimum.source_snapshot != coverage.snapshot
            || minimum.revision != coverage.baseline_revision
            || minimum.config != coverage.baseline_config
        {
            return Err(invalid(Kind::Revision));
        }
        match &minimum.root_binding {
            RootInputBinding::Physical(source) => {
                if minimum.entry_address != source.address
                    || minimum.environment.to
                        != crate::AddressValue::Concrete(source.address.clone())
                    || matches!(
                        minimum.declared_root_source,
                        ProcessSource::InputProgram(_) | ProcessSource::InputDelegation(_)
                    )
                {
                    return Err(invalid(Kind::Source));
                }
            }
            RootInputBinding::Namespace(source) => {
                if minimum.entry_address != source.coordinate
                    || !matches!(minimum.environment.to, crate::AddressValue::Symbolic(_))
                {
                    return Err(invalid(Kind::Source));
                }
            }
        }
        if let Some(global) = &self.global {
            let metadata = &global.metadata;
            if minimum.fork != global.fork
                || minimum.scope != global.scope
                || minimum.source_snapshot != metadata.source_snapshot
                || minimum.revision != metadata.revision
                || minimum.root_binding != metadata.root_binding
                || minimum.declared_root_source != metadata.declared_root_source
                || minimum.entry_address != metadata.entry_address
                || minimum.fingerprint != metadata.fingerprint
                || minimum.numeric_model != metadata.limits.numeric_model
                || minimum.environment.to != metadata.environment.to
                || minimum.environment.caller != metadata.environment.caller
                || minimum.environment.origin != metadata.environment.origin
                || minimum.environment.is_static != metadata.environment.is_static
            {
                return Err(invalid(Kind::Source));
            }
        }
        for summary in &minimum.summaries {
            for row in &summary.bounds {
                self.minimum_store(minimum, &row.input)?;
                for output in &row.outputs {
                    self.minimum_store(minimum, &output.store)?;
                }
            }
        }
        Ok(())
    }

    fn minimum_store(
        &self,
        minimum: &MinimumModel,
        store: &MinimumStore,
    ) -> Result<(), ReportValidationFailure> {
        for code in &store.codes {
            let Some(id) = code.program else { continue };
            let program = self
                .programs
                .iter()
                .find(|program| program.id == id)
                .ok_or_else(|| invalid(Kind::Source))?;
            let supplied = program.code_address.is_none()
                && matches!(&minimum.declared_root_source,
                    ProcessSource::InputProgram(source) if program.code_hash == source.hash.to_string());
            if program.kind != CodeKind::Runtime
                || program.fork != minimum.fork
                || (program.code_address.as_deref() != Some(code.owner.as_str()) && !supplied)
            {
                return Err(invalid(Kind::Source));
            }
        }
        Ok(())
    }
}
