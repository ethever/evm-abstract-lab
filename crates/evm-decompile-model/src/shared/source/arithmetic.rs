//! Arithmetic templates retain an expression arena anchor. Leaf expressions
//! are typed call operands, including literals; no result or state is cached.
use super::{Error, SharedRegion, SharedSourceIndex, charge};
use crate::{
    EntityId, LiftedExpressionKind as X, MAX_SHARED_REGION_ENTITIES, SharedBinding,
    SharedInterface, SharedInvocation, SharedProcedureKind,
};
use std::collections::BTreeSet;

pub(super) fn arithmetic_opcode(opcode: u8) -> bool {
    evm_bytecode::word::arity(opcode).is_some()
}

pub(super) fn arithmetic_primitive(opcode: u8) -> bool {
    arithmetic_opcode(opcode) || matches!(opcode, 0x50 | 0x56 | 0x57 | 0x5b | 0x5f..=0x9f)
}

pub(super) fn leaf(kind: &X) -> bool {
    matches!(kind, X::Word(_) | X::Predicate(_) | X::Value(_))
}

pub(super) fn operand_count(kind: &X) -> usize {
    match kind {
        X::WordFromPredicate(_) | X::NonZero(_) | X::Not(_) => 1,
        X::Compare { .. } => 2,
        X::WordOp { arguments, .. } => arguments.len(),
        _ => 0,
    }
}

impl SharedSourceIndex<'_> {
    pub(super) fn compare_arithmetic(
        &self,
        a: &SharedRegion,
        b: &SharedRegion,
        bind: &mut impl FnMut(EntityId, EntityId) -> Result<(), Error>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(), Error> {
        charge(
            pay,
            a.entities
                .len()
                .saturating_add(b.entities.len())
                .saturating_mul(4),
        )?;
        let mut pending = vec![(a.root, b.root)];
        let mut seen = BTreeSet::new();
        while let Some((aid, bid)) = pending.pop() {
            charge(pay, 6)?;
            bind(aid, bid)?;
            if !seen.insert((aid, bid)) {
                continue;
            }
            let ae = self.expressions.get(&aid).ok_or(Error::Missing(aid))?;
            let be = self.expressions.get(&bid).ok_or(Error::Missing(bid))?;
            if ae.ty != be.ty {
                return Err(Error::Correspondence(bid));
            }
            // Leaves are the typed call parameters. Their literal contents or
            // source SSA roots are evaluated by each caller, not specialized in
            // the definition. Internal computations still compare exactly.
            if leaf(&ae.kind) && leaf(&be.kind) {
                continue;
            }
            match (&ae.kind, &be.kind) {
                (X::WordFromPredicate(a), X::WordFromPredicate(b))
                | (X::NonZero(a), X::NonZero(b))
                | (X::Not(a), X::Not(b)) => pending.push((*a, *b)),
                (
                    X::Compare {
                        relation: ar,
                        left: al,
                        right: ax,
                    },
                    X::Compare {
                        relation: br,
                        left: bl,
                        right: bx,
                    },
                ) if ar == br => pending.extend([(*al, *bl), (*ax, *bx)]),
                (
                    X::WordOp {
                        opcode: ao,
                        arguments: aa,
                    },
                    X::WordOp {
                        opcode: bo,
                        arguments: ba,
                    },
                ) if ao == bo && aa.len() == ba.len() => {
                    charge(pay, aa.len())?;
                    pending.extend(aa.iter().copied().zip(ba.iter().copied()));
                }
                _ => return Err(Error::Correspondence(bid)),
            }
        }
        Ok(())
    }

    /// A bounded, typed arithmetic DAG with explicit leaf parameters. Literal
    /// values remain at their original call sites, so templates may share code
    /// across different scales and offsets without specializing the body.
    pub fn arithmetic_region(
        &self,
        root: EntityId,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<SharedRegion, Error> {
        charge(pay, 8)?;
        if self.nodes.contains_key(&root) {
            return Err(Error::Region(root));
        }
        let mut pending = vec![root];
        let mut entities = BTreeSet::new();
        let mut inputs = BTreeSet::new();
        let mut blocks = BTreeSet::new();
        let mut operations = 0_usize;
        while let Some(id) = pending.pop() {
            charge(pay, 6)?;
            if entities.contains(&id) {
                continue;
            }
            if entities.len() >= MAX_SHARED_REGION_ENTITIES {
                return Err(Error::Region(root));
            }
            let expression = self.expressions.get(&id).ok_or(Error::Missing(id))?;
            let operands = operand_count(&expression.kind);
            charge(pay, operands.saturating_add(4))?;
            if pending.len().saturating_add(operands) > MAX_SHARED_REGION_ENTITIES {
                return Err(Error::Region(root));
            }
            if let X::WordOp { opcode, .. } = &expression.kind
                && !arithmetic_opcode(*opcode)
            {
                return Err(Error::Region(root));
            }
            entities.insert(id);
            blocks.insert(expression.block);
            if leaf(&expression.kind) {
                inputs.insert(id);
            } else {
                operations = operations.saturating_add(1);
                pending.extend(expression.kind.operands());
            }
        }
        // A leaf is already a caller operand. Outlining it would substitute the
        // invocation into its own argument, with no executable body boundary.
        if operations == 0 {
            return Err(Error::Region(root));
        }
        Ok(SharedRegion {
            kind: SharedProcedureKind::PureArithmetic,
            root,
            nodes: Vec::new(),
            blocks: blocks.into_iter().collect(),
            entities: entities.into_iter().collect(),
            interface: SharedInterface {
                inputs: inputs.into_iter().collect(),
                ..SharedInterface::default()
            },
            operations,
            retained_operations: operations,
            terminal_closed: true,
        })
    }

    /// The expression scope and arena position are source facts, never supplied
    /// by the candidate. Expression replacement has no lexical control exit.
    pub fn arithmetic_invocation(
        &self,
        id: EntityId,
        procedure: EntityId,
        source: EntityId,
        bindings: Vec<SharedBinding>,
    ) -> Result<SharedInvocation, Error> {
        let expression = self
            .expressions
            .get(&source)
            .ok_or(Error::Missing(source))?;
        Ok(SharedInvocation {
            id,
            procedure,
            source,
            owner: Some(expression.block),
            position: *self
                .expression_positions
                .get(&source)
                .ok_or(Error::Missing(source))?,
            continuation: None,
            bindings,
        })
    }
}
