//! Instantiate every scalar, source and state root before installing any graph.
//! One map belongs to one replay; certified snapshots are never mutated.

use super::{Certificate, Node, Terminal};
use crate::{
    analysis::{CallStack, Continuation, MachineError, MachinePayload},
    domain::{
        AbstractValue,
        symbolic::{ExprId, ExprLimits},
    },
    resource::WorkBudget,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub(in crate::analysis) struct Prepared {
    pub nodes: Vec<Node>,
    pub terminals: Vec<Terminal>,
    pub renaming: BTreeMap<ExprId, ExprId>,
}

pub(in crate::analysis) struct Replay<'a> {
    certificate: &'a Certificate,
    prefix: CallStack,
    continuation: Continuation,
    prefix_cost: usize,
    renaming: BTreeMap<ExprId, ExprId>,
    limits: ExprLimits,
}

fn charge(budget: &mut WorkBudget, cost: usize) -> Result<(), MachineError> {
    if cost == usize::MAX || !budget.charge(cost) {
        Err(MachineError::work_failure())
    } else {
        Ok(())
    }
}

impl<'a> Replay<'a> {
    pub fn new(
        certificate: &'a Certificate,
        payload: &MachinePayload,
        budget: &mut WorkBudget,
        limits: ExprLimits,
    ) -> Result<Self, MachineError> {
        let input = certificate.nodes.first().ok_or(MachineError::Shape)?;
        charge(
            budget,
            payload.work_size().saturating_add(input.entry.work_size()),
        )?;
        let mut prefix = payload.call_stack.clone();
        let child = prefix.pop_child().ok_or(MachineError::Shape)?;
        // The certified entry is the binding boundary. Old results in suspended
        // caller frames are not new callee outputs; source tokens reachable from
        // the actual callee inputs (including rollback) must remain bound.
        let external = input.entry.fresh_leaves();
        let mut internal = BTreeSet::new();
        for node in &certificate.nodes {
            charge(budget, node.work_size())?;
            internal.extend(node.entry.fresh_leaves());
            internal.extend(node.exit.fresh_leaves());
            for value in &node.exit_stack {
                charge(budget, 0)?;
                collect_value(value, &mut internal);
            }
            for call in &node.completed_calls {
                charge(budget, 0)?;
                internal.extend(call.payload.fresh_leaves());
                internal.extend(call.data.fresh_leaves());
                internal.extend(call.output_data.fresh_leaves());
                internal.extend(call.output_store.fresh_leaves());
            }
        }
        for terminal in &certificate.terminals {
            charge(budget, terminal.work_size())?;
            internal.extend(terminal.payload.fresh_leaves());
            internal.extend(terminal.raw_data.fresh_leaves());
            internal.extend(terminal.output.data.fresh_leaves());
            internal.extend(terminal.output.store.fresh_leaves());
        }
        let mut renaming = BTreeMap::new();
        for local in internal.difference(&external) {
            charge(budget, 1)?;
            renaming.insert(
                local.clone(),
                ExprId::fresh().ok_or(MachineError::IdentityExhausted)?,
            );
        }
        Ok(Self {
            certificate,
            prefix,
            continuation: child.continuation,
            prefix_cost: payload.work_size(),
            renaming,
            limits,
        })
    }

    fn payload(&self, relative: &MachinePayload) -> Result<MachinePayload, MachineError> {
        let mut relative = relative.clone();
        relative.rename_fresh(&self.renaming, self.limits)?;
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
            relations: relative.relations,
            call_stack,
            store: relative.store,
        };
        result.normalize();
        Ok(result)
    }

    fn value(&self, value: &mut AbstractValue) -> Result<(), MachineError> {
        value.forget_identity();
        if let Some(expression) = value.expression() {
            *value = value
                .clone()
                .with_expression(expression.rename_fresh(&self.renaming, self.limits)?);
        }
        Ok(())
    }

    fn node(&self, source: &Node, budget: &mut WorkBudget) -> Result<Node, MachineError> {
        let cost = source
            .work_size()
            .saturating_add(
                self.prefix_cost
                    .saturating_mul(source.completed_calls.len().saturating_add(2)),
            )
            .saturating_mul(4);
        charge(budget, cost)?;
        let mut exit_stack = source.exit_stack.clone();
        for value in &mut exit_stack {
            charge(budget, 0)?;
            self.value(value)?;
        }
        let mut completed_calls = Vec::new();
        for call in &source.completed_calls {
            charge(budget, 0)?;
            let mut call = call.clone();
            call.payload = self.payload(&call.payload)?;
            call.data.rename_fresh(&self.renaming, self.limits)?;
            call.output_data.rename_fresh(&self.renaming, self.limits)?;
            call.output_store
                .rename_fresh(&self.renaming, self.limits)?;
            completed_calls.push(call);
        }
        Ok(Node {
            entry: self.payload(&source.entry)?,
            exit: self.payload(&source.exit)?,
            exit_stack,
            executed_pcs: source.executed_pcs.clone(),
            instruction_progress: source.instruction_progress.clone(),
            // This receipt stores numeric/provenance projections only.
            instruction_values: Arc::clone(&source.instruction_values),
            diagnostics: source.diagnostics.clone(),
            completed_calls,
        })
    }

    fn terminal(
        &self,
        source: &Terminal,
        budget: &mut WorkBudget,
    ) -> Result<Terminal, MachineError> {
        charge(
            budget,
            source
                .work_size()
                .saturating_add(self.prefix_cost)
                .saturating_mul(4),
        )?;
        let mut terminal = source.clone();
        terminal.payload = self.payload(&terminal.payload)?;
        terminal
            .raw_data
            .rename_fresh(&self.renaming, self.limits)?;
        terminal
            .output
            .data
            .rename_fresh(&self.renaming, self.limits)?;
        terminal
            .output
            .store
            .rename_fresh(&self.renaming, self.limits)?;
        Ok(terminal)
    }

    /// Any renaming or allocation failure drops this entire temporary graph.
    pub fn prepare(self, budget: &mut WorkBudget) -> Result<Prepared, MachineError> {
        charge(
            budget,
            self.certificate
                .nodes
                .len()
                .saturating_add(self.certificate.terminals.len()),
        )?;
        let nodes = self
            .certificate
            .nodes
            .iter()
            .map(|node| self.node(node, budget))
            .collect::<Result<_, _>>()?;
        let terminals = self
            .certificate
            .terminals
            .iter()
            .map(|terminal| self.terminal(terminal, budget))
            .collect::<Result<_, _>>()?;
        charge(budget, 0)?;
        Ok(Prepared {
            nodes,
            terminals,
            renaming: self.renaming,
        })
    }
}

fn collect_value(value: &AbstractValue, into: &mut BTreeSet<ExprId>) {
    if let Some(expression) = value.expression() {
        into.extend(expression.fresh_leaves());
    }
}

#[cfg(test)]
mod tests;
