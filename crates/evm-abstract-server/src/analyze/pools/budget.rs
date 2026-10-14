//! Native comparison/copy metering, distinct from expanding the report's DAG.
//! Entries, map lookups and fixed-size node data are metering primitives; set
//! contents and recursive scalar comparisons record their retained cardinality.
use super::symbolic;
use evm_abstract::{
    domain::{
        AbstractValue,
        symbolic::{ExprId, ExprKind},
    },
    world::ByteArray,
};
use evm_abstract_protocol as api;

pub(super) struct BufferWork {
    copy: usize,
}

impl BufferWork {
    pub(super) fn new(
        source: &ByteArray,
        export: &mut symbolic::Pool<'_>,
    ) -> Result<Self, api::ApiError> {
        // Count reads are O(1). Record the scan before inspecting entries.
        export.charge(source.metadata_len())?;
        // Root handle, memory flag and fixed-size RangeError/failure marker.
        let mut copy = 3usize;
        for value in source
            .stored_bytes()
            .map(|(_, value)| value)
            .chain(source.symbolic_facts().map(|(_, value)| value))
            .chain([source.len(), source.default_byte()])
        {
            export.charge(0)?;
            copy = copy.saturating_add(scalar_copy_work(value));
        }
        copy = copy
            .saturating_add(source.stored_bytes().len())
            .saturating_add(source.symbolic_facts().len());
        for _ in source.allocation_facts() {
            export.charge(0)?;
            copy = copy.saturating_add(2);
        }
        Ok(Self { copy })
    }

    pub(super) fn comparison(
        &self,
        source: &ByteArray,
        candidate: &ByteArray,
        export: &mut symbolic::Pool<'_>,
    ) -> Result<bool, api::ApiError> {
        export.charge(5)?;
        if source.state_root() != candidate.state_root()
            || source.is_memory() != candidate.is_memory()
            || source.stored_bytes().len() != candidate.stored_bytes().len()
            || source.symbolic_facts().len() != candidate.symbolic_facts().len()
            || source.allocation_facts().len() != candidate.allocation_facts().len()
        {
            return Ok(false);
        }
        export.charge(source.metadata_len())?;
        let mut work = self.copy;
        for (incoming, retained) in [
            (source.len(), candidate.len()),
            (source.default_byte(), candidate.default_byte()),
        ] {
            export.charge(0)?;
            work = work.saturating_add(scalar_extra(incoming, retained, export)?);
        }
        for ((_, incoming), (_, retained)) in source.stored_bytes().zip(candidate.stored_bytes()) {
            export.charge(0)?;
            work = work.saturating_add(scalar_extra(incoming, retained, export)?);
        }
        for ((key, incoming), (other_key, retained)) in
            source.symbolic_facts().zip(candidate.symbolic_facts())
        {
            export.charge(0)?;
            work = work.saturating_add(expression_extra(key, other_key, export)?);
            work = work.saturating_add(scalar_extra(incoming, retained, export)?);
        }
        for ((at, size), (other_at, other_size)) in
            source.allocation_facts().zip(candidate.allocation_facts())
        {
            export.charge(0)?;
            work = work.saturating_add(expression_extra(at, other_at, export)?);
            work = work.saturating_add(expression_extra(size, other_size, export)?);
        }
        // Only the unchanged complete native equality can accept a candidate.
        export.charge(work)?;
        Ok(true)
    }

    pub(super) fn copy(&self, export: &mut symbolic::Pool<'_>) -> Result<(), api::ApiError> {
        export.charge(self.copy)
    }
}

fn scalar_copy_work(value: &AbstractValue) -> usize {
    // NumericValue and OriginSet use cached set lengths. The counted elements
    // estimate the later copy/compare; reading this estimate does not scan sets.
    value
        .numeric()
        .work_size()
        .saturating_add(value.provenance().origins().work_size())
        // Code-address role, identity, symbolic-limit flag and optional handle.
        .saturating_add(4)
}

fn scalar_extra(
    source: &AbstractValue,
    candidate: &AbstractValue,
    export: &mut symbolic::Pool<'_>,
) -> Result<usize, api::ApiError> {
    match (source.expression(), candidate.expression()) {
        (Some(left), Some(right)) => expression_extra(left, right, export),
        _ => Ok(0),
    }
}

fn expression_extra(
    source: &ExprId,
    candidate: &ExprId,
    export: &mut symbolic::Pool<'_>,
) -> Result<usize, api::ApiError> {
    if std::ptr::eq(source.kind(), candidate.kind()) {
        return Ok(0);
    }
    // Full ExprId equality checks fingerprint, then recursive scalar structure.
    // Byte/slot state handles compare by id; their histories/payloads are never
    // traversed by equality. Count expanded scalar work even on collisions.
    let mut work = 0usize;
    let mut pending = vec![source];
    while let Some(expression) = pending.pop() {
        export.charge(1)?;
        work = work.saturating_add(1);
        let (state, children) = match expression.kind() {
            ExprKind::Operation { args, .. } => (false, args.iter().collect::<Vec<_>>()),
            ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } => (false, vec![condition, then_value, else_value]),
            ExprKind::ByteRead { index, .. } => (true, vec![index]),
            ExprKind::SlotRead { key, .. } => (true, vec![key]),
            ExprKind::WordRead { view, offset } => (true, vec![&view.offset, &view.length, offset]),
            ExprKind::Keccak { input } => (true, vec![&input.offset, &input.length]),
            ExprKind::Constant(_) | ExprKind::Input(_) | ExprKind::Fresh(_) => (false, vec![]),
        };
        if state {
            export.charge(1)?;
            work = work.saturating_add(1);
        }
        export.charge(children.len())?;
        pending.extend(children);
    }
    // The copy estimate already includes the first expression-handle operation.
    Ok(work.saturating_sub(1))
}
