//! Exact cyclic CFG membership, without recursive host stack growth.
use crate::{EntityId, Ir};
use std::collections::{BTreeMap, BTreeSet};
pub(super) fn blocks(ir: &Ir, pay: &mut impl FnMut(usize) -> bool) -> Option<BTreeSet<EntityId>> {
    if !pay(ir
        .blocks
        .len()
        .saturating_mul(10)
        .saturating_add(ir.edges.len().saturating_mul(4))
        .saturating_add(1))
    {
        return None;
    }
    let indices: BTreeMap<_, _> = ir
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id, i))
        .collect();
    let n = ir.blocks.len();
    let mut next = vec![Vec::new(); n];
    let mut previous = vec![Vec::new(); n];
    for edge in &ir.edges {
        let (a, b) = (*indices.get(&edge.from)?, *indices.get(&edge.to)?);
        next[a].push(b);
        previous[b].push(a);
    }
    let mut seen = vec![false; n];
    let mut order = Vec::new();
    for root in 0..n {
        if seen[root] {
            continue;
        }
        seen[root] = true;
        let mut stack = vec![(root, 0)];
        while let Some((node, position)) = stack.last_mut() {
            if !pay(1) {
                return None;
            }
            if let Some(child) = next[*node].get(*position).copied() {
                *position += 1;
                if !seen[child] {
                    seen[child] = true;
                    stack.push((child, 0));
                }
            } else {
                order.push(*node);
                stack.pop();
            }
        }
    }
    seen.fill(false);
    let mut result = BTreeSet::new();
    for root in order.into_iter().rev() {
        if seen[root] {
            continue;
        }
        let mut component = Vec::new();
        let mut pending = vec![root];
        seen[root] = true;
        while let Some(node) = pending.pop() {
            if !pay(previous[node].len().saturating_add(1)) {
                return None;
            }
            component.push(node);
            for child in &previous[node] {
                if !seen[*child] {
                    seen[*child] = true;
                    pending.push(*child);
                }
            }
        }
        if component.len() > 1 || next[root].contains(&root) {
            result.extend(component.into_iter().map(|i| ir.blocks[i].id));
        }
    }
    Some(result)
}
