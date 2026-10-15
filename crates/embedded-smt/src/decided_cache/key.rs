//! Self-contained, full-equality ordered typed-DAG key. Free names are renamed
//! bijectively by (name, Sort); UF declarations by query-local identity class.
//! Non-symbol DAG sharing and all assertion/argument order are preserved. This
//! may miss equivalences but never truncates a key to make an apparent match.

use super::{
    CacheScope, Limits,
    budget::{MaintenanceBudget, hash_work, tree_work},
};
use crate::{
    Bool, Bv, Cancellation, Provider,
    term::{Op, Sort, Term},
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

mod pointers;
#[cfg(test)]
mod tests;

type Result<T> = std::result::Result<T, ()>;
fn sort_key(sort: Sort) -> (u8, u32, u32) {
    match sort {
        Sort::Bool => (0, 0, 0),
        Sort::Bv(width) => (1, width, 0),
        Sort::Array {
            index_width,
            element_width,
        } => (2, index_width, element_width),
    }
}
struct Encoder<'a, 'b, 'c> {
    limits: Limits,
    cancellation: Cancellation,
    out: Vec<u8>,
    nodes: HashMap<usize, u32>,
    variables: BTreeMap<((u8, u32, u32), &'a str), u32>,
    functions: HashMap<usize, u32>,
    edges: usize,
    steps: usize,
    budget: &'b mut MaintenanceBudget<'c>,
}
impl<'a> Encoder<'a, '_, '_> {
    fn checkpoint(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            Err(())
        } else {
            Ok(())
        }
    }
    fn bytes(&mut self, value: &[u8]) -> Result<()> {
        if value.len() > self.limits.key_bytes.saturating_sub(self.out.len()) {
            return Err(());
        }
        if !self.budget.charge(value.len().saturating_add(1)) {
            return Err(());
        }
        self.out.extend_from_slice(value);
        Ok(())
    }
    fn byte(&mut self, value: u8) -> Result<()> {
        self.bytes(&[value])
    }
    fn number(&mut self, value: u32) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }
    fn wide(&mut self, value: usize) -> Result<()> {
        self.bytes(&(value as u64).to_le_bytes())
    }
    fn text(&mut self, value: &str) -> Result<()> {
        self.number(u32::try_from(value.len()).map_err(|_| ())?)?;
        self.bytes(value.as_bytes())
    }
    fn sort(&mut self, sort: Sort) -> Result<()> {
        let (kind, first, second) = sort_key(sort);
        self.byte(kind)?;
        match kind {
            0 => Ok(()),
            1 => self.number(first),
            _ => {
                self.number(first)?;
                self.number(second)
            }
        }
    }
    fn node(&mut self, term: &'a Term) -> Result<u32> {
        if !self.budget.charge(1) {
            return Err(());
        }
        let mut pending = vec![(term, false)];
        while let Some((current, ready)) = pending.pop() {
            self.checkpoint()?;
            if !self.budget.charge(1) {
                return Err(());
            }
            self.steps = self.steps.checked_add(1).ok_or(())?;
            if self.steps
                > self
                    .limits
                    .nodes
                    .saturating_mul(2)
                    .saturating_add(self.limits.edges)
            {
                return Err(());
            }
            if pointers::get(&self.nodes, current.id(), self.budget)?.is_some() {
                continue;
            }
            if !ready {
                self.edges = self.edges.checked_add(current.0.args.len()).ok_or(())?;
                if self.edges > self.limits.edges || pending.len() > self.limits.edges {
                    return Err(());
                }
                if !self.budget.charge(current.0.args.len().saturating_add(1)) {
                    return Err(());
                }
                pending.push((current, true));
                for child in current.0.args.iter().rev() {
                    pending.push((child, false))
                }
                continue;
            }
            if self.nodes.len() >= self.limits.nodes {
                return Err(());
            }
            let id = u32::try_from(self.nodes.len()).map_err(|_| ())?;
            self.byte(0x4e)?; // A full node definition, never a pointer.
            self.number(id)?;
            self.sort(current.0.sort)?;
            self.op(current)?;
            self.number(u32::try_from(current.0.args.len()).map_err(|_| ())?)?;
            for child in &current.0.args {
                let child_id = pointers::get(&self.nodes, child.id(), self.budget)?.ok_or(())?;
                self.number(child_id)?;
            }
            pointers::insert(
                &mut self.nodes,
                current.id(),
                id,
                self.limits.nodes,
                &self.cancellation,
                self.budget,
            )?;
        }
        pointers::get(&self.nodes, term.id(), self.budget)?.ok_or(())
    }
    fn op(&mut self, term: &'a Term) -> Result<()> {
        let tag = match &term.0.op {
            Op::Bool(_) => 0,
            Op::Literal(_) => 1,
            Op::Variable(_) => 2,
            Op::ConstantArray => 3,
            Op::Select => 4,
            Op::Store => 5,
            Op::Apply(_) => 6,
            Op::Eq => 7,
            Op::Not => 8,
            Op::And => 9,
            Op::Or => 10,
            Op::Ite => 11,
            Op::Add => 12,
            Op::Sub => 13,
            Op::Mul => 14,
            Op::Udiv => 15,
            Op::Sdiv => 16,
            Op::Urem => 17,
            Op::Srem => 18,
            Op::Ult => 19,
            Op::Ule => 20,
            Op::Ugt => 21,
            Op::Uge => 22,
            Op::Slt => 23,
            Op::Sle => 24,
            Op::Sgt => 25,
            Op::Sge => 26,
            Op::BvAnd => 27,
            Op::BvOr => 28,
            Op::BvXor => 29,
            Op::BvNot => 30,
            Op::Shl => 31,
            Op::Lshr => 32,
            Op::Ashr => 33,
            Op::ZeroExtend(_) => 34,
            Op::Extract(_, _) => 35,
        };
        self.byte(tag)?;
        match &term.0.op {
            Op::Bool(value) => self.byte(u8::from(*value))?,
            Op::Literal(value) => self.text(value)?,
            Op::Variable(name) => {
                if !self
                    .budget
                    .charge(tree_work(self.variables.len(), name.len()))
                {
                    return Err(());
                }
                if name.contains('\0') {
                    return Err(());
                }
                let next = u32::try_from(self.variables.len()).map_err(|_| ())?;
                let id = *self
                    .variables
                    .entry((sort_key(term.0.sort), name))
                    .or_insert(next);
                self.number(id)?;
            }
            Op::Apply(declaration) => {
                if !self.budget.charge(declaration.name.len()) {
                    return Err(());
                }
                if declaration.name.contains('\0') {
                    return Err(());
                }
                let next = u32::try_from(self.functions.len()).map_err(|_| ())?;
                // Pointer identities exist ONLY in this temporary lookup. They
                // are replaced by canonical IDs, never serialized or retained.
                let pointer = Arc::as_ptr(declaration) as usize;
                let id = match pointers::get(&self.functions, pointer, self.budget)? {
                    Some(id) => id,
                    None => {
                        pointers::insert(
                            &mut self.functions,
                            pointer,
                            next,
                            self.limits.nodes,
                            &self.cancellation,
                            self.budget,
                        )?;
                        next
                    }
                };
                self.number(id)?;
                // apply() validates every argument against the declaration, so
                // this is its complete domain and result signature, including
                // arrays and zero-argument functions.
                self.number(u32::try_from(term.0.args.len()).map_err(|_| ())?)?;
                for arg in &term.0.args {
                    self.sort(arg.0.sort)?
                }
                self.sort(term.0.sort)?;
            }
            Op::ZeroExtend(extra) => self.number(*extra)?,
            Op::Extract(high, low) => {
                self.number(*high)?;
                self.number(*low)?;
            }
            _ => {}
        }
        Ok(())
    }
}
pub(super) fn query(
    base: &[Bool],
    model: Option<&Bv>,
    provider: Provider,
    rlimit: u32,
    scope: CacheScope,
    limits: Limits,
    budget: &mut MaintenanceBudget<'_>,
) -> Result<Vec<u8>> {
    if base.len() > limits.nodes {
        return Err(());
    }
    let cancellation = crate::current_cancellation();
    // HashMap::new initializes RandomState even before any table allocation.
    if cancellation.is_cancelled() || !budget.charge(hash_work(2)) {
        return Err(());
    }
    let encoder = Encoder {
        limits,
        cancellation,
        out: Vec::new(),
        nodes: HashMap::new(),
        variables: BTreeMap::new(),
        functions: HashMap::new(),
        edges: 0,
        steps: 0,
        budget,
    };
    encode(encoder, base, model, provider, rlimit, scope)
}

fn encode<'a>(
    mut encoder: Encoder<'a, '_, '_>,
    base: &'a [Bool],
    model: Option<&'a Bv>,
    provider: Provider,
    rlimit: u32,
    scope: CacheScope,
) -> Result<Vec<u8>> {
    encoder.checkpoint()?;
    encoder.byte(1)?; // Typed key format version.
    encoder.byte(match provider {
        Provider::Z3 => 0,
        Provider::Bitwuzla => 1,
        Provider::Cvc5 => 2,
    })?;
    encoder.number(rlimit)?;
    encoder.byte(u8::from(scope.enabled))?;
    encoder.wide(scope.constraints)?;
    encoder.wide(scope.nodes)?;
    encoder.wide(scope.depth)?;
    encoder.byte(u8::from(model.is_some()))?; // Unique proof vs candidate SAT.
    encoder.number(u32::try_from(base.len()).map_err(|_| ())?)?;
    for assertion in base {
        encoder.byte(0x41)?;
        let id = encoder.node(&assertion.0)?;
        encoder.byte(0x52)?;
        encoder.number(id)?;
    }
    encoder.byte(0x4d)?;
    encoder.byte(u8::from(model.is_some()))?;
    if let Some(model) = model {
        let id = encoder.node(&model.0)?;
        encoder.byte(0x52)?;
        encoder.number(id)?;
    }
    encoder.checkpoint()?;
    Ok(encoder.out)
}
