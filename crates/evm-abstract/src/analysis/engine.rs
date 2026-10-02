//! 增量工作表求固定点：变化才入队，join 只上升，状态键永远不变。

use super::{
    Analysis, Config, Diagnostic, Edge, Frontier, Limit, State, StateKey, Status, transfer,
};
use crate::{bytecode::Program, domain::Domain};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    num::NonZeroUsize,
};

pub(super) fn run(program: Program, config: Config) -> Analysis {
    let domain = Domain::new(NonZeroUsize::new(config.max_constants).expect("validated config"));
    let mut result = Analysis {
        program,
        config,
        states: Vec::new(),
        edges: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        status: Status::Converged,
        transfers: 0,
    };
    if result.program.blocks().is_empty() {
        return result;
    }

    let initial_key = StateKey {
        block: 0,
        stack_height: 0,
        context: Vec::new(),
    };
    result.states.push(State {
        id: 0,
        key: initial_key.clone(),
        entry_stack: Vec::new(),
        exit_stack: Vec::new(),
        executed_pcs: Vec::new(),
    });
    let mut ids = BTreeMap::from([(initial_key, 0)]);
    let mut queue = VecDeque::from([0]);
    let mut queued = BTreeSet::from([0]);
    let mut edges = BTreeSet::new();
    let mut diagnostics = BTreeSet::new();

    while let Some(id) = queue.pop_front() {
        queued.remove(&id);
        if result.transfers == result.config.max_transfers {
            // 保留刚弹出的状态和队列中所有待处理状态，不能悄悄清空后称“收敛”。
            for pending in std::iter::once(id).chain(queue) {
                result.frontiers.push(Frontier {
                    from: None,
                    target: result.states[pending].key.clone(),
                    limit: Limit::Transfers,
                });
            }
            break;
        }
        result.transfers += 1;
        let state = &result.states[id];
        let execution =
            transfer::execute(&result.program, state.key.block, &state.entry_stack, domain);
        let mut context = state.key.context.clone();
        if let Some(source_pc) = execution.jump_pc {
            context.push(source_pc);
            let discard = context.len().saturating_sub(result.config.context_depth);
            context.drain(..discard);
        }
        for (pc, kind) in execution.diagnostics {
            diagnostics.insert(Diagnostic {
                state: id,
                pc,
                kind,
            });
        }
        result.states[id].executed_pcs = execution.executed_pcs;
        result.states[id].exit_stack = execution.stack.clone();

        let mut successors = execution.successors.into_iter();
        while let Some(successor) = successors.next() {
            let key = StateKey {
                block: successor.block,
                stack_height: execution.stack.len(),
                context: context.clone(),
            };
            let to = if let Some(existing) = ids.get(&key).copied() {
                let entry = &mut result.states[existing].entry_stack;
                let mut changed = false;
                for (old, incoming) in entry.iter_mut().zip(&execution.stack) {
                    let joined = domain.join(old, incoming);
                    changed |= joined != *old;
                    *old = joined;
                }
                if changed && queued.insert(existing) {
                    queue.push_back(existing);
                }
                existing
            } else {
                if result.states.len() == result.config.max_states {
                    // 停止整个调度器，保留本条拒绝的边、还没传播的出边与现存工作表。
                    result.frontiers.push(Frontier {
                        from: Some(id),
                        target: key,
                        limit: Limit::States,
                    });
                    for pending in successors {
                        result.frontiers.push(Frontier {
                            from: Some(id),
                            target: StateKey {
                                block: pending.block,
                                stack_height: execution.stack.len(),
                                context: context.clone(),
                            },
                            limit: Limit::States,
                        });
                    }
                    for pending in queue.iter().copied() {
                        result.frontiers.push(Frontier {
                            from: None,
                            target: result.states[pending].key.clone(),
                            limit: Limit::States,
                        });
                    }
                    result.status = Status::Incomplete;
                    result.edges = edges.into_iter().collect();
                    result.diagnostics = diagnostics.into_iter().collect();
                    return result;
                }
                let new_id = result.states.len();
                ids.insert(key.clone(), new_id);
                result.states.push(State {
                    id: new_id,
                    key,
                    entry_stack: execution.stack.clone(),
                    exit_stack: Vec::new(),
                    executed_pcs: Vec::new(),
                });
                queued.insert(new_id);
                queue.push_back(new_id);
                new_id
            };
            edges.insert(Edge {
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
    result
}
