//! One worklist spans every active and suspended execution frame.

use super::{
    ConfigError, Diagnostic, Limit, Status,
    machine::{
        ExecutionConfig, FrontierReason, MachineEdge, MachineFrontier, MachineOutcome,
        MachineState, OutcomeKind, WorldAnalysis,
    },
    transfer,
};
use crate::world::{Entry, World};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(super) fn run_world(
    world: World,
    entry: Entry,
    config: ExecutionConfig,
) -> Result<WorldAnalysis, ConfigError> {
    let domain = config.domain()?;
    let mut result = WorldAnalysis {
        world,
        entry,
        config,
        states: Vec::new(),
        edges: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        status: Status::Converged,
        transfers: 0,
        work: 0,
    };
    let mut budget = transfer::WorkBudget::new(result.config.max_work);
    let initial = match transfer::initial(&result.world, &result.entry) {
        Ok(payload) => payload,
        Err(reason) => {
            result.frontiers.push(MachineFrontier {
                from: None,
                target: None,
                pc: None,
                reason,
            });
            result.status = Status::Incomplete;
            return Ok(result);
        }
    };
    result.outcomes.push(MachineOutcome {
        state: 0,
        kind: OutcomeKind::Failure,
        data: crate::world::ByteArray::empty(),
        store: initial.store.clone(),
    });
    let key = initial.key();
    result.states.push(MachineState {
        id: 0,
        key: key.clone(),
        entry: initial,
        exit: None,
        exit_stack: Vec::new(),
        executed_pcs: Vec::new(),
    });
    let mut ids = BTreeMap::from([(key, 0)]);
    let mut queue = VecDeque::from([0]);
    let mut queued = BTreeSet::from([0]);
    let mut edges = BTreeSet::new();
    let mut diagnostics = BTreeSet::new();

    while let Some(id) = queue.pop_front() {
        queued.remove(&id);
        if result.transfers >= result.config.analysis.max_transfers || budget.exhausted() {
            let reason = if budget.exhausted() {
                FrontierReason::Work
            } else {
                FrontierReason::Budget(Limit::Transfers)
            };
            for pending in std::iter::once(id).chain(queue.iter().copied()) {
                result.frontiers.push(MachineFrontier {
                    from: None,
                    target: Some(result.states[pending].key.clone()),
                    pc: None,
                    reason: reason.clone(),
                });
            }
            break;
        }
        if !budget.charge(result.states[id].entry.work_size()) {
            for pending in std::iter::once(id).chain(queue.iter().copied()) {
                result.frontiers.push(MachineFrontier {
                    from: None,
                    target: Some(result.states[pending].key.clone()),
                    pc: None,
                    reason: FrontierReason::Work,
                });
            }
            break;
        }
        result.transfers += 1;
        let mut execution = transfer::execute(
            &result.world,
            &result.entry,
            result.states[id].entry.clone(),
            &result.config,
            domain,
            &mut budget,
        );
        execution.payload.normalize();
        result.states[id].exit_stack = execution.payload.active().stack.clone();
        result.states[id].executed_pcs = execution.executed_pcs;
        result.states[id].exit = Some(execution.payload);
        for (pc, kind) in execution.diagnostics {
            diagnostics.insert(Diagnostic {
                state: id,
                pc,
                kind,
            });
        }
        for (pc, reason, target) in execution.frontiers {
            result.frontiers.push(MachineFrontier {
                from: Some(id),
                target,
                pc: Some(pc),
                reason,
            });
        }
        for outcome in execution.outcomes {
            result.outcomes.push(MachineOutcome {
                state: id,
                kind: outcome.kind,
                data: outcome.data,
                store: outcome.store,
            });
        }
        for mut successor in execution.successors {
            successor.payload.normalize();
            let key = successor.payload.key();
            let to = if let Some(existing) = ids.get(&key).copied() {
                let join_cost = successor
                    .payload
                    .work_size()
                    .saturating_add(result.states[existing].entry.work_size());
                if !budget.charge(join_cost) {
                    result.frontiers.push(MachineFrontier {
                        from: Some(id),
                        target: Some(key),
                        pc: None,
                        reason: FrontierReason::Work,
                    });
                    continue;
                }
                let joined = result.states[existing]
                    .entry
                    .join(&successor.payload, domain);
                if joined != result.states[existing].entry {
                    result.states[existing].entry = joined;
                    if queued.insert(existing) {
                        queue.push_back(existing);
                    }
                }
                existing
            } else {
                if result.states.len() >= result.config.analysis.max_states {
                    result.frontiers.push(MachineFrontier {
                        from: Some(id),
                        target: Some(key),
                        pc: None,
                        reason: FrontierReason::Budget(Limit::States),
                    });
                    continue;
                }
                let new_id = result.states.len();
                ids.insert(key.clone(), new_id);
                result.states.push(MachineState {
                    id: new_id,
                    key,
                    entry: successor.payload,
                    exit: None,
                    exit_stack: Vec::new(),
                    executed_pcs: Vec::new(),
                });
                queue.push_back(new_id);
                queued.insert(new_id);
                new_id
            };
            edges.insert(MachineEdge {
                from: id,
                to,
                kind: successor.kind,
            });
        }
    }
    result.status = if result.frontiers.is_empty() {
        Status::Converged
    } else {
        Status::Incomplete
    };
    result.edges = edges.into_iter().collect();
    result.diagnostics = diagnostics.into_iter().collect();
    result.work = budget.used();
    Ok(result)
}
