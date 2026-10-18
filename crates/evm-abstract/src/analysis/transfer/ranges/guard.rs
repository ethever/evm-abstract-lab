//! The original eager memory predicate, prepaid on the transfer's root ledger.
//!
//! Caller copies and local destruction have their own owners. Domain pricing
//! still owns numeric execution, reduction, provenance transfer and expression
//! construction, including its internal copies. Refusal publishes no guard.

use super::{
    AbstractValue, Domain, GuardRetention, RangeKind, RangeRequest, TransferContext,
    literal_memory_singleton_work, logical,
};
use crate::{domain::WorkExhausted, resource::WorkBudget};
use revm_bytecode::opcode;

// A fixed tariff for at most two operands' cached metadata and scalar quotes:
// no candidate iteration, expression traversal, or allocation occurs here.
const QUOTE_WORK: usize = 32 * 4;
const VALUE_INLINE_WORK: usize = 32;
const CONSTANT_WORK_SIZE: usize = 14;

pub(super) struct PreparedGuard {
    pub(super) request: RangeRequest,
    pub(super) condition: AbstractValue,
    pub(super) retention: GuardRetention,
}

struct Builder<'a> {
    domain: Domain,
    budget: &'a mut WorkBudget,
    retention: GuardRetention,
}

impl Builder<'_> {
    fn pay(&mut self, work: usize) -> Result<(), WorkExhausted> {
        if self.budget.exhausted() || !self.budget.charge(work) {
            return Err(WorkExhausted);
        }
        Ok(())
    }

    fn identify(
        &mut self,
        value: &AbstractValue,
        enabled: bool,
    ) -> Result<AbstractValue, WorkExhausted> {
        self.pay(QUOTE_WORK)?;
        // The extra leaf covers identify's possible fresh expression. This
        // reservation includes the copy's release on every later exit path.
        let work = copy_work(value.work_size().saturating_add(1)).saturating_add(if enabled {
            literal_memory_singleton_work(value)
        } else {
            0
        });
        self.pay(work)?;
        Ok(super::super::relations::identify(value.clone(), enabled))
    }

    fn copied(
        &mut self,
        op: u8,
        args: [&AbstractValue; 2],
    ) -> Result<AbstractValue, WorkExhausted> {
        self.pay(QUOTE_WORK)?;
        let work = args.iter().fold(0usize, |work, arg| {
            work.saturating_add(copy_work(arg.work_size()))
        });
        self.pay(work)?;
        let args = args.map(Clone::clone);
        self.apply(op, &args)
    }

    fn apply(&mut self, op: u8, args: &[AbstractValue]) -> Result<AbstractValue, WorkExhausted> {
        self.pay(QUOTE_WORK)?;
        // Domain owns creation/retention; this pays only the caller's eventual
        // release, even if a later stage refuses. Successful copy/release
        // prepayments also keep Domain's unchanged small-slice work sum finite.
        self.pay(output_release_work(self.domain, op, args))?;
        let work = self.domain.operation_work(args);
        self.pay(work)?;
        let reduction = self.domain.apply_detailed(op, args);
        self.retention.record(reduction.symbolic_error.as_ref());
        Ok(reduction.value)
    }
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
fn output_release_work(domain: Domain, op: u8, args: &[AbstractValue]) -> usize {
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

pub(super) fn build(
    offset: &AbstractValue,
    size: &AbstractValue,
    context: &mut TransferContext<'_>,
) -> Result<PreparedGuard, WorkExhausted> {
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
    let offset = builder.identify(offset, enabled)?;
    let size = builder.identify(size, enabled)?;
    // Identification can itself lose a fresh identity; do not attribute that
    // inherited refusal to a later finite operation in this guard.
    builder.retention = GuardRetention::new(&offset, &size);
    builder.pay(copy_work(CONSTANT_WORK_SIZE).saturating_mul(2))?;
    let request = RangeRequest {
        kind: RangeKind::LogicalMemory,
        offset,
        size,
        limit: AbstractValue::constant(logical::MAX_END),
    };
    let limit = AbstractValue::constant(logical::MAX_END);

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
