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
    }
}

fn block_evidence(node: &RegionalNode, semantics: TransferSemantics) -> Result<(), Kind> {
    // Non-program modes can retain a native placeholder block key. It is not
    // captured source, including when invalid delegation retains a raw hash.
    if !compatible_code(node.frame.kind, semantics) {
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
    if let Some(transfer) = &node.transfer {
        if transfer.semantics != semantics
            || (semantics == TransferSemantics::Native && transfer.process.is_some())
            || transfer
                .process
                .as_ref()
                .is_some_and(|process| transfer.complete && !process.unresolved.is_empty())
        {
            return Err(Kind::Semantics);
        }
    }
    Ok(())
}

impl Validation<'_> {
    pub(super) fn regional(
        &self,
        index: usize,
        summary: &AbstractSummary,
    ) -> Result<(), ReportValidationFailure> {
        let cfg = &summary.regional_cfg;
        let failure = |id, call, kind| {
            ReportValidationFailure::Regional(RegionalValidationFailure {
                summary: index,
                id,
                call,
                kind,
            })
        };
        let node_ref = |id| self.pool(PoolReferenceKind::RegionalNode, id, cfg.nodes.len());
        let call_ref = |id| self.pool(PoolReferenceKind::RegionalCall, id, cfg.calls.len());
        let procedure_ref = |id| {
            self.pool(
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
            for target in &node.successors {
                node_ref(*target)?;
            }
            block_evidence(node, summary.procedures[node.procedure].semantics)
                .map_err(|kind| failure(node.id, false, kind))?;
            if let Some(transfer) = &node.transfer {
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
            for entry in &call.entries {
                node_ref(*entry)?;
                if cfg.nodes[*entry].procedure != call.callee {
                    return Err(failure(call.id, true, Kind::CalleeEntry));
                }
            }
            for delivery in &call.deliveries {
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
