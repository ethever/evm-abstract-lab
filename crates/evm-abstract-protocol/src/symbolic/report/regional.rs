//! Regional IDs are record-local table indexes, never native CFG/SSA IDs.
use super::Validation;
use crate::{
    AbstractSummary, CodeKind, PoolReferenceKind, RegionalNode, RegionalValidationFailure,
    RegionalValidationKind as Kind, ReportValidationFailure, TransferSemantics,
};

#[cfg(test)]
mod tests;

fn compatible_code(kind: CodeKind, semantics: TransferSemantics) -> bool {
    match semantics {
        TransferSemantics::Native => kind != CodeKind::AbstractEnvironment,
        TransferSemantics::Environment => kind == CodeKind::AbstractEnvironment,
        TransferSemantics::KnownProcess => kind == CodeKind::Runtime,
        TransferSemantics::ConstructorProcess => kind == CodeKind::Initcode,
        TransferSemantics::BoundaryProcess => matches!(
            kind,
            CodeKind::Empty | CodeKind::Precompile | CodeKind::InvalidDelegation
        ),
    }
}

fn block_evidence(node: &RegionalNode, semantics: TransferSemantics) -> Result<(), Kind> {
    // Non-program modes can retain a native placeholder block key. It is not
    // captured source, including when invalid delegation retains a raw hash.
    if !compatible_code(node.frame.kind, semantics) {
        return Err(Kind::Semantics);
    }
    if let Some(key) = &node.frame.process {
        if key.source.semantics() != semantics
            || key.source.kind() != node.frame.kind
            || key.is_static != node.frame.is_static
        {
            return Err(Kind::Semantics);
        }
        if node.frame.code_address.is_some()
            || node.frame.code_hash.is_some()
            || node.frame.storage_address.is_some()
            || node.frame.address_value.is_some()
            || node.frame.caller.is_some()
            || node.frame.stack_height.is_some()
            || !node.frame.context.is_empty()
        {
            return Err(Kind::Semantics);
        }
    } else if matches!(
        semantics,
        TransferSemantics::KnownProcess
            | TransferSemantics::ConstructorProcess
            | TransferSemantics::BoundaryProcess
    ) {
        return Err(Kind::Semantics);
    }
    let bytecode = semantics == TransferSemantics::Native
        && matches!(node.frame.kind, CodeKind::Runtime | CodeKind::Initcode);
    if bytecode != node.frame.basic_block.is_some()
        || node
            .transfer
            .as_ref()
            .is_some_and(|transfer| transfer.visited_block && !bytecode)
    {
        return Err(Kind::BlockVisit);
    }
    if let Some(transfer) = &node.transfer
        && (transfer.semantics != semantics
            || (semantics == TransferSemantics::Native && transfer.process.is_some())
            || transfer
                .process
                .as_ref()
                .is_some_and(|process| transfer.complete && !process.unresolved.is_empty()))
    {
        return Err(Kind::Semantics);
    }
    Ok(())
}

impl AbstractSummary {
    pub(crate) fn validate_control(&self, index: usize) -> Result<(), ReportValidationFailure> {
        let summary = self;
        let pool = |kind, id, length| {
            if id < length {
                Ok(())
            } else {
                Err(ReportValidationFailure::Pool(crate::PoolReferenceFailure {
                    kind,
                    index: id,
                    length,
                }))
            }
        };
        let cfg = &summary.regional_cfg;
        let failure = |id, call, kind| {
            ReportValidationFailure::Regional(RegionalValidationFailure {
                summary: index,
                id,
                call,
                kind,
            })
        };
        let node_ref = |id| pool(PoolReferenceKind::RegionalNode, id, cfg.nodes.len());
        let call_ref = |id| pool(PoolReferenceKind::RegionalCall, id, cfg.calls.len());
        let procedure_ref = |id| {
            pool(
                PoolReferenceKind::AbstractProcedure,
                id,
                summary.procedures.len(),
            )
        };
        for (position, node) in cfg.nodes.iter().enumerate() {
            if node.id != position {
                return Err(failure(node.id, false, Kind::Identity));
            }
            procedure_ref(node.procedure)?;
            let procedure = &summary.procedures[node.procedure];
            if procedure.process != node.frame.process {
                return Err(failure(node.id, false, Kind::Semantics));
            }
            for target in &node.successors {
                node_ref(*target)?;
            }
            block_evidence(node, summary.procedures[node.procedure].semantics)
                .map_err(|kind| failure(node.id, false, kind))?;
            if let Some(transfer) = &node.transfer {
                if let Some(process) = &transfer.process {
                    for creation in &process.creations {
                        if !matches!(
                            creation.action,
                            crate::ProcessAction::Create | crate::ProcessAction::Create2
                        ) {
                            return Err(failure(node.id, false, Kind::Semantics));
                        }
                    }
                }
                if transfer.current && transfer.input_revision != node.input_revision {
                    return Err(failure(node.id, false, Kind::Revision));
                }
                if transfer.current && node.excluded {
                    return Err(failure(node.id, false, Kind::Excluded));
                }
                for edge in &transfer.successors {
                    node_ref(edge.to)?;
                }
                for call in &transfer.calls {
                    call_ref(*call)?;
                    if cfg.calls[*call].from != node.id {
                        return Err(failure(node.id, false, Kind::CallOrigin));
                    }
                }
            }
        }
        for (position, call) in cfg.calls.iter().enumerate() {
            if call.id != position {
                return Err(failure(call.id, true, Kind::Identity));
            }
            node_ref(call.from)?;
            procedure_ref(call.callee)?;
            if call.action.is_some() != call.target.is_some() {
                return Err(failure(call.id, true, Kind::Semantics));
            }
            if call.scope.is_some() != call.action.is_some() {
                return Err(failure(call.id, true, Kind::Semantics));
            }
            for entry in &call.entries {
                node_ref(*entry)?;
                if cfg.nodes[*entry].procedure != call.callee {
                    return Err(failure(call.id, true, Kind::CalleeEntry));
                }
            }
            for delivery in &call.deliveries {
                if !delivery.creation.is_empty()
                    && !matches!(
                        call.action,
                        Some(crate::ProcessAction::Create | crate::ProcessAction::Create2)
                    )
                {
                    return Err(failure(call.id, true, Kind::Semantics));
                }
                for creation in &delivery.creation {
                    super::process::completion(creation)
                        .map_err(ReportValidationFailure::Process)?;
                }
                if delivery.current && delivery.input_revision != call.input_revision {
                    return Err(failure(call.id, true, Kind::Revision));
                }
                for edge in &delivery.successors {
                    node_ref(edge.to)?;
                }
            }
        }
        Ok(())
    }
}

impl Validation<'_> {
    pub(super) fn regional(
        &self,
        index: usize,
        summary: &AbstractSummary,
    ) -> Result<(), ReportValidationFailure> {
        summary.validate_control(index)
    }
}
