//! The original eager memory predicate, prepaid on the transfer's root ledger.
//!
//! Caller copies and local destruction have their own owners. AlgebraRef pricing
//! still owns numeric execution, reduction, provenance transfer and expression
//! construction, including its internal copies. Refusal publishes no guard.

use crate::domain::{AlgebraRef, EvmNumericAlgebra, WordElement};

use super::{
    EvmRangeRequest, EvmValue, GuardRetention, RangeKind, RangeQueries, TransferContext, logical,
};

use super::super::query;
#[cfg(test)]
use super::literal_memory_singleton_work;
use crate::{
    analysis::FrontierReason,
    domain::{SingletonProof, symbolic::ExprId},
    resource::WorkBudget,
};
use revm_bytecode::opcode;

// A fixed tariff for at most two operands' cached metadata and scalar quotes:
// no candidate iteration, expression traversal, or allocation occurs here.
const QUOTE_WORK: usize = 32 * 4;
const VALUE_INLINE_WORK: usize = 32;

pub(super) struct PreparedGuard<N: WordElement> {
    pub(super) request: EvmRangeRequest<N>,
    pub(super) condition: EvmValue<N>,
    pub(super) retention: GuardRetention,
    pub(super) singleton: SingletonProof,
}

struct Builder<'a, D: EvmNumericAlgebra> {
    domain: AlgebraRef<'a, D>,
    budget: &'a mut WorkBudget,
    retention: GuardRetention,
}

impl<D: EvmNumericAlgebra> Builder<'_, D> {
    fn pay(&mut self, work: usize) -> Result<(), FrontierReason> {
        query::checkpoint(self.budget)?;
        if work == usize::MAX || !self.budget.charge(work) {
            return Err(FrontierReason::Work);
        }
        query::checkpoint(self.budget)
    }

    #[cfg(test)]
    fn identify(
        &mut self,
        value: &EvmValue<D::Element>,
        enabled: bool,
        fresh: impl FnOnce() -> Option<ExprId>,
    ) -> Result<(EvmValue<D::Element>, bool), FrontierReason> {
        self.identify_after_proof(value, enabled, SingletonProof::NotQueried, fresh)
    }

    fn identify_after_proof(
        &mut self,
        value: &EvmValue<D::Element>,
        enabled: bool,
        mut proof: SingletonProof,
        fresh: impl FnOnce() -> Option<ExprId>,
    ) -> Result<(EvmValue<D::Element>, bool), FrontierReason> {
        self.pay(QUOTE_WORK)?;
        // The extra leaf covers identify's possible fresh expression. This
        // reservation includes the copy's release on every later exit path.
        self.pay(value_copy_work(value).saturating_add(2))?;
        let needs_identity = enabled
            && value.expression().is_none()
            && query::resolve(value.numeric(), &mut proof, self.budget)?.is_none();
        let value = value.clone();
        query::checkpoint(self.budget)?;
        if needs_identity {
            return Ok(match fresh() {
                Some(expression) => (value.with_expression(expression), false),
                None => (value, true),
            });
        }
        Ok((value, false))
    }

    fn copied(
        &mut self,
        op: u8,
        args: [&EvmValue<D::Element>; 2],
    ) -> Result<EvmValue<D::Element>, FrontierReason> {
        self.pay(QUOTE_WORK)?;
        let work = args.iter().fold(0usize, |work, arg| {
            work.saturating_add(value_copy_work(arg))
        });
        self.pay(work)?;
        let args = args.map(Clone::clone);
        self.apply(op, &args)
    }

    fn apply(
        &mut self,
        op: u8,
        args: &[EvmValue<D::Element>],
    ) -> Result<EvmValue<D::Element>, FrontierReason> {
        Ok(self.apply_with_singleton(op, args)?.0)
    }

    fn apply_with_singleton(
        &mut self,
        op: u8,
        args: &[EvmValue<D::Element>],
    ) -> Result<(EvmValue<D::Element>, SingletonProof), FrontierReason> {
        self.pay(QUOTE_WORK)?;
        // The algebra owns creation/retention; this pays only the caller's eventual
        // release, even if a later stage refuses. Successful copy/release
        // prepayments also keep the algebra's unchanged small-slice work sum finite.
        self.pay(output_release_work(self.domain, op, args))?;
        let work = self.domain.operation_work_for(op, args);
        self.pay(work)?;
        let (reduction, singleton) = self.domain.apply_detailed_with_singleton(op, args);
        query::checkpoint(self.budget)?;
        self.retention.record(reduction.symbolic_error.as_ref());
        Ok((reduction.value, singleton))
    }
}

fn release_work(work: usize) -> usize {
    work.saturating_add(VALUE_INLINE_WORK)
}

fn value_copy_work<N: WordElement>(value: &EvmValue<N>) -> usize {
    value
        .clone_work()
        .saturating_add(release_work(value.work_size()))
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

#[cfg(test)]
pub(super) fn build<D: EvmNumericAlgebra>(
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
) -> Result<PreparedGuard<D::Element>, FrontierReason> {
    build_with_allocator(offset, size, context, ExprId::fresh)
}

pub(super) fn build_after_queries<D: EvmNumericAlgebra>(
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    queries: RangeQueries,
    context: &mut TransferContext<'_, D>,
) -> Result<PreparedGuard<D::Element>, FrontierReason> {
    build_with_queries(offset, size, queries, context, ExprId::fresh)
}

#[cfg(test)]
fn build_with_allocator<D: EvmNumericAlgebra>(
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
    fresh: impl FnMut() -> Option<ExprId>,
) -> Result<PreparedGuard<D::Element>, FrontierReason> {
    build_with_queries(offset, size, RangeQueries::default(), context, fresh)
}

fn build_with_queries<D: EvmNumericAlgebra>(
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    queries: RangeQueries,
    context: &mut TransferContext<'_, D>,
    mut fresh: impl FnMut() -> Option<ExprId>,
) -> Result<PreparedGuard<D::Element>, FrontierReason> {
    // Pay before reading operand metadata or constructing any local value.
    query::checkpoint(context.budget)?;
    if !context.budget.charge(QUOTE_WORK) {
        return Err(FrontierReason::Work);
    }
    let mut builder = Builder {
        domain: context.domain,
        budget: context.budget,
        retention: GuardRetention::new(offset, size),
    };
    let enabled = context.config.analysis.relations.enabled;
    let (offset, offset_identity_failed) =
        builder.identify_after_proof(offset, enabled, queries.offset, &mut fresh)?;
    let (size, size_identity_failed) =
        builder.identify_after_proof(size, enabled, queries.size, &mut fresh)?;
    builder.retention = GuardRetention::new(&offset, &size);
    // A declined fresh identity leaves the value unmarked. Its explicit veto
    // must survive any later finite operation error in this guard.
    if offset_identity_failed || size_identity_failed {
        builder.retention.veto_identity_failure();
    }
    // Both literals are real opaque constructions. The domain's literal quote
    // owns their construction and release; a default product size cannot price N.
    builder.pay(
        builder
            .domain
            .literal_quote_work(logical::MAX_END)
            .saturating_mul(2),
    )?;
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
    let (condition, singleton) = builder.apply_with_singleton(opcode::OR, &[empty, fits])?;

    Ok(PreparedGuard {
        request,
        condition,
        retention: builder.retention,
        singleton,
    })
}

pub(super) fn sequence<D: EvmNumericAlgebra>(
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    length: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
) -> Result<PreparedGuard<D::Element>, FrontierReason> {
    let mut builder = Builder {
        domain: context.domain,
        budget: context.budget,
        retention: GuardRetention::new(offset, size),
    };
    builder.pay(QUOTE_WORK)?;
    let enabled = context.config.analysis.relations.enabled;
    let (offset, _) =
        builder.identify_after_proof(offset, enabled, SingletonProof::NotQueried, ExprId::fresh)?;
    let (size, _) =
        builder.identify_after_proof(size, enabled, SingletonProof::NotQueried, ExprId::fresh)?;
    let (limit, _) =
        builder.identify_after_proof(length, enabled, SingletonProof::NotQueried, ExprId::fresh)?;
    let request = EvmRangeRequest {
        kind: RangeKind::ReturnData,
        offset,
        size,
        limit,
    };
    let greater = builder.copied(opcode::GT, [&request.offset, &request.limit])?;
    let offset_fits = builder.apply(opcode::ISZERO, &[greater])?;
    let remaining = builder.copied(opcode::SUB, [&request.limit, &request.offset])?;
    let greater = builder.copied(opcode::GT, [&request.size, &remaining])?;
    let size_fits = builder.apply(opcode::ISZERO, &[greater])?;
    let (condition, singleton) =
        builder.apply_with_singleton(opcode::AND, &[offset_fits, size_fits])?;
    Ok(PreparedGuard {
        request,
        condition,
        singleton,
        retention: builder.retention,
    })
}

#[cfg(test)]
mod tests;
