//! Transactional source relation analysis. Discovery states are never exported;
//! a fresh source replay admits every loop before its words enter the byte kernel.
use super::{Identity, Word};
use crate::{
    Block, Edge, EdgeKind, EntityId, FactScope, Ir, Operation, Terminator, U256, ValueKind,
};
use std::collections::{BTreeMap, BTreeSet};
mod affine;
use affine::{Atom, Expr, Range};
mod execute;
mod induction;
mod join;
mod retain;
#[cfg(test)]
mod tests;

const MAX_TERMS: usize = 32;
const MAX_CELLS: usize = 128;
#[derive(Clone, Debug)]
struct Predicate {
    opcode: u8,
    inputs: Vec<Expr>,
}
#[derive(Clone, Debug, Default)]
struct State {
    values: BTreeMap<EntityId, Expr>,
    atoms: BTreeMap<Atom, Range>,
    bounds: BTreeMap<Expr, Range>,
    real: BTreeMap<Expr, Range>,
    predicates: BTreeMap<Atom, Predicate>,
    cells: BTreeMap<Expr, Expr>,
    memory: Option<EntityId>,
    effect: Option<EntityId>,
}
impl State {
    fn range(&self, expr: &Expr) -> Range {
        self.ordinary(expr).unwrap_or(Range::FULL)
    }
    fn ordinary(&self, expr: &Expr) -> Option<Range> {
        let numeric = expr.range(&self.atoms).or_else(|| {
            let start = Expr {
                terms: expr.terms.clone(),
                constant: U256::ZERO,
            };
            let end = Expr {
                terms: expr.terms.clone(),
                constant: U256::MAX,
            };
            self.real
                .range(start..=end)
                .take(MAX_TERMS)
                .find_map(|(known, range)| {
                    let delta = expr.constant.wrapping_sub(known.constant);
                    if !known.integral_add(&Expr::constant(delta)) {
                        return None;
                    }
                    if delta > (U256::MAX >> 1) {
                        let delta = U256::ZERO.wrapping_sub(delta);
                        Some(Range {
                            low: range.low.checked_sub(delta)?,
                            high: range.high.checked_sub(delta)?,
                        })
                    } else {
                        Some(Range {
                            low: range.low.checked_add(delta)?,
                            high: range.high.checked_add(delta)?,
                        })
                    }
                })
        });
        match (numeric, self.bounds.get(expr).copied()) {
            (Some(a), Some(b)) => a.intersect(b),
            (Some(a), None) => Some(a),
            _ => None,
        }
    }
}
#[derive(Clone, Debug)]
enum Flow {
    Dead,
    Unknown,
    Known(State),
}
struct Source<'a, P> {
    ir: &'a Ir,
    blocks: BTreeMap<EntityId, &'a Block>,
    values: BTreeMap<EntityId, &'a crate::Value>,
    incoming: BTreeMap<EntityId, Vec<&'a Edge>>,
    outgoing: BTreeMap<EntityId, Vec<&'a Edge>>,
    pay: P,
    refused: bool,
    expressions: BTreeMap<Vec<(Atom, U256)>, usize>,
    interned: Vec<Vec<(Atom, U256)>>,
    admitted: BTreeMap<EntityId, Word>,
    arities: BTreeMap<u8, (usize, usize)>,
    edge_truth: BTreeMap<EntityId, Option<bool>>,
    valid_cfg: bool,
    return_data_size_bound: Option<U256>,
    literals: BTreeSet<EntityId>,
    certificates: BTreeMap<EntityId, induction::Proof>,
}

pub(super) fn analyze(
    ir: &Ir,
    pay: &mut impl FnMut(usize) -> bool,
) -> Option<BTreeMap<EntityId, Word>> {
    if ir.fact_scope != FactScope::CompleteLocalCfg {
        return Some(BTreeMap::new());
    }
    let mut source = Source::new(ir, pay)?;
    if !source.valid_cfg {
        return Some(BTreeMap::new());
    }
    let components = super::cycles::components(ir, &mut source.pay)?;
    let mut flows = BTreeMap::new();
    for component in components {
        if !source.charge(
            component
                .len()
                .saturating_add(source.outgoing.get(&component[0]).map_or(0, Vec::len))
                .saturating_add(1),
        ) {
            return None;
        }
        if component.len() == 1
            && !source
                .outgoing
                .get(&component[0])
                .into_iter()
                .flatten()
                .any(|e| e.to == component[0])
        {
            let block = source.blocks[&component[0]];
            let flow = if Some(block.id) == ir.entry {
                Flow::Known(State {
                    memory: Some(block.memory_parameter),
                    effect: Some(block.effect_parameter),
                    ..State::default()
                })
            } else {
                source.join(block, &flows, None)
            };
            source.process(block, flow, None, false, &mut flows, true);
        } else {
            source.loop_region(&component, &mut flows);
        }
    }
    (!source.refused).then_some(source.admitted)
}
impl<'a, P: FnMut(usize) -> bool> Source<'a, P> {
    fn new(ir: &'a Ir, mut pay: P) -> Option<Self> {
        let return_data_size_bound = super::execution_bound(ir)?;
        if !pay(ir
            .values
            .len()
            .saturating_add(ir.edges.len().saturating_mul(4))
            .saturating_add(ir.blocks.len())
            .saturating_add(1))
        {
            return None;
        }
        if !pay(256 * 128) {
            return None;
        }
        let mut arities = BTreeMap::new();
        for opcode in 0..=u8::MAX {
            let program = evm_bytecode::Program::decode(&[opcode]).ok()?;
            let instruction = program.blocks().first()?.instructions.first()?;
            let io = match opcode {
                0x80..=0x8f => (1, 0),
                0x90..=0x9f => (2, 0),
                _ => instruction.stack_io(),
            };
            arities.insert(opcode, io);
        }
        let mut incoming = BTreeMap::<_, Vec<_>>::new();
        let mut outgoing = BTreeMap::<_, Vec<_>>::new();
        for edge in &ir.edges {
            incoming.entry(edge.to).or_default().push(edge);
            outgoing.entry(edge.from).or_default().push(edge);
        }
        let op_count = ir.blocks.iter().map(|b| b.operations.len()).sum::<usize>();
        if !pay(ir
            .values
            .len()
            .saturating_add(ir.edges.len())
            .saturating_add(ir.blocks.len())
            .saturating_add(op_count)
            .saturating_mul(3)
            .saturating_add(1))
        {
            return None;
        }
        let blocks: BTreeMap<_, _> = ir.blocks.iter().map(|b| (b.id, b)).collect();
        let edges: BTreeMap<_, _> = ir.edges.iter().map(|e| (e.id, e)).collect();
        let mut ids = BTreeSet::new();
        let mut valid_cfg = ir
            .blocks
            .iter()
            .map(|b| b.id)
            .chain(ir.values.iter().map(|v| v.id))
            .chain(ir.edges.iter().map(|e| e.id))
            .chain(
                ir.blocks
                    .iter()
                    .flat_map(|b| b.operations.iter().map(|o| o.id)),
            )
            .all(|id| ids.insert(id));
        valid_cfg &= ir.entry.is_none_or(|id| blocks.contains_key(&id));
        let mut edge_truth = BTreeMap::new();
        for block in &ir.blocks {
            valid_cfg &= !block.terminator.is_partial();
            let count = match &block.terminator {
                Terminator::Jump { edges, .. } => edges.len(),
                Terminator::Branch {
                    then_edges,
                    else_edge,
                    ..
                } => then_edges
                    .len()
                    .saturating_add(usize::from(else_edge.is_some())),
                Terminator::Fallthrough { .. } => 1,
                _ => 0,
            };
            if !pay(count.saturating_mul(4).saturating_add(1)) {
                return None;
            }
            let advertised: Vec<_> = match &block.terminator {
                Terminator::Branch {
                    then_edges,
                    else_edge,
                    ..
                } => then_edges
                    .iter()
                    .map(|&id| (id, Some(true)))
                    .chain(else_edge.iter().map(|&id| (id, Some(false))))
                    .collect(),
                _ => block
                    .terminator
                    .edges()
                    .into_iter()
                    .map(|id| (id, None))
                    .collect(),
            };
            for (id, truth) in advertised {
                valid_cfg &= edge_truth.insert(id, truth).is_none();
                valid_cfg &= edges.get(&id).is_some_and(|edge| {
                    edge.from == block.id
                        && blocks
                            .get(&edge.to)
                            .is_some_and(|target| edge.arguments.len() == target.parameters.len())
                        && truth.is_none_or(|truth| {
                            edge.kind
                                == if truth {
                                    EdgeKind::True
                                } else {
                                    EdgeKind::False
                                }
                        })
                });
            }
        }
        valid_cfg &= edge_truth.len() == ir.edges.len();
        let literals = ir
            .blocks
            .iter()
            .flat_map(|b| &b.operations)
            .filter(|op| (0x5f..=0x7f).contains(&op.opcode))
            .flat_map(|op| op.outputs.iter().copied())
            .collect();
        Some(Self {
            ir,
            blocks: ir.blocks.iter().map(|b| (b.id, b)).collect(),
            values: ir.values.iter().map(|v| (v.id, v)).collect(),
            incoming,
            outgoing,
            pay,
            refused: false,
            expressions: BTreeMap::new(),
            interned: Vec::new(),
            admitted: BTreeMap::new(),
            arities,
            edge_truth,
            valid_cfg,
            return_data_size_bound,
            literals,
            certificates: BTreeMap::new(),
        })
    }
    fn charge(&mut self, amount: usize) -> bool {
        if !(self.pay)(amount) {
            self.refused = true;
        }
        !self.refused
    }
    fn value(&self, state: &State, id: EntityId) -> Option<Expr> {
        if let Some(expr) = state.values.get(&id) {
            return Some(expr.clone());
        }
        // Only literal source words are axioms here. A constant produced by a
        // folded arithmetic operation must be recomputed from its actual opcode
        // and operands; a stale ValueKind::Constant must not hide source edits.
        if self.literals.contains(&id)
            && let Some(ValueKind::Constant(value)) = self.values.get(&id).map(|v| &v.kind)
        {
            return Some(Expr::constant(*value));
        }
        None
    }
    fn work(&mut self, state: &State, extra: usize) -> bool {
        self.charge(
            state
                .values
                .len()
                .saturating_add(state.atoms.len())
                .saturating_add(state.bounds.len())
                .saturating_add(state.real.len())
                .saturating_add(state.predicates.len())
                .saturating_add(state.cells.len().saturating_mul(2))
                .saturating_mul(MAX_TERMS * MAX_TERMS)
                .saturating_add(extra),
        )
    }
    fn export(&mut self, state: &State) {
        if !self.work(state, state.values.len().saturating_mul(MAX_TERMS)) {
            return;
        }
        for (&id, expr) in &state.values {
            let range = state.range(expr);
            if range == Range::FULL && expr.exact().is_none() {
                continue;
            }
            let base = if expr.terms.is_empty() {
                None
            } else {
                let key = expr.terms.iter().map(|(&a, &c)| (a, c)).collect::<Vec<_>>();
                let next = self.expressions.len();
                let index = if let Some(&index) = self.expressions.get(&key) {
                    index
                } else {
                    self.interned.push(key.clone());
                    self.expressions.insert(key, next);
                    next
                };
                Some(Identity::Affine(index))
            };
            self.admitted.insert(
                id,
                Word {
                    base,
                    offset: expr.constant,
                    low: range.low,
                    high: range.high,
                },
            );
        }
    }
    fn process(
        &mut self,
        block: &Block,
        flow: Flow,
        scope: Option<EntityId>,
        optimistic: bool,
        flows: &mut BTreeMap<EntityId, Flow>,
        publish: bool,
    ) {
        let mut flow = flow;
        if let Flow::Known(ref mut state) = flow
            && !self.execute(block, state, scope, optimistic)
        {
            flow = Flow::Unknown;
        }
        if publish && let Flow::Known(state) = &flow {
            self.export(state);
        }
        if !self.charge(
            self.outgoing
                .get(&block.id)
                .map_or(0, Vec::len)
                .saturating_add(1),
        ) {
            return;
        }
        let outgoing = self.outgoing.get(&block.id).cloned().unwrap_or_default();
        for edge in outgoing {
            let next = if let Flow::Known(state) = &flow {
                if !self.work(state, edge.arguments.len().saturating_mul(MAX_TERMS)) {
                    Flow::Unknown
                } else {
                    let mut next = state.clone();
                    if !self.edge_valid(block, edge, &next) {
                        Flow::Unknown
                    } else if !self.condition(block, edge, &mut next, optimistic) {
                        Flow::Dead
                    } else {
                        next.values = self
                            .blocks
                            .get(&edge.to)
                            .map(|target| {
                                target
                                    .parameters
                                    .iter()
                                    .zip(&edge.arguments)
                                    .filter_map(|(&id, &argument)| {
                                        self.value(&next, argument).map(|expr| (id, expr))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        next.memory = self.blocks.get(&edge.to).map(|b| b.memory_parameter);
                        next.effect = self.blocks.get(&edge.to).map(|b| b.effect_parameter);
                        if self.retain_contents(&mut next) {
                            Flow::Known(next)
                        } else {
                            Flow::Unknown
                        }
                    }
                }
            } else {
                flow.clone()
            };
            flows.insert(edge.id, next);
        }
    }
    fn edge_valid(&self, block: &Block, edge: &Edge, state: &State) -> bool {
        self.valid_cfg
            && edge.from == block.id
            && self.edge_truth.contains_key(&edge.id)
            && state.memory == Some(edge.memory)
            && state.effect == Some(edge.effect)
            && self
                .blocks
                .get(&edge.to)
                .is_some_and(|b| b.parameters.len() == edge.arguments.len())
            && !block.terminator.is_partial()
    }
    fn condition(
        &mut self,
        block: &Block,
        edge: &Edge,
        state: &mut State,
        optimistic: bool,
    ) -> bool {
        let Terminator::Branch {
            condition,
            unresolved: false,
            invalid_targets,
            ..
        } = &block.terminator
        else {
            return true;
        };
        if !invalid_targets.is_empty() {
            return true;
        }
        let Some(Some(truth)) = self.edge_truth.get(&edge.id).copied() else {
            return true;
        };
        let Some(expr) = self.value(state, *condition) else {
            return true;
        };
        self.assume(state, expr, truth, optimistic)
    }
}
