//! Validate local numerical/control identities without constructing a graph.

use super::{
    MinimumByteArray, MinimumByteCell, MinimumByteContent, MinimumModel, MinimumOwnerValue,
    MinimumPlane, MinimumStore, MinimumValidationKind,
};
use crate::{ReportValidationFailure, TransferSemantics, ValueInfo};
use std::collections::BTreeSet;

type Checked = Result<(), MinimumValidationKind>;

fn scalar(value: &ValueInfo) -> Checked {
    if value.expression.is_some() {
        Err(MinimumValidationKind::PrivateState)
    } else {
        Ok(())
    }
}

fn unique<K: Ord>(values: impl IntoIterator<Item = K>) -> Checked {
    let mut keys = BTreeSet::new();
    for key in values {
        if !keys.insert(key) {
            return Err(MinimumValidationKind::Duplicate);
        }
    }
    Ok(())
}

fn cells(values: &[MinimumByteCell]) -> Checked {
    unique(values.iter().map(|cell| cell.offset))?;
    for cell in values {
        scalar(&cell.value)?;
    }
    Ok(())
}

fn bytes(value: &MinimumByteArray) -> Checked {
    scalar(&value.length)?;
    if let MinimumByteContent::Cells(values) = &value.content {
        cells(values)?;
    }
    cells(&value.words)
}

fn owners(values: &[MinimumOwnerValue]) -> Checked {
    unique(values.iter().map(|value| &value.owner))?;
    for value in values {
        scalar(&value.value)?;
    }
    Ok(())
}

fn plane(value: &MinimumPlane) -> Checked {
    scalar(&value.default)?;
    owners(&value.owners)?;
    unique(value.slots.iter().map(|slot| (&slot.owner, &slot.slot)))?;
    for slot in &value.slots {
        scalar(&slot.value)?;
    }
    unique(value.independent_owners.iter().map(|value| &value.owner))?;
    unique(
        value
            .independent_slots
            .iter()
            .map(|slot| (&slot.owner, &slot.slot)),
    )
}

fn store(value: &MinimumStore) -> Checked {
    plane(&value.persistent)?;
    plane(&value.transient)?;
    scalar(&value.balance_default)?;
    owners(&value.balances)?;
    owners(&value.nonces)?;
    unique(value.codes.iter().map(|value| &value.owner))?;
    for code in &value.codes {
        if (code.kind == crate::CodeKind::Runtime) != code.program.is_some()
            || (code.kind == crate::CodeKind::Delegation) != code.delegation_target.is_some()
        {
            return Err(MinimumValidationKind::Source);
        }
    }
    unique(value.existence.iter().map(|value| &value.owner))?;
    for flags in [&value.created, &value.pending_destruction, &value.touched] {
        unique(flags.iter().map(|value| &value.owner))?;
    }
    unique(value.logs.iter().map(|log| {
        (
            &log.address,
            &log.code_address,
            &log.code_hash,
            matches!(log.source_kind, crate::LogSourceKind::Runtime),
            log.pc,
        )
    }))?;
    for log in &value.logs {
        if log.topics.len() > 4 {
            return Err(MinimumValidationKind::Bounds);
        }
        for topic in &log.topics {
            scalar(topic)?;
        }
        bytes(&log.data)?;
    }
    Ok(())
}

impl MinimumModel {
    /// Check the minimum's own identities, joint alternatives and numerical
    /// payloads. The publication owner additionally binds it to the envelope,
    /// source catalog and existing library Hybrid qualification.
    pub fn validate(&self) -> Result<(), ReportValidationFailure> {
        let error = ReportValidationFailure::Minimum;
        self.validate_numerical().map_err(error)?;
        for (index, summary) in self.summaries.iter().enumerate() {
            summary.relation.validate_control(index)?;
        }
        Ok(())
    }

    fn validate_numerical(&self) -> Checked {
        if self.namespace != crate::AnalysisNamespace::Baseline
            || self.declared_root_source.fork() != self.fork
        {
            return Err(MinimumValidationKind::Identity);
        }
        if self.current && self.status != crate::AnalysisStatus::Converged {
            return Err(MinimumValidationKind::Closure);
        }
        if self.current
            && !self.summaries.iter().any(|summary| {
                summary.relation.source_current
                    && summary.relation.closed
                    && summary.relation.root_procedure.is_some()
                    && !summary.relation.regional_cfg.nodes.is_empty()
            })
        {
            return Err(MinimumValidationKind::Closure);
        }
        if self.export_usage.nodes > self.export_limits.max_nodes
            || self.export_usage.exact_bytes > self.export_limits.max_exact_bytes
            || self.postprocessing_max_work == 0
            || self.export_usage.work > self.postprocessing_max_work
        {
            return Err(MinimumValidationKind::Export);
        }
        let env = &self.environment;
        for value in [
            &env.call_value,
            &env.gas_price,
            &env.timestamp,
            &env.number,
            &env.prevrandao,
            &env.gas_limit,
            &env.chain_id,
            &env.base_fee,
            &env.blob_base_fee,
            &env.blob_count,
        ] {
            scalar(value)?;
        }
        bytes(&env.calldata)?;
        unique(self.accounts.iter().map(|account| &account.address))?;
        for account in &self.accounts {
            if account.root_template
                != matches!(&self.root_binding,
                crate::RootInputBinding::Namespace(root) if root.coordinate == account.address)
                || (account.code_kind == crate::CodeKind::Runtime) != account.program.is_some()
                || (account.code_kind == crate::CodeKind::Delegation)
                    != account.delegation_target.is_some()
            {
                return Err(MinimumValidationKind::Source);
            }
            scalar(&account.balance)?;
            scalar(&account.nonce)?;
            unique(account.storage.iter().map(|slot| &slot.slot))?;
            for slot in &account.storage {
                scalar(&slot.value)?;
            }
        }
        for summary in &self.summaries {
            let relation = &summary.relation;
            if relation.procedures.len() != summary.bounds.len()
                || relation
                    .root_procedure
                    .is_some_and(|id| id >= summary.bounds.len())
            {
                return Err(MinimumValidationKind::Bounds);
            }
            if relation.closed
                && (summary.version.generation == 0 || relation.root_procedure.is_none())
            {
                return Err(MinimumValidationKind::Closure);
            }
            for (procedure, bounds) in relation.procedures.iter().zip(summary.bounds.iter()) {
                if let Some(process) = &procedure.process {
                    if process.source.fork() != self.fork
                        || process.source.semantics() != procedure.semantics
                        || process.is_static != procedure.is_static
                        || process.source.kind() != procedure.kind
                        || procedure.process_input.is_none()
                        || procedure.code_address.is_some()
                        || procedure.code_hash.is_some()
                        || procedure.storage_address.is_some()
                    {
                        return Err(MinimumValidationKind::Source);
                    }
                } else if matches!(
                    procedure.semantics,
                    TransferSemantics::KnownProcess
                        | TransferSemantics::ConstructorProcess
                        | TransferSemantics::BoundaryProcess
                ) {
                    return Err(MinimumValidationKind::Source);
                }
                if let Some(input) = &procedure.process_input {
                    scalar(&input.address)?;
                    scalar(&input.caller)?;
                    scalar(&input.value)?;
                }
                if procedure
                    .dependencies
                    .iter()
                    .any(|id| *id >= summary.bounds.len())
                    || procedure.outcomes
                        != bounds.outputs.iter().map(|o| o.kind).collect::<Vec<_>>()
                {
                    return Err(MinimumValidationKind::Bounds);
                }
                store(&bounds.input)?;
                for output in &bounds.outputs {
                    bytes(&output.data)?;
                    store(&output.store)?;
                }
            }
            for call in &relation.regional_cfg.calls {
                let bounds = summary
                    .bounds
                    .get(call.callee)
                    .ok_or(MinimumValidationKind::Bounds)?;
                for delivery in &call.deliveries {
                    if delivery.current
                        && !bounds.outputs.iter().any(|output| {
                            output.kind == delivery.kind
                                && output.generation == delivery.output_generation
                        })
                    {
                        return Err(MinimumValidationKind::Revision);
                    }
                }
            }
        }
        Ok(())
    }
}
