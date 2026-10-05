//! Materialize a certified callee graph under a different suspended caller.

use super::{Certificate, Node};
use crate::analysis::{CallStack, Continuation, MachinePayload, transfer::WorkBudget};

pub(in crate::analysis) struct Replay<'a> {
    certificate: &'a Certificate,
    prefix: CallStack,
    continuation: Continuation,
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
        let mut prefix = payload.call_stack.clone();
        let child = prefix
            .pop_child()
            .expect("summary replay requires a child frame");
        Some(Self {
            certificate,
            prefix,
            continuation: child.continuation,
            prefix_cost: payload.work_size(),
        })
    }

    fn payload(&self, relative: &MachinePayload) -> MachinePayload {
        let mut call_stack = self.prefix.clone();
        call_stack.push_child(
            relative
                .call_stack
                .root()
                .clone()
                .into_child(self.continuation.clone()),
        );
        for child in relative.call_stack.children() {
            call_stack.push_child(child.clone());
        }
        let mut result = MachinePayload {
            call_stack,
            store: relative.store.clone(),
        };
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
