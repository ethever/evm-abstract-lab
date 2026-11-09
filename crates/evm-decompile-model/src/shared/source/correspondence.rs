//! Direct finite semantic correspondence; hashes and discovery are absent.
use super::{Error, SharedRegion, SharedSourceIndex, charge};
use crate::EntityId;
use crate::{SharedBinding, SharedInterface, StructuredKind as K, Terminator as T};
use std::collections::{BTreeMap, BTreeSet};
impl SharedSourceIndex<'_> {
    /// Discovery supplies role pairs; only verify_correspondence admits a map.
    pub fn correspondence(
        &self,
        a: &SharedRegion,
        b: &SharedRegion,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<Vec<SharedBinding>, Error> {
        let mut map = BTreeMap::new();
        self.compare(
            a,
            b,
            &mut |a, b| match map.insert(a, b) {
                Some(old) if old != b => Err(Error::Correspondence(a)),
                _ => Ok(()),
            },
            pay,
        )?;
        Ok(map
            .into_iter()
            .filter(|(a, b)| a != b)
            .map(|(formal, actual)| SharedBinding { formal, actual })
            .collect())
    }
    fn compare(
        &self,
        a: &SharedRegion,
        b: &SharedRegion,
        bind: &mut impl FnMut(EntityId, EntityId) -> Result<(), Error>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(), Error> {
        if a.kind != b.kind || a.nodes.is_empty() != b.nodes.is_empty() {
            return Err(Error::Correspondence(b.root));
        }
        if a.nodes.is_empty() {
            return self.compare_arithmetic(a, b, bind, pay);
        }
        charge(
            pay,
            a.entities
                .len()
                .saturating_add(b.entities.len())
                .saturating_mul(4),
        )?;
        if a.nodes.len() != b.nodes.len() {
            return Err(Error::Correspondence(b.root));
        }
        let mut expressions = Vec::new();
        let mut edges = Vec::new();
        fn ids(
            a: &[EntityId],
            b: &[EntityId],
            bind: &mut impl FnMut(EntityId, EntityId) -> Result<(), Error>,
        ) -> Result<(), Error> {
            if a.len() != b.len() {
                return Err(Error::Correspondence(EntityId(0)));
            }
            for (a, b) in a.iter().zip(b) {
                bind(*a, *b)?;
            }
            Ok(())
        }
        fn optional(
            a: Option<EntityId>,
            b: Option<EntityId>,
            bind: &mut impl FnMut(EntityId, EntityId) -> Result<(), Error>,
        ) -> Result<(), Error> {
            match (a, b) {
                (Some(a), Some(b)) => bind(a, b),
                (None, None) => Ok(()),
                _ => Err(Error::Correspondence(EntityId(0))),
            }
        }
        for (an, bn) in a.nodes.iter().zip(&b.nodes) {
            bind(*an, *bn)?;
            let ak = &self.nodes[an].kind;
            let bk = &self.nodes[bn].kind;
            match (ak, bk) {
                (K::Sequence(a), K::Sequence(b)) if a.len() == b.len() => {}
                (K::Block { block: ab }, K::Block { block: bb }) => {
                    bind(*ab, *bb)?;
                    let ar = self.blocks[ab];
                    let br = self.blocks[bb];
                    let ah = self.high_blocks[ab];
                    let bh = self.high_blocks[bb];
                    bind(ah.id, bh.id)?;
                    ids(&ah.parameters, &bh.parameters, bind)?;
                    bind(ar.memory_parameter, br.memory_parameter)?;
                    bind(ar.effect_parameter, br.effect_parameter)?;
                    if ah.statements.len() != bh.statements.len() {
                        return Err(Error::Correspondence(*bb));
                    }
                    for (as_, bs) in ah.statements.iter().zip(&bh.statements) {
                        charge(
                            pay,
                            as_.inputs
                                .len()
                                .saturating_add(as_.outputs.len())
                                .saturating_add(16),
                        )?;
                        let ao = self.operations[&as_.operation];
                        let bo = self.operations[&bs.operation];
                        if ao.opcode != bo.opcode
                            || ao.replacement != bo.replacement
                            || ao.name != bo.name
                            || ao.effects != bo.effects
                            || (ao.opcode == 0x58 && ao.source != bo.source)
                            || ao.outputs.len() != bo.outputs.len()
                        {
                            return Err(Error::Correspondence(bo.id));
                        }
                        // PUSH's immediate lives in its raw output definition,
                        // not in an input expression or replacement field.
                        // Retained PUSH statements therefore need this literal
                        // check even though ordinary projection inlines them.
                        if matches!(ao.opcode, 0x5f..=0x7f) {
                            let ([a], [b]) = (ao.outputs.as_slice(), bo.outputs.as_slice()) else {
                                return Err(Error::Correspondence(bo.id));
                            };
                            match (
                                self.values.get(a).map(|v| &v.kind),
                                self.values.get(b).map(|v| &v.kind),
                            ) {
                                (
                                    Some(crate::ValueKind::Constant(a)),
                                    Some(crate::ValueKind::Constant(b)),
                                ) if a == b && (ao.opcode != 0x5f || *a == crate::U256::ZERO) => {}
                                _ => return Err(Error::Correspondence(bo.id)),
                            }
                        }
                        bind(as_.id, bs.id)?;
                        bind(ao.id, bo.id)?;
                        ids(&as_.outputs, &bs.outputs, bind)?;
                        if as_.inputs.len() != bs.inputs.len() {
                            return Err(Error::Correspondence(bo.id));
                        }
                        expressions
                            .extend(as_.inputs.iter().copied().zip(bs.inputs.iter().copied()));
                        for (a, b) in as_.outputs.iter().zip(&bs.outputs) {
                            if ao.outputs.iter().position(|v| v == a)
                                != bo.outputs.iter().position(|v| v == b)
                            {
                                return Err(Error::Correspondence(bo.id));
                            }
                        }
                        optional(ao.memory_input, bo.memory_input, bind)?;
                        optional(ao.memory_output, bo.memory_output, bind)?;
                        optional(ao.effect_input, bo.effect_input, bind)?;
                        optional(ao.effect_output, bo.effect_output, bind)?;
                    }
                    match (self.constructions.get(ab), self.constructions.get(bb)) {
                        (None, None) => {}
                        (Some(a), Some(b)) if (a.start, a.end) == (b.start, b.end) => {
                            let ar = self.reverts[&a.revert];
                            let br = self.reverts[&b.revert];
                            bind(a.id, b.id)?;
                            bind(ar.id, br.id)?;
                            self.payload(ar.payload, br.payload, bind)?;
                            ids(
                                &[ar.operation, ar.memory, ar.offset, ar.size],
                                &[br.operation, br.memory, br.offset, br.size],
                                bind,
                            )?;
                            ids(&ar.writes, &br.writes, bind)?;
                        }
                        _ => return Err(Error::Correspondence(*bb)),
                    }
                }
                (K::Exit { block: a }, K::Exit { block: b }) => {
                    bind(*a, *b)?;
                    let ah = self.high_blocks[a];
                    let bh = self.high_blocks[b];
                    bind(ah.id, bh.id)?;
                    match (&self.blocks[a].terminator, &self.blocks[b].terminator) {
                        (T::Exit { kind: a, .. }, T::Exit { kind: b, .. }) if a == b => {}
                        (T::Trap { reason: a }, T::Trap { reason: b }) if a == b => {}
                        _ => return Err(Error::Correspondence(*b)),
                    }
                    if ah.exit_values.len() != bh.exit_values.len() {
                        return Err(Error::Correspondence(*b));
                    }
                    expressions.extend(
                        ah.exit_values
                            .iter()
                            .copied()
                            .zip(bh.exit_values.iter().copied()),
                    );
                }
                (K::Transfer { edge: a }, K::Transfer { edge: b }) => edges.push((*a, *b)),
                (K::If { .. }, K::If { .. }) => {}
                (
                    K::While {
                        condition: a,
                        condition_nonzero: ap,
                        exit_edge: ae,
                        ..
                    },
                    K::While {
                        condition: b,
                        condition_nonzero: bp,
                        exit_edge: be,
                        ..
                    },
                ) if a.is_some() == b.is_some() && ap == bp => match (ae, be) {
                    (Some(a), Some(b)) => edges.push((*a, *b)),
                    (None, None) => {}
                    _ => return Err(Error::Correspondence(*bn)),
                },
                (K::Break, K::Break) | (K::Continue, K::Continue) => {}
                _ => return Err(Error::Correspondence(*bn)),
            }
            match (self.controls.get(an), self.controls.get(bn)) {
                (None, None) => {}
                (Some(ac), Some(bc)) => {
                    bind(ac.id, bc.id)?;
                    expressions.push((ac.condition, bc.condition));
                    match (&ac.guard, &bc.guard) {
                        (None, None) => {}
                        (Some(a), Some(b)) if a.pass_nonzero == b.pass_nonzero => {
                            ids(
                                &[
                                    a.block,
                                    a.pass_edge,
                                    a.failure_edge,
                                    a.pass_node,
                                    a.failure_node,
                                ],
                                &[
                                    b.block,
                                    b.pass_edge,
                                    b.failure_edge,
                                    b.pass_node,
                                    b.failure_node,
                                ],
                                bind,
                            )?;
                            match (a.payload, b.payload) {
                                (
                                    crate::LiftedGuardPayload::EmptyRevert,
                                    crate::LiftedGuardPayload::EmptyRevert,
                                ) => {}
                                (
                                    crate::LiftedGuardPayload::ErrorString(a),
                                    crate::LiftedGuardPayload::ErrorString(b),
                                ) => self.payload(a, b, bind)?,
                                _ => return Err(Error::Correspondence(*bn)),
                            }
                            match (&a.continuation, &b.continuation) {
                                (None, None) => {}
                                (Some(a), Some(b))
                                    if (a.start, a.end, a.pass_falls_through)
                                        == (b.start, b.end, b.pass_falls_through) =>
                                {
                                    bind(a.sequence, b.sequence)?
                                }
                                _ => return Err(Error::Correspondence(*bn)),
                            }
                        }
                        _ => return Err(Error::Correspondence(*bn)),
                    }
                }
                _ => return Err(Error::Correspondence(*bn)),
            }
        }
        for (a, b) in edges {
            let ae = self.edges[&a];
            let be = self.edges[&b];
            let ah = self.high_transfers[&a];
            let bh = self.high_transfers[&b];
            if ae.kind != be.kind || ah.arguments.len() != bh.arguments.len() {
                return Err(Error::Correspondence(b));
            }
            ids(
                &[a, ah.id, ae.from, ae.to, ae.memory, ae.effect],
                &[b, bh.id, be.from, be.to, be.memory, be.effect],
                bind,
            )?;
            let at = self.blocks[&ae.to];
            let bt = self.blocks[&be.to];
            ids(
                &[at.memory_parameter, at.effect_parameter],
                &[bt.memory_parameter, bt.effect_parameter],
                bind,
            )?;
            ids(
                &self.high_blocks[&ae.to].parameters,
                &self.high_blocks[&be.to].parameters,
                bind,
            )?;
            expressions.extend(
                ah.arguments
                    .iter()
                    .copied()
                    .zip(bh.arguments.iter().copied()),
            );
        }
        let mut seen = BTreeSet::new();
        while let Some((a, b)) = expressions.pop() {
            charge(pay, 4)?;
            bind(a, b)?;
            if !seen.insert((a, b)) {
                continue;
            }
            let a = self.expressions[&a];
            let b = self.expressions[&b];
            if a.ty != b.ty {
                return Err(Error::Correspondence(b.id));
            }
            bind(a.block, b.block)?;
            use crate::LiftedExpressionKind as X;
            match (&a.kind, &b.kind) {
                (X::Word(a), X::Word(b)) if a == b => {}
                (X::Predicate(a), X::Predicate(b)) if a == b => {}
                (X::Value(a), X::Value(b)) => bind(*a, *b)?,
                (X::WordFromPredicate(a), X::WordFromPredicate(b))
                | (X::NonZero(a), X::NonZero(b))
                | (X::Not(a), X::Not(b)) => expressions.push((*a, *b)),
                (
                    X::Compare {
                        relation: a,
                        left: al,
                        right: ar,
                    },
                    X::Compare {
                        relation: b,
                        left: bl,
                        right: br,
                    },
                ) if a == b => expressions.extend([(*al, *bl), (*ar, *br)]),
                (
                    X::WordOp {
                        opcode: a,
                        arguments: aa,
                    },
                    X::WordOp {
                        opcode: b,
                        arguments: ba,
                    },
                ) if a == b && aa.len() == ba.len() => {
                    charge(pay, aa.len())?;
                    expressions.extend(aa.iter().copied().zip(ba.iter().copied()));
                }
                _ => return Err(Error::Correspondence(b.id)),
            }
        }
        Ok(())
    }
    fn payload(
        &self,
        a: EntityId,
        b: EntityId,
        bind: &mut impl FnMut(EntityId, EntityId) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let ap = self.payloads.get(&a).ok_or(Error::Missing(a))?;
        let bp = self.payloads.get(&b).ok_or(Error::Missing(b))?;
        if ap.bytes != bp.bytes || ap.message != bp.message {
            return Err(Error::Correspondence(b));
        }
        bind(a, b)
    }
    /// Validate supplied finite semantic/source correspondence directly.
    pub fn verify_correspondence(
        &self,
        a: &SharedRegion,
        b: &SharedRegion,
        bindings: &[SharedBinding],
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(), Error> {
        if !a.terminal_closed || !b.terminal_closed {
            return Err(Error::Region(b.root));
        }
        charge(
            pay,
            bindings
                .len()
                .saturating_add(a.entities.len())
                .saturating_mul(8),
        )?;
        if bindings.len() > a.entities.len()
            || bindings.windows(2).any(|w| w[0].formal >= w[1].formal)
            || bindings
                .iter()
                .any(|b| a.entities.binary_search(&b.formal).is_err())
        {
            return Err(Error::Correspondence(b.root));
        }
        let mut map: BTreeMap<_, _> = a.entities.iter().map(|v| (*v, *v)).collect();
        for b in bindings {
            map.insert(b.formal, b.actual);
        }
        // Parameter substitution may identify two free words (helper(z, z)).
        // Definitions, statements, transfers and state versions retain their
        // distinct source roles. Value-expression wrappers of free inputs may
        // identify along with those inputs; no internal definition may alias.
        let mut aliasable: BTreeSet<_> = a.interface.inputs.iter().copied().collect();
        if !a.nodes.is_empty() {
            for id in &a.entities {
                if self.expressions.get(id).is_some_and(|e| {
                    matches!(e.kind, crate::LiftedExpressionKind::Value(v) if a.interface.inputs.binary_search(&v).is_ok())
                }) {
                    aliasable.insert(*id);
                }
            }
        }
        let mut targets = BTreeMap::new();
        for (formal, actual) in &map {
            if let Some(previous) = targets.insert(*actual, *formal)
                && (!aliasable.contains(formal) || !aliasable.contains(&previous))
            {
                return Err(Error::Correspondence(*formal));
            }
        }
        if targets.keys().copied().ne(b.entities.iter().copied()) {
            return Err(Error::Correspondence(b.root));
        }
        self.compare(
            a,
            b,
            &mut |a, b| {
                if map.get(&a) == Some(&b) {
                    Ok(())
                } else {
                    Err(Error::Correspondence(a))
                }
            },
            pay,
        )?;
        if !interface_matches(&a.interface, &b.interface, &map) {
            return Err(Error::Interface(b.root));
        }
        Ok(())
    }
}
fn interface_matches(
    a: &SharedInterface,
    b: &SharedInterface,
    map: &BTreeMap<EntityId, EntityId>,
) -> bool {
    let equal = |a: &[EntityId], b: &[EntityId], aliases: bool| {
        let Some(mut actual) = a
            .iter()
            .map(|id| map.get(id).copied())
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        actual.sort_unstable();
        if aliases {
            actual.dedup();
        }
        actual == b
    };
    [
        &a.inputs,
        &a.outputs,
        &a.memory_inputs,
        &a.memory_outputs,
        &a.effect_inputs,
        &a.effect_outputs,
    ]
    .into_iter()
    .zip([
        &b.inputs,
        &b.outputs,
        &b.memory_inputs,
        &b.memory_outputs,
        &b.effect_inputs,
        &b.effect_outputs,
    ])
    .enumerate()
    .all(|(position, (a, b))| equal(a, b, position == 0))
        && {
            let mapped: Option<BTreeSet<_>> = a
                .continuations
                .iter()
                .map(|c| Some((*map.get(&c.edge)?, *map.get(&c.target)?)))
                .collect();
            mapped == Some(b.continuations.iter().map(|c| (c.edge, c.target)).collect())
        }
}
