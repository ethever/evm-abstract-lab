//! Retained receipt qualifiers and coordinates, not a control interpreter.
use super::{AnalysisReport, Location, ReportValidationFailure, invalid, location, source_fork};
use crate::{AbstractSummary, EdgeKind, ProcessControlRegion, ProcessFrameKey};
use std::collections::BTreeSet;

impl AnalysisReport {
    pub(super) fn control_summary(
        &self,
        summary: &AbstractSummary,
        fork: crate::Fork,
    ) -> Result<(), ReportValidationFailure> {
        for procedure in &summary.procedures {
            if let Some(key) = &procedure.process {
                source_fork(key, fork)?;
                self.control_location(key)?;
            }
        }
        for node in &summary.regional_cfg.nodes {
            let Some(key) = &node.frame.process else {
                continue;
            };
            self.control_location(key)?;
            let Some(transfer) = &node.transfer else {
                continue;
            };
            let region = transfer
                .process
                .as_ref()
                .and_then(|process| process.control.as_ref());
            if let Some(region) = region {
                self.control_region(key, region)?;
            }
            for edge in &transfer.successors {
                if edge.kind != EdgeKind::ProcessControl {
                    continue;
                }
                let target = summary
                    .regional_cfg
                    .nodes
                    .get(edge.to)
                    .ok_or_else(invalid)?;
                let target_key = target.frame.process.as_ref().ok_or_else(invalid)?;
                if !key.same_context(target_key)
                    || region.is_none_or(|region| !region.boundaries.contains(&target_key.location))
                {
                    return Err(invalid());
                }
            }
            if transfer.complete
                && let Some(region) = region
            {
                for boundary in &region.boundaries {
                    if !transfer.successors.iter().any(|edge| {
                        edge.kind == EdgeKind::ProcessControl
                            && summary
                                .regional_cfg
                                .nodes
                                .get(edge.to)
                                .and_then(|target| target.frame.process.as_ref())
                                .is_some_and(|target| {
                                    key.same_context(target) && target.location == *boundary
                                })
                    }) {
                        return Err(invalid());
                    }
                }
            }
        }
        Ok(())
    }

    fn control_region(
        &self,
        key: &ProcessFrameKey,
        region: &ProcessControlRegion,
    ) -> Result<(), ReportValidationFailure> {
        let relation = self
            .process_controls
            .get(region.relation)
            .ok_or_else(invalid)?;
        if key.epoch != relation.epoch
            || key.source != relation.source
            || key.location != region.entry
        {
            return Err(invalid());
        }
        let mut points = BTreeSet::new();
        for point in region.through.iter().chain(&region.boundaries) {
            if !location(*point, relation.blocks.len()) || !points.insert(*point) {
                return Err(invalid());
            }
        }
        if !points.contains(&region.entry) {
            return Err(invalid());
        }
        for point in &region.through {
            match point {
                Location::Block(index) => {
                    let block = &relation.blocks[*index];
                    if block.boundary
                        || block
                            .successors
                            .iter()
                            .flatten()
                            .any(|edge| !points.contains(&edge.to))
                    {
                        return Err(invalid());
                    }
                }
                Location::JumpTargets if region.entry == Location::JumpTargets => {
                    if relation
                        .jump_targets
                        .iter()
                        .any(|index| !points.contains(&Location::Block(*index)))
                    {
                        return Err(invalid());
                    }
                }
                _ => return Err(invalid()),
            }
        }
        for point in &region.boundaries {
            if match point {
                Location::Block(index) => !relation.blocks[*index].boundary,
                Location::JumpTargets => region.entry == Location::JumpTargets,
                Location::End => false,
                Location::Any => true,
            } {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
