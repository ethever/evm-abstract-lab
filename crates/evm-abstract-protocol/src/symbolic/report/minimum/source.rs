//! Original account code and resolved executable provenance are distinct.
use super::{CodeKind, Kind, MinimumModel, ProcessSource, ReportValidationFailure, invalid};
use crate::{AnalysisReport, MinimumInputAccount, RootInputBinding};

impl AnalysisReport {
    pub(super) fn minimum_original_source(
        &self,
        minimum: &MinimumModel,
    ) -> Result<(), ReportValidationFailure> {
        let original = minimum
            .accounts
            .iter()
            .find(|account| account.address == minimum.entry_address);
        let namespace = matches!(minimum.root_binding, RootInputBinding::Namespace(_));
        let required = || original.ok_or_else(|| invalid(Kind::Source));
        let delegation = |account: &MinimumInputAccount, target: &str| {
            account.code_kind == CodeKind::Delegation
                && account.delegation_target.as_deref() == Some(target)
        };
        let valid = match &minimum.declared_root_source {
            ProcessSource::InputProgram(source) => {
                let original = required()?;
                original.code_kind == CodeKind::Runtime
                    && self.minimum_program_hash(original, &source.hash.to_string())
            }
            ProcessSource::InputDelegation(source) => delegation(required()?, &source.target),
            ProcessSource::Runtime(source) => {
                let original = required()?;
                // Resolution can follow an original delegation marker. A
                // Runtime declaration does not turn that marker into Runtime.
                let direct = !namespace && source.source == minimum.entry_address;
                let original_matches = if direct {
                    original.code_kind == CodeKind::Runtime
                } else {
                    delegation(original, &source.source)
                };
                original_matches
                    && minimum.accounts.iter().any(|account| {
                        !account.root_template
                            && account.address == source.source
                            && account.code_kind == CodeKind::Runtime
                            && self.minimum_program_hash(account, &source.hash.to_string())
                    })
            }
            ProcessSource::InvalidDelegation(source) => delegation(required()?, &source.source),
            // A delegation to empty code or a native precompile resolves to
            // Empty. The executable declaration has no original pointer here.
            ProcessSource::Empty(_) => matches!(
                required()?.code_kind,
                CodeKind::Empty | CodeKind::Delegation
            ),
            ProcessSource::AnyRuntime(_) => original.is_none_or(|account| {
                account.code_kind == CodeKind::Unknown
                    || (!namespace && account.code_kind == CodeKind::Delegation)
            }),
            // Physical precompile selection precedes lookup of the account's
            // original code. A namespace coordinate never selects it.
            ProcessSource::Precompile(source) => {
                !namespace && source.address == minimum.entry_address
            }
            // A transaction's declared World input is not a constructor body.
            ProcessSource::InitCode(_) | ProcessSource::AnyInitCode(_) => false,
        };
        if valid {
            Ok(())
        } else {
            Err(invalid(Kind::Source))
        }
    }

    fn minimum_program_hash(&self, account: &MinimumInputAccount, hash: &str) -> bool {
        account.program.is_some_and(|id| {
            self.programs
                .iter()
                .any(|program| program.id == id && program.code_hash == hash)
        })
    }
}
