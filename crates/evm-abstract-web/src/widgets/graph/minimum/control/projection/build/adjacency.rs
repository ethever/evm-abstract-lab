//! Regional categories alone do not establish source-level adjacency. Macro
//! composition and accumulated topology remain in the separate model projection.

use super::{Builder, EdgeLevel, GraphLevel};
use evm_abstract_protocol::{EdgeKind, ProcessAction, ProcessLocation, RegionalNode};

impl Builder<'_> {
    pub(super) fn transfer_level(
        &self,
        from: &RegionalNode,
        to: &RegionalNode,
        kind: EdgeKind,
    ) -> Option<EdgeLevel> {
        match self.projection.level {
            GraphLevel::Model => Some(EdgeLevel::Model),
            GraphLevel::AbstractBoundary => Some(EdgeLevel::AbstractBoundary),
            GraphLevel::Source => {
                if let EdgeKind::Process(action) = kind {
                    return self.continuation_level(from, to, Some(action));
                }
                if matches!(kind, EdgeKind::Failure | EdgeKind::Revert) {
                    return Some(EdgeLevel::ExceptionBoundary);
                }
                if !matches!(
                    kind,
                    EdgeKind::Fallthrough
                        | EdgeKind::Jump
                        | EdgeKind::BranchTrue
                        | EdgeKind::BranchFalse
                ) {
                    return None;
                }
                let frame = from.frame.process.as_ref()?;
                let target = to.frame.process.as_ref()?;
                if !frame.same_context(target) {
                    return None;
                }
                let ProcessLocation::Block(block) = frame.location else {
                    return None;
                };
                if let Some(relation) = self.directories.values().find(|relation| {
                    relation.epoch == frame.epoch && relation.source == frame.source
                }) {
                    relation
                        .blocks
                        .get(block)?
                        .successors
                        .iter()
                        .flatten()
                        .any(|edge| edge.to == target.location && edge.kind == kind)
                        .then_some(EdgeLevel::Source)
                } else {
                    // A boundary-only program may have no exported pure-region
                    // directory. Its actual non-macro typed Body edge remains
                    // evidence; directory presence cannot manufacture it.
                    from.transfer
                        .as_ref()
                        .is_some_and(|transfer| {
                            transfer
                                .process
                                .as_ref()
                                .is_none_or(|process| process.control.is_none())
                        })
                        .then_some(EdgeLevel::Source)
                }
            }
        }
    }

    pub(super) fn call_level(
        &self,
        from: &RegionalNode,
        action: Option<ProcessAction>,
    ) -> Option<EdgeLevel> {
        match self.projection.level {
            GraphLevel::Model => Some(EdgeLevel::Model),
            GraphLevel::AbstractBoundary => Some(EdgeLevel::AbstractBoundary),
            GraphLevel::Source => {
                let frame = from.frame.process.as_ref()?;
                let ProcessLocation::Block(block) = frame.location else {
                    return None;
                };
                let program = self.report.process_program(&frame.source)?;
                let opcode = program.blocks.get(block)?.instructions.last()?.opcode;
                let matches = match action {
                    Some(ProcessAction::Call) => opcode == 0xf1,
                    Some(ProcessAction::StaticCall) => opcode == 0xfa,
                    Some(ProcessAction::DelegateCall) => opcode == 0xf4,
                    Some(ProcessAction::CallCode) => opcode == 0xf2,
                    Some(ProcessAction::Create) => opcode == 0xf0,
                    Some(ProcessAction::Create2) => opcode == 0xf5,
                    None => matches!(opcode, 0xf0 | 0xf1 | 0xf2 | 0xf4 | 0xf5 | 0xfa),
                };
                matches.then_some(EdgeLevel::CallBoundary)
            }
        }
    }

    pub(super) fn continuation_level(
        &self,
        caller: &RegionalNode,
        target: &RegionalNode,
        action: Option<ProcessAction>,
    ) -> Option<EdgeLevel> {
        let level = self.call_level(caller, action)?;
        if self.projection.level != GraphLevel::Source {
            return Some(level);
        }
        let frame = caller.frame.process.as_ref()?;
        let continuation = target.frame.process.as_ref()?;
        if !frame.same_context(continuation) {
            return None;
        }
        let ProcessLocation::Block(block) = frame.location else {
            return None;
        };
        let program = self.report.process_program(&frame.source)?;
        let expected = self
            .directories
            .values()
            .find(|relation| relation.epoch == frame.epoch && relation.source == frame.source)
            .and_then(|relation| relation.blocks.get(block))
            .and_then(|block| block.continuation)
            .unwrap_or({
                if block + 1 < program.blocks.len() {
                    ProcessLocation::Block(block + 1)
                } else {
                    ProcessLocation::End
                }
            });
        // Decoded continuation checks placement only. The caller must provide
        // an actual local-action or delivery receipt before adding this edge.
        (continuation.location == expected).then_some(EdgeLevel::CallBoundary)
    }
}
