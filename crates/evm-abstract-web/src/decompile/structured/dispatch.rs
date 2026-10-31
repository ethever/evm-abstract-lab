//! Render actual dispatcher decisions and scoped edge transfers. Labels are
//! qualified by their AST owner, not frontend-created semantic entities.
use super::{Line, Scope, StageIndex, body, emit, terminator, transfer, value};
use evm_abstract_protocol::decompile_model::{
    DispatchCase, DispatchTransition, EntityId, Ir, Terminator,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(super) struct Context<'a> {
    pub ir: &'a Ir,
    pub index: &'a StageIndex,
    pub scope: Scope,
}

pub(super) fn show(
    node: EntityId,
    entry: EntityId,
    cases: &[DispatchCase],
    context: Context<'_>,
    indent: usize,
    output: &mut Vec<Line>,
) {
    let dispatch = Dispatch {
        node,
        members: cases.iter().map(|case| case.block).collect(),
        context,
    };
    emit(
        output,
        node,
        indent,
        format!("dispatch_state_{} = b{};", node.0, entry.0),
    );
    emit(
        output,
        node,
        indent,
        format!("'dispatch{}: while (true) {{", node.0),
    );
    emit(
        output,
        node,
        indent + 1,
        format!("switch (dispatch_state_{}) {{", node.0),
    );
    for case in cases {
        emit(
            output,
            case.block,
            indent + 2,
            format!("case b{}: {{", case.block.0),
        );
        body(
            node,
            case.block,
            context.ir,
            context.index,
            indent + 3,
            output,
        );
        let transitions = case
            .transitions
            .iter()
            .map(|transition| (transition.edge, transition))
            .collect();
        if let Some(block) = context.index.block(context.ir, case.block) {
            dispatch.control(
                case.block,
                &block.terminator,
                &transitions,
                indent + 3,
                output,
            );
        }
        emit(output, case.block, indent + 2, "}");
    }
    emit(output, node, indent + 1, "}");
    emit(output, node, indent, "}");
}

struct Dispatch<'a> {
    node: EntityId,
    members: BTreeSet<EntityId>,
    context: Context<'a>,
}

impl Dispatch<'_> {
    fn edge(
        &self,
        edge: EntityId,
        transitions: &BTreeMap<EntityId, &DispatchTransition>,
        indent: usize,
        output: &mut Vec<Line>,
    ) {
        let Some(transition) = transitions.get(&edge) else {
            emit(
                output,
                self.node,
                indent,
                format!("unavailable_transition(e{});", edge.0),
            );
            return;
        };
        emit(output, transition.id, indent, format!("// on e{}", edge.0));
        transfer(
            transition.id,
            edge,
            self.context.ir,
            self.context.index,
            indent,
            output,
        );
        let target = transition.target;
        let action = if self.members.contains(&target) {
            format!("continue 'dispatch{};", self.node.0)
        } else if Some(target) == self.context.scope.following {
            format!("break 'dispatch{}; // resume b{}", self.node.0, target.0)
        } else if let Some(outer) = self.context.scope.outer_loop {
            format!(
                "{} 'loop{}; // resume b{}",
                if Some(target) == outer.entry {
                    "continue"
                } else {
                    "break"
                },
                outer.node.0,
                target.0
            )
        } else {
            format!(
                "break 'dispatch{}; // region exit to b{}",
                self.node.0, target.0
            )
        };
        emit(
            output,
            transition.id,
            indent,
            format!("dispatch_state_{} = b{}; {action}", self.node.0, target.0),
        );
    }

    fn jump(
        &self,
        target: EntityId,
        edges: &[EntityId],
        unresolved: bool,
        transitions: &BTreeMap<EntityId, &DispatchTransition>,
        indent: usize,
        output: &mut Vec<Line>,
    ) {
        emit(
            output,
            self.node,
            indent,
            format!("match {} {{", value(target)),
        );
        for edge in edges {
            let pc = self
                .context
                .index
                .edge(self.context.ir, *edge)
                .and_then(|edge| self.context.index.block(self.context.ir, edge.to))
                .and_then(|block| block.source);
            let label = pc.map_or_else(
                || format!("target_of(e{})", edge.0),
                |span| format!("0x{:x}", span.pc),
            );
            let owner = transitions
                .get(edge)
                .map_or(self.node, |transition| transition.id);
            emit(output, owner, indent + 1, format!("{label}: {{"));
            self.edge(*edge, transitions, indent + 2, output);
            emit(output, owner, indent + 1, "}");
        }
        emit(
            output,
            self.node,
            indent + 1,
            if unresolved {
                "otherwise: partial_exit(\"unresolved jump target\");"
            } else {
                "otherwise: exception(\"invalid jump destination\");"
            },
        );
        emit(output, self.node, indent, "}");
    }

    fn control(
        &self,
        block: EntityId,
        term: &Terminator,
        transitions: &BTreeMap<EntityId, &DispatchTransition>,
        indent: usize,
        output: &mut Vec<Line>,
    ) {
        match term {
            Terminator::Fallthrough { edge } => self.edge(*edge, transitions, indent, output),
            Terminator::Jump {
                target,
                edges,
                unresolved,
                ..
            } => self.jump(*target, edges, *unresolved, transitions, indent, output),
            Terminator::Branch {
                condition,
                target,
                then_edges,
                else_edge,
                unresolved,
                ..
            } => {
                emit(
                    output,
                    block,
                    indent,
                    format!("if ({} != 0) {{", value(*condition)),
                );
                self.jump(
                    *target,
                    then_edges,
                    *unresolved,
                    transitions,
                    indent + 1,
                    output,
                );
                emit(output, block, indent, "} else {");
                if let Some(edge) = else_edge {
                    self.edge(*edge, transitions, indent + 1, output);
                } else {
                    emit(
                        output,
                        block,
                        indent + 1,
                        "evm.stop(); // JUMPI false continuation: end of code",
                    );
                }
                emit(output, block, indent, "}");
            }
            _ => emit(output, block, indent, terminator(term)),
        }
    }
}
