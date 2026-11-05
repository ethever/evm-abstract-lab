//! Iterative SCC discovery in dependency-first order; no recursive stack growth.
pub(super) struct Components {
    pub(super) members: Vec<Vec<usize>>,
}

pub(super) fn build(
    dependencies: &[Vec<usize>],
    pay: &mut impl FnMut(usize) -> bool,
) -> Option<Components> {
    let n = dependencies.len();
    if !pay(n.saturating_mul(8).saturating_add(1)) {
        return None;
    }
    let mut reverse = vec![Vec::new(); n];
    for (node, operands) in dependencies.iter().enumerate() {
        if !pay(operands.len()) {
            return None;
        }
        for operand in operands {
            reverse[*operand].push(node);
        }
    }
    let mut seen = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut pending = Vec::new();
    for node in 0..n {
        if seen[node] {
            continue;
        }
        seen[node] = true;
        pending.push((node, 0));
        while let Some((node, next)) = pending.last_mut() {
            if !pay(1) {
                return None;
            }
            if let Some(operand) = dependencies[*node].get(*next).copied() {
                *next += 1;
                if !seen[operand] {
                    seen[operand] = true;
                    pending.push((operand, 0));
                }
            } else {
                order.push(*node);
                pending.pop();
            }
        }
    }
    seen.fill(false);
    let mut members = Vec::new();
    let mut todo = Vec::new();
    for node in order.into_iter().rev() {
        if seen[node] {
            continue;
        }
        seen[node] = true;
        todo.push(node);
        let mut component = Vec::new();
        while let Some(node) = todo.pop() {
            if !pay(reverse[node].len().saturating_add(1)) {
                return None;
            }
            component.push(node);
            for dependent in &reverse[node] {
                if !seen[*dependent] {
                    seen[*dependent] = true;
                    todo.push(*dependent);
                }
            }
        }
        members.push(component);
    }
    // The first pass follows uses to definitions. Reversing condensation order
    // lets every outside dependency have its final canonical identity already.
    members.reverse();
    Some(Components { members })
}
