//! The original eager memory predicate, prepaid on the transfer's root ledger.
//!
//! Caller copies and local destruction have their own owners. AlgebraRef pricing
//! still owns numeric execution, reduction, provenance transfer and expression
//! construction, including its internal copies. Refusal publishes no guard.

use crate::domain::WordElement;

use super::{
    AlgebraRef, EvmRangeRequest, EvmValue, GuardRetention, RangeKind, TransferContext,
    literal_memory_singleton_work, logical,
};

use crate::{
    domain::{WorkExhausted, symbolic::ExprId},
    resource::WorkBudget,
};
use revm_bytecode::opcode;

// A fixed tariff for at most two operands' cached metadata and scalar quotes:
// no candidate iteration, expression traversal, or allocation occurs here.
const QUOTE_WORK: usize = 32 * 4;
const VALUE_INLINE_WORK: usize = 32;
const CONSTANT_WORK_SIZE: usize = 14;

pub(super) struct PreparedGuard<N: WordElement> {
    pub(super) request: EvmRangeRequest<N>,
    pub(super) condition: EvmValue<N>,
    pub(super) retention: GuardRetention,
}

struct Builder<'a, D: EvmNumericAlgebra> {
    domain: AlgebraRef<'a, D>,
    budget: &'a mut WorkBudget,
    retention: GuardRetention,
}

impl<D: EvmNumericAlgebra> Builder<'_, D> {
    fn pay(&mut self, work: usize) -> Result<(), WorkExhausted> {
        if self.budget.exhausted() || !self.budget.charge(work) {
            return Err(WorkExhausted);
        }
        Ok(())
    }

    fn identify(
        &mut self,
        value: &EvmValue<D::Element>,
        enabled: bool,
        fresh: impl FnOnce() -> Option<ExprId>,
    ) -> Result<(EvmValue<D::Element>, bool), WorkExhausted> {
        self.pay(QUOTE_WORK)?;
        // The extra leaf covers identify's possible fresh expression. This
        // reservation includes the copy's release on every later exit path.
        let work = copy_work(value.work_size().saturating_add(1)).saturating_add(if enabled {
            literal_memory_singleton_work(value)
        } else {
            0
        });
        self.pay(work)?;
        Ok(identify(value.clone(), enabled, fresh))
    }

    fn copied(
        &mut self,
        op: u8,
        args: [&EvmValue<D::Element>; 2],
    ) -> Result<EvmValue<D::Element>, WorkExhausted> {
        self.pay(QUOTE_WORK)?;
        let work = args.iter().fold(0usize, |work, arg| {
            work.saturating_add(copy_work(arg.work_size()))
        });
        self.pay(work)?;
        let args = args.map(Clone::clone);
        self.apply(op, &args)
    }

    fn apply(
        &mut self,
        op: u8,
        args: &[EvmValue<D::Element>],
    ) -> Result<EvmValue<D::Element>, WorkExhausted> {
        self.pay(QUOTE_WORK)?;
        // The algebra owns creation/retention; this pays only the caller's eventual
        // release, even if a later stage refuses. Successful copy/release
        // prepayments also keep the algebra's unchanged small-slice work sum finite.
        self.pay(output_release_work(self.domain, op, args))?;
        let work = self.domain.operation_work(args);
        self.pay(work)?;
        let reduction = self.domain.apply_detailed(op, args);
        self.retention.record(reduction.symbolic_error.as_ref());
        Ok(reduction.value)
    }
}

// Keep relations::identify's query order and returned value, but retain the
// otherwise unmarked identity refusal without repeating its singleton query.
fn identify<N: WordElement>(
    value: EvmValue<N>,
    enabled: bool,
    fresh: impl FnOnce() -> Option<ExprId>,
) -> (EvmValue<N>, bool) {
    if enabled && value.singleton().is_none() && value.expression().is_none() {
        return match fresh() {
            Some(expression) => (value.with_expression(expression), false),
            None => (value, true),
        };
    }
    (value, false)
}

fn release_work(work: usize) -> usize {
    work.saturating_add(VALUE_INLINE_WORK)
}

fn copy_work(work: usize) -> usize {
    release_work(work).saturating_mul(2)
}

/// Only the eight operations below use this bound. Their Boolean outputs have
/// at most two candidates. SUB has at most the finite input tuple count, or the
/// domain's existing candidate limit when reduction starts from an open value.
fn output_release_work<D: EvmNumericAlgebra>(
    domain: AlgebraRef<'_, D>,
    op: u8,
    args: &[EvmValue<D::Element>],
) -> usize {
    let candidates = if op == opcode::SUB {
        args.iter()
            .try_fold(1usize, |tuples, arg| {
                arg.finite_constants()
                    .cardinality()
                    .map(|len| tuples.saturating_mul(len))
            })
            .unwrap_or_else(|| {
                if domain.spec().profile() == crate::domain::Profile::ConstantsOnly {
                    1
                } else {
                    domain.capacity()
                }
            })
            .min(domain.capacity())
    } else {
        2.min(domain.capacity())
    };
    let nodes = if domain.spec().relations().enabled {
        args.iter().fold(1usize, |nodes, arg| {
            nodes.saturating_add(arg.expression().map_or(1, |expr| expr.nodes()))
        })
    } else {
        0
    };
    release_work(
        candidates
            .saturating_add(12)
            .saturating_add(crate::domain::provenance::MAX_ORIGINS)
            .saturating_add(nodes),
    )
}

pub(super) fn build<D: EvmNumericAlgebra>(
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
) -> Result<PreparedGuard<D::Element>, WorkExhausted> {
    build_with_allocator(offset, size, context, ExprId::fresh)
}

fn build_with_allocator<D: EvmNumericAlgebra>(
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
    mut fresh: impl FnMut() -> Option<ExprId>,
) -> Result<PreparedGuard<D::Element>, WorkExhausted> {
    // Pay before reading operand metadata or constructing any local value.
    if context.budget.exhausted() || !context.budget.charge(QUOTE_WORK) {
        return Err(WorkExhausted);
    }
    let mut builder = Builder {
        domain: context.domain,
        budget: context.budget,
        retention: GuardRetention::new(offset, size),
    };
    let enabled = context.config.analysis.relations.enabled;
    let (offset, offset_identity_failed) = builder.identify(offset, enabled, &mut fresh)?;
    let (size, size_identity_failed) = builder.identify(size, enabled, &mut fresh)?;
    builder.retention = GuardRetention::new(&offset, &size);
    // A declined fresh identity leaves the value unmarked. Its explicit veto
    // must survive any later finite operation error in this guard.
    if offset_identity_failed || size_identity_failed {
        builder.retention.veto_identity_failure();
    }
    builder.pay(copy_work(CONSTANT_WORK_SIZE).saturating_mul(2))?;
    let request = EvmRangeRequest {
        kind: RangeKind::LogicalMemory,
        offset,
        size,
        limit: EvmValue::<D::Element>::constant(logical::MAX_END),
    };
    let limit = EvmValue::<D::Element>::constant(logical::MAX_END);

    let empty = builder.apply(opcode::ISZERO, std::slice::from_ref(&request.size))?;
    let greater = builder.copied(opcode::GT, [&request.offset, &limit])?;
    let offset_fits = builder.apply(opcode::ISZERO, &[greater])?;
    let remaining = builder.copied(opcode::SUB, [&limit, &request.offset])?;
    let greater = builder.copied(opcode::GT, [&request.size, &remaining])?;
    let size_fits = builder.apply(opcode::ISZERO, &[greater])?;
    let fits = builder.apply(opcode::AND, &[offset_fits, size_fits])?;
    let condition = builder.apply(opcode::OR, &[empty, fits])?;

    builder.pay(QUOTE_WORK)?;
    builder.pay(literal_memory_singleton_work(&condition))?;
    Ok(PreparedGuard {
        request,
        condition,
        retention: builder.retention,
    })
}

#[cfg(test)]
mod tests;
