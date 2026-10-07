//! Materialize a certified callee graph under a different suspended caller.

use super::{Certificate, Node};
use crate::analysis::{CallStack, Continuation, MachinePayload, transfer::WorkBudget};
use crate::{
    domain::{
        Value,
        symbolic::{ExprId, ExprLimits},
    },
    world::ByteArray,
};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::analysis) struct Replay<'a> {
    certificate: &'a Certificate,
    prefix: CallStack,
    continuation: Continuation,
    prefix_cost: usize,
    renaming: BTreeMap<ExprId, ExprId>,
    limits: ExprLimits,
}

impl<'a> Replay<'a> {
    pub fn new(
        certificate: &'a Certificate,
        payload: &MachinePayload,
        budget: &mut WorkBudget,
        limits: ExprLimits,
    ) -> Option<Self> {
        if !budget.charge(payload.work_size()) {
            return None;
        }
        let mut prefix = payload.call_stack.clone();
        let child = prefix
            .pop_child()
            .expect("summary replay requires a child frame");
        // Only callee inputs are bound. An older result in a suspended caller
        // must not prevent this invocation from receiving fresh local symbols.
        let mut external = payload.relations.fresh_leaves();
        let mut collect = |value: &Value| collect_value(value, &mut external);
        payload.active().visit_values(&mut collect);
        payload.store.visit_values(&mut collect);
        let mut internal = BTreeSet::new();
        for node in &certificate.nodes {
            let cost = node.entry.work_size().saturating_add(node.exit.work_size());
            if !budget.charge(cost) {
                return None;
            }
            internal.extend(node.entry.fresh_leaves());
            internal.extend(node.exit.fresh_leaves());
            for value in &node.exit_stack {
                collect_value(value, &mut internal);
            }
            for call in &node.completed_calls {
                if !budget.charge(
                    call.payload
                        .work_size()
                        .saturating_add(call.data.work_size())
                        .saturating_add(call.output_store.work_size())
                        .saturating_add(call.output_data.work_size()),
                ) {
                    return None;
                }
                internal.extend(call.payload.fresh_leaves());
                call.data
                    .visit_values(&mut |v| collect_value(v, &mut internal));
                call.output_data
                    .visit_values(&mut |v| collect_value(v, &mut internal));
                call.output_store
                    .visit_values(&mut |v| collect_value(v, &mut internal));
            }
        }
        for terminal in &certificate.terminals {
            if !budget.charge(
                terminal
                    .payload
                    .work_size()
                    .saturating_add(terminal.raw_data.work_size()),
            ) {
                return None;
            }
            internal.extend(terminal.payload.fresh_leaves());
            terminal
                .raw_data
                .visit_values(&mut |v| collect_value(v, &mut internal));
        }
        let mut renaming = BTreeMap::new();
        for local in internal.difference(&external) {
            renaming.insert(local.clone(), ExprId::fresh()?);
        }
        Some(Self {
            certificate,
            prefix,
            continuation: child.continuation,
            prefix_cost: payload.work_size(),
            renaming,
            limits,
        })
    }

    fn payload(&self, relative: &MachinePayload) -> Option<MachinePayload> {
        let mut relative = relative.clone();
        relative.rename_fresh(&self.renaming, self.limits).ok()?;
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
            relations: relative.relations.clone(),
            call_stack,
            store: relative.store.clone(),
        };
        result.normalize();
        Some(result)
    }

    fn value(&self, value: &mut Value) -> bool {
        value.forget_identity();
        if let Some(expression) = value.expression() {
            let Ok(expression) = expression.rename_fresh(&self.renaming, self.limits) else {
                return false;
            };
            *value = value.clone().with_expression(expression);
        }
        true
    }

    fn bytes(&self, bytes: &mut ByteArray) -> bool {
        let mut valid = true;
        bytes.update_values(&mut |value| valid &= self.value(value));
        valid
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
                    .saturating_add(call.output_data.work_size())
                    .saturating_add(self.prefix_cost)
            },
        );
        if !budget.charge(cost) {
            return None;
        }
        let mut exit_stack = source.exit_stack.clone();
        for value in &mut exit_stack {
            if !self.value(value) {
                return None;
            }
        }
        let mut completed_calls = Vec::new();
        for call in &source.completed_calls {
            let mut call = call.clone();
            call.payload = self.payload(&call.payload)?;
            if !self.bytes(&mut call.data) || !self.bytes(&mut call.output_data) {
                return None;
            }
            let mut valid = true;
            call.output_store
                .update_values(&mut |value| valid &= self.value(value));
            if !valid {
                return None;
            }
            completed_calls.push(call);
        }
        Some(Node {
            entry: self.payload(&source.entry)?,
            exit: self.payload(&source.exit)?,
            exit_stack,
            executed_pcs: source.executed_pcs.clone(),
            diagnostics: source.diagnostics.clone(),
            completed_calls,
        })
    }

    pub fn terminal_payload(
        &self,
        index: usize,
        budget: &mut WorkBudget,
    ) -> Option<(MachinePayload, ByteArray)> {
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
        let mut data = terminal.raw_data.clone();
        if !self.bytes(&mut data) {
            return None;
        }
        Some((self.payload(&terminal.payload)?, data))
    }
}

fn collect_value(value: &Value, into: &mut BTreeSet<ExprId>) {
    if let Some(expression) = value.expression() {
        into.extend(expression.fresh_leaves());
    }
}
