//! Flat wire encoding of the owned AST. Child identity is checked before tree
//! reconstruction; JSON nesting never grows with the semantic region depth.
use crate::{EntityId, IndirectArm, StructuredKind, StructuredNode, StructuredProgram};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};

/// Supported region-construction recursion ceiling, validated before recovery.
pub const MAX_STRUCTURING_DEPTH: usize = 512;
/// Maximum admitted owned-tree depth, independent of JSON parser nesting.
pub const MAX_STRUCTURED_WIRE_DEPTH: usize = 2048;
/// Maximum flat AST node records admitted by the wire codec.
pub const MAX_STRUCTURED_WIRE_NODES: usize = 1_000_000;

strict_record! {
pub struct WireProgram {
    pub root: EntityId,
    pub nodes: Vec<WireNode>,
}
}
strict_record! {
pub struct WireNode {
    pub id: EntityId,
    pub kind: StructuredKind<EntityId>,
}
}

impl Serialize for StructuredProgram {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut nodes = Vec::new();
        let mut seen = BTreeSet::new();
        let mut todo = vec![(&self.root, 1_usize)];
        while let Some((node, depth)) = todo.pop() {
            if depth > MAX_STRUCTURED_WIRE_DEPTH || nodes.len() >= MAX_STRUCTURED_WIRE_NODES {
                return Err(serde::ser::Error::custom(
                    "structured tree exceeds wire admission bounds",
                ));
            }
            if node.id.0 == 0 || !seen.insert(node.id) {
                return Err(serde::ser::Error::custom(
                    "duplicate or zero structured node identity",
                ));
            }
            let kind = map_kind(&node.kind, |child| {
                todo.push((child, depth + 1));
                child.id
            });
            nodes.push(WireNode { id: node.id, kind });
        }
        nodes.sort_by_key(|n| n.id);
        WireProgram {
            root: self.root.id,
            nodes,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StructuredProgram {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireProgram::deserialize(deserializer)?;
        decode(wire).map_err(serde::de::Error::custom)
    }
}

fn decode(wire: WireProgram) -> Result<StructuredProgram, &'static str> {
    if wire.nodes.len() > MAX_STRUCTURED_WIRE_NODES {
        return Err("structured node count exceeds wire admission bound");
    }
    let mut table = BTreeMap::new();
    for node in wire.nodes {
        if node.id.0 == 0 || table.insert(node.id, node).is_some() {
            return Err("duplicate or zero structured node identity");
        }
    }
    if !table.contains_key(&wire.root) {
        return Err("structured root is missing");
    }
    let mut parents = BTreeMap::<EntityId, usize>::new();
    for node in table.values() {
        for child in children(&node.kind) {
            if !table.contains_key(child) {
                return Err("structured child reference is missing");
            }
            let count = parents.entry(*child).or_default();
            *count += 1;
            if *count > 1 {
                return Err("structured child is shared by multiple parents");
            }
        }
    }
    if parents.contains_key(&wire.root) {
        return Err("structured root participates in a cycle or has a parent");
    }
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    let mut todo = vec![(wire.root, 1_usize)];
    while let Some((id, depth)) = todo.pop() {
        if depth > MAX_STRUCTURED_WIRE_DEPTH {
            return Err("structured tree depth exceeds wire admission bound");
        }
        if !seen.insert(id) {
            return Err("structured child cycle or shared reference");
        }
        order.push(id);
        todo.extend(
            children(&table[&id].kind)
                .into_iter()
                .map(|child| (*child, depth + 1)),
        );
    }
    if order.len() != table.len() {
        return Err("unreachable or cyclic structured records");
    }
    let mut built = BTreeMap::new();
    for id in order.into_iter().rev() {
        let node = &table[&id];
        // Every child has exactly one parent and precedes it in this reverse
        // traversal. Remove owned children rather than recursively cloning them.
        let kind = map_kind(&node.kind, |child| {
            built.remove(child).expect("validated child order")
        });
        built.insert(id, StructuredNode { id, kind });
    }
    Ok(StructuredProgram {
        root: built.remove(&wire.root).expect("validated root"),
    })
}

fn children<N>(kind: &StructuredKind<N>) -> Vec<&N> {
    match kind {
        StructuredKind::Sequence(nodes) => nodes.iter().collect(),
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => vec![then_branch, else_branch],
        StructuredKind::While { header, body, .. } => vec![header, body],
        StructuredKind::Indirect { arms, .. } => arms.iter().map(|a| &a.body).collect(),
        _ => Vec::new(),
    }
}

fn map_kind<'a, N, M>(
    kind: &'a StructuredKind<N>,
    mut child: impl FnMut(&'a N) -> M,
) -> StructuredKind<M> {
    match kind {
        StructuredKind::Sequence(nodes) => {
            StructuredKind::Sequence(nodes.iter().map(child).collect())
        }
        StructuredKind::Block { block } => StructuredKind::Block { block: *block },
        StructuredKind::Transfer { edge } => StructuredKind::Transfer { edge: *edge },
        StructuredKind::If {
            condition,
            then_branch,
            else_branch,
        } => StructuredKind::If {
            condition: *condition,
            then_branch: Box::new(child(then_branch)),
            else_branch: Box::new(child(else_branch)),
        },
        StructuredKind::While {
            header,
            condition,
            condition_nonzero,
            body,
            exit_edge,
        } => StructuredKind::While {
            header: Box::new(child(header)),
            condition: *condition,
            condition_nonzero: *condition_nonzero,
            body: Box::new(child(body)),
            exit_edge: *exit_edge,
        },
        StructuredKind::Break => StructuredKind::Break,
        StructuredKind::Continue => StructuredKind::Continue,
        StructuredKind::Exit { block } => StructuredKind::Exit { block: *block },
        StructuredKind::PartialExit { block } => StructuredKind::PartialExit { block: *block },
        StructuredKind::Indirect {
            block,
            arms,
            unresolved,
        } => StructuredKind::Indirect {
            block: *block,
            unresolved: *unresolved,
            arms: arms
                .iter()
                .map(|a| IndirectArm {
                    target_pc: a.target_pc,
                    body: child(&a.body),
                })
                .collect(),
        },
        StructuredKind::Dispatcher { entry, cases } => StructuredKind::Dispatcher {
            entry: *entry,
            cases: cases.clone(),
        },
    }
}
