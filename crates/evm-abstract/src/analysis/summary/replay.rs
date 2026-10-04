//! Materialize a certified callee graph under a different suspended caller.

use super::{Certificate, Node};
use crate::analysis::{Continuation, Frame, MachinePayload, transfer::WorkBudget};

pub(in crate::analysis) struct Replay<'a> {
    certificate: &'a Certificate,
    prefix: Vec<Frame>,
    continuation: Option<Continuation>,
    prefix_cost: usize,
}

impl<'a> Replay<'a> {
    pub fn new(
        certificate: &'a Certificate,
        payload: &MachinePayload,
        budget: &mut WorkBudget,
    ) -> Option<Self> {
        if !budget.charge(payload.work_size()) {
            return None;
        }
        Some(Self {
            certificate,
            prefix: payload.frames[..payload.frames.len() - 1].to_vec(),
            continuation: payload.active().continuation.clone(),
            prefix_cost: payload.work_size(),
        })
    }

    fn payload(&self, relative: &MachinePayload) -> MachinePayload {
        let mut result = relative.clone();
        result.frames[0].continuation = self.continuation.clone();
        let mut frames = self.prefix.clone();
        frames.append(&mut result.frames);
        result.frames = frames;
        result.normalize();
        result
    }

    pub fn node(&self, index: usize, budget: &mut WorkBudget) -> Option<Node> {
        let source = &self.certificate.nodes[index];
        let cost = source.completed_calls.iter().fold(
            source
                .entry
                .work_size()
                .saturating_add(source.exit.work_size())
                .saturating_add(self.prefix_cost.saturating_mul(2))
                .saturating_add(source.executed_pcs.len()),
            |cost, call| {
                cost.saturating_add(call.payload.work_size())
                    .saturating_add(call.data.work_size())
                    .saturating_add(call.output_store.work_size())
                    .saturating_add(self.prefix_cost)
            },
        );
        if !budget.charge(cost) {
            return None;
        }
        Some(Node {
            entry: self.payload(&source.entry),
            exit: self.payload(&source.exit),
            exit_stack: source.exit_stack.clone(),
            executed_pcs: source.executed_pcs.clone(),
            diagnostics: source.diagnostics.clone(),
            completed_calls: source
                .completed_calls
                .iter()
                .cloned()
                .map(|mut call| {
                    call.payload = self.payload(&call.payload);
                    call
                })
                .collect(),
        })
    }

    pub fn terminal_payload(
        &self,
        index: usize,
        budget: &mut WorkBudget,
    ) -> Option<MachinePayload> {
        let terminal = &self.certificate.terminals[index];
        if !budget.charge(
            terminal
                .payload
                .work_size()
                .saturating_add(self.prefix_cost)
                .saturating_add(terminal.output.data.work_size()),
        ) {
            return None;
        }
        Some(self.payload(&terminal.payload))
    }
}
