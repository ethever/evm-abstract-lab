//! Regional IDs are record-local table indexes, never native CFG/SSA IDs.
use super::Validation;
use crate::{
    AbstractSummary, CodeKind, PoolReferenceKind, RegionalNode, RegionalValidationFailure,
    RegionalValidationKind as Kind, ReportValidationFailure,
};

#[cfg(test)]
mod tests;

fn block_evidence(node: &RegionalNode) -> Result<(), Kind> {
    // Non-program modes can retain a native placeholder block key. It is not
    // captured source, including when invalid delegation retains a raw hash.
    let bytecode = matches!(node.frame.kind, CodeKind::Runtime | CodeKind::Initcode);
    if bytecode != node.frame.basic_block.is_some()
        || node
            .transfer
            .as_ref()
            .is_some_and(|transfer| transfer.visited_block && !bytecode)
    {
        return Err(Kind::BlockVisit);
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
            block_evidence(node).map_err(|kind| failure(node.id, false, kind))?;
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
