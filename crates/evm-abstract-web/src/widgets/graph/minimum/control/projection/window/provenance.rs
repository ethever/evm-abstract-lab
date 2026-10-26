//! Resolve crossing endpoints from their actual receipt, not a representative
//! card member. Missing evidence is an error, never a manufactured node zero.

use std::collections::BTreeMap;

use super::super::super::scope::{ControlOrigin, FlowOrigin, ProjectionError, ProjectionTarget};
use evm_abstract_protocol::{ProcessLocation, RegionalCall, RegionalCfg, RegionalNode};

pub(super) struct Provenance<'a> {
    summary: usize,
    nodes: BTreeMap<usize, &'a RegionalNode>,
    calls: BTreeMap<usize, &'a RegionalCall>,
}

impl<'a> Provenance<'a> {
    pub(super) fn new(summary: usize, cfg: &'a RegionalCfg) -> Self {
        Self {
            summary,
            nodes: cfg.nodes.iter().map(|node| (node.id, node)).collect(),
            calls: cfg.calls.iter().map(|call| (call.id, call)).collect(),
        }
    }

    fn target(
        &self,
        id: usize,
        location: Option<ProcessLocation>,
    ) -> Result<ProjectionTarget, ProjectionError> {
        let node = self
            .nodes
            .get(&id)
            .ok_or(ProjectionError::MissingFlowOrigin)?;
        let location = location.filter(|&location| {
            !node
                .frame
                .process
                .as_ref()
                .is_some_and(|frame| frame.location == location)
        });
        if let Some(location) = location {
            let admitted = node
                .transfer
                .as_ref()
                .and_then(|transfer| transfer.process.as_ref())
                .and_then(|process| process.control.as_ref())
                .is_some_and(|control| {
                    control.through.contains(&location) || control.boundaries.contains(&location)
                });
            if !admitted {
                return Err(ProjectionError::MissingFlowOrigin);
            }
        }
        Ok(ProjectionTarget {
            summary: self.summary,
            procedure: node.procedure,
            origin: ControlOrigin { node: id, location },
        })
    }

    pub(super) fn endpoints(
        &self,
        origin: FlowOrigin,
    ) -> Result<(ProjectionTarget, ProjectionTarget), ProjectionError> {
        match origin {
            FlowOrigin::Source { node, from, to, .. } => {
                Ok((self.target(node, Some(from))?, self.target(node, Some(to))?))
            }
            FlowOrigin::Regional { from, to } => {
                Ok((self.target(from, None)?, self.target(to, None)?))
            }
            FlowOrigin::CallSite { call } => {
                let call = self
                    .calls
                    .get(&call)
                    .ok_or(ProjectionError::MissingFlowOrigin)?;
                let site = self.target(call.from, None)?;
                Ok((site, site))
            }
            FlowOrigin::Call { call, entry } => {
                let call = self
                    .calls
                    .get(&call)
                    .ok_or(ProjectionError::MissingFlowOrigin)?;
                Ok((self.target(call.from, None)?, self.target(entry, None)?))
            }
            FlowOrigin::Delivery {
                call,
                delivery,
                successor,
            } => {
                let call = self
                    .calls
                    .get(&call)
                    .ok_or(ProjectionError::MissingFlowOrigin)?;
                if !call.deliveries.get(delivery).is_some_and(|receipt| {
                    receipt.successors.iter().any(|edge| edge.to == successor)
                }) {
                    return Err(ProjectionError::MissingFlowOrigin);
                }
                Ok((self.target(call.from, None)?, self.target(successor, None)?))
            }
            FlowOrigin::Jump { node, target, .. } => Ok((
                self.target(node, Some(ProcessLocation::JumpTargets))?,
                self.target(node, Some(ProcessLocation::Block(target)))?,
            )),
            FlowOrigin::Topology { .. }
            | FlowOrigin::Admission { .. }
            | FlowOrigin::DecodedJump { .. } => Err(ProjectionError::MissingFlowOrigin),
        }
    }
}
