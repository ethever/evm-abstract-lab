//! Flat typed DAG admission, independent of raw-witness certification.
use super::{
    LiftedExpression, LiftedExpressionKind, LiftedModelError, LiftedProgram, LiftedType,
    MAX_LIFTED_RECORDS,
};
use crate::EntityId;
use std::collections::{BTreeMap, BTreeSet};

impl LiftedProgram {
    /// Bounded acyclic/type admission. Witness correspondence is checked by core.
    pub fn validate(&self) -> Result<(), LiftedModelError> {
        if self.record_count() > MAX_LIFTED_RECORDS {
            return Err(LiftedModelError::RecordLimit);
        }
        self.validate_shared_model()?;
        let mut ids = BTreeSet::new();
        for id in self.entity_ids() {
            if id.0 == 0 || !ids.insert(id) {
                return Err(LiftedModelError::DuplicateIdentity(id));
            }
        }
        let mut previous = BTreeMap::<EntityId, &LiftedExpression>::new();
        for e in &self.expressions {
            if e.ty != e.kind.ty() {
                return Err(LiftedModelError::Type(e.id));
            }
            for operand in e.kind.operands() {
                if !previous
                    .get(&operand)
                    .is_some_and(|child| child.block == e.block)
                {
                    return Err(LiftedModelError::Operand(e.id));
                }
            }
            let sort = |id: &EntityId, ty| previous.get(id).is_some_and(|child| child.ty == ty);
            let correct = match &e.kind {
                LiftedExpressionKind::WordFromPredicate(id) | LiftedExpressionKind::Not(id) => {
                    sort(id, LiftedType::Predicate)
                }
                LiftedExpressionKind::NonZero(id) => sort(id, LiftedType::Word256),
                LiftedExpressionKind::Compare { left, right, .. } => {
                    sort(left, LiftedType::Word256) && sort(right, LiftedType::Word256)
                }
                LiftedExpressionKind::WordOp { opcode, arguments } => {
                    evm_bytecode::word::arity(*opcode) == Some(arguments.len())
                        && arguments.iter().all(|id| sort(id, LiftedType::Word256))
                }
                _ => true,
            };
            if !correct {
                return Err(LiftedModelError::Type(e.id));
            }
            previous.insert(e.id, e);
        }
        let word = |id: &EntityId| {
            previous
                .get(id)
                .is_some_and(|e| e.ty == LiftedType::Word256)
        };
        let predicate = |id: &EntityId| {
            previous
                .get(id)
                .is_some_and(|e| e.ty == LiftedType::Predicate)
        };
        for b in &self.blocks {
            let word_in_scope = |id: &EntityId| {
                previous
                    .get(id)
                    .is_some_and(|e| e.ty == LiftedType::Word256 && e.block == b.block)
            };
            let predicate_in_scope = |id: &EntityId| {
                previous
                    .get(id)
                    .is_some_and(|e| e.ty == LiftedType::Predicate && e.block == b.block)
            };
            if b.condition
                .as_ref()
                .is_some_and(|id| !predicate_in_scope(id))
                || b.target.as_ref().is_some_and(|id| !word_in_scope(id))
                || !b.exit_values.iter().all(word_in_scope)
            {
                return Err(LiftedModelError::Type(b.id));
            }
            for s in &b.statements {
                if !s.inputs.iter().all(word_in_scope) {
                    return Err(LiftedModelError::Type(s.id));
                }
            }
        }
        for t in &self.transfers {
            if !t.arguments.iter().all(word) {
                return Err(LiftedModelError::Type(t.id));
            }
        }
        for c in &self.controls {
            if !predicate(&c.condition) {
                return Err(LiftedModelError::Type(c.id));
            }
        }
        let blocks: BTreeSet<_> = self.blocks.iter().map(|b| b.block).collect();
        let edges: BTreeSet<_> = self.transfers.iter().map(|t| t.edge).collect();
        let payloads: BTreeSet<_> = self.payloads.iter().map(|p| p.id).collect();
        let data: BTreeMap<_, _> = self.revert_data.iter().map(|p| (p.id, p)).collect();
        let mut data_blocks = BTreeSet::new();
        for item in &self.revert_data {
            item.validate()?;
            if !blocks.contains(&item.block) || !data_blocks.insert(item.block) {
                return Err(LiftedModelError::Payload(item.id));
            }
        }
        for payload in &self.payloads {
            if !payload.is_canonical() {
                return Err(LiftedModelError::Payload(payload.id));
            }
        }
        let valid_payload = |p: super::LiftedGuardPayload| match p {
            super::LiftedGuardPayload::EmptyRevert => true,
            super::LiftedGuardPayload::ErrorString(id) => payloads.contains(&id),
            super::LiftedGuardPayload::Data(id) => data.contains_key(&id),
        };
        let mut revert_blocks = BTreeSet::new();
        let mut reverts = BTreeMap::new();
        for r in &self.reverts {
            if r.execution_domain.is_generic() != r.execution_fork.is_none() {
                return Err(LiftedModelError::Payload(r.id));
            }
            if !payloads.contains(&r.payload)
                || !blocks.contains(&r.block)
                || !revert_blocks.insert(r.block)
                || r.writes.is_empty()
                || r.writes.len() > super::MAX_ERROR_PAYLOAD_BYTES
                || [r.operation, r.memory, r.offset, r.size]
                    .iter()
                    .any(|id| id.0 == 0)
                || r.writes.iter().any(|id| id.0 == 0)
            {
                return Err(LiftedModelError::Payload(r.id));
            }
            reverts.insert(r.id, r);
        }
        let high: BTreeMap<_, _> = self.blocks.iter().map(|b| (b.block, b)).collect();
        let mut construction_blocks = BTreeSet::new();
        for c in &self.constructions {
            if !construction_blocks.insert(c.block)
                || c.start >= c.end
                || !high
                    .get(&c.block)
                    .is_some_and(|b| c.end == b.statements.len())
                || !(reverts.get(&c.revert).is_some_and(|r| r.block == c.block)
                    || data.get(&c.revert).is_some_and(|r| {
                        r.block == c.block && !matches!(r.content, super::LiftedRevertDataKind::Raw)
                    }))
            {
                return Err(LiftedModelError::Payload(c.id));
            }
        }
        let mut failures = BTreeSet::new();
        for f in &self.failures {
            if !valid_payload(f.payload)
                || f.block.0 == 0
                || !blocks.contains(&f.block)
                || !failures.insert(f.block)
                || f.rank as usize >= blocks.len()
            {
                return Err(LiftedModelError::Guard(f.block));
            }
        }
        let mut controls = BTreeSet::new();
        let mut omissions = BTreeSet::new();
        for c in &self.controls {
            if !controls.insert(c.node) {
                return Err(LiftedModelError::Guard(c.node));
            }
            if let Some(g) = &c.guard
                && (!valid_payload(g.payload)
                    || c.node.0 == 0
                    || g.source_references().any(|id| id.0 == 0)
                    || !blocks.contains(&g.block)
                    || g.pass_node.0 == 0
                    || g.failure_node.0 == 0
                    || g.pass_node == g.failure_node
                    || g.pass_node == c.node
                    || g.failure_node == c.node
                    || g.pass_edge == g.failure_edge
                    || !edges.contains(&g.pass_edge)
                    || !edges.contains(&g.failure_edge)
                    || g.continuation.as_ref().is_some_and(|s| {
                        s.sequence.0 == 0
                            || s.start == 0
                            || s.start >= s.end
                            || s.end > crate::MAX_STRUCTURED_WIRE_NODES
                            || !omissions.insert((s.sequence, s.start))
                    }))
            {
                return Err(LiftedModelError::Guard(c.node));
            }
        }
        Ok(())
    }
}
