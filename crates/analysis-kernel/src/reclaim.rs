//! Scoped payload ownership for statically composed provider stores.

use crate::{
    Capability, Channel, Cons, Kernel, KernelError, Meter, Nil, Scope, algebra::PublicationPolicy,
};

/// Release an already retired scope's cells from an owned provider store.
///
/// Only cells keyed by `retired` are removed; values in other scopes, including
/// world-scoped aggregates, remain owned by the store. Channel implementations
/// delegate validation, metering and possible last-owner drops to
/// [`Kernel::reclaim_channel_scope`], including [`Capability::drop_work`].
///
/// Each channel is atomic on refusal or cancellation. A [`Cons`] visits its head
/// before its tail, so an error can leave earlier children reclaimed. Retrying
/// the same retired scope is safe: successful calls count only cells removed by
/// that call, and already reclaimed children contribute zero. Count overflow
/// returns [`KernelError::CostOverflow`] after both children have succeeded.
///
/// [`Nil`] checks cancellation without reserving work or validating the scope,
/// then returns zero. It owns no cells or scope handles to validate.
pub trait ReclaimScope {
    /// Reclaim owned cells for `retired`, preserving the meter's typed failure.
    fn reclaim_scope<J, M: Meter>(
        &mut self,
        kernel: &mut Kernel<J>,
        retired: Scope,
        meter: &mut M,
    ) -> Result<usize, KernelError<M::Stop>>;
}

impl ReclaimScope for Nil {
    fn reclaim_scope<J, M: Meter>(
        &mut self,
        _: &mut Kernel<J>,
        _: Scope,
        meter: &mut M,
    ) -> Result<usize, KernelError<M::Stop>> {
        meter.checkpoint().map_err(KernelError::Stopped)?;
        Ok(0)
    }
}

impl<Q, P> ReclaimScope for Channel<Q, P>
where
    Q: Capability,
    P: PublicationPolicy<Value = Q::Value>,
{
    fn reclaim_scope<J, M: Meter>(
        &mut self,
        kernel: &mut Kernel<J>,
        retired: Scope,
        meter: &mut M,
    ) -> Result<usize, KernelError<M::Stop>> {
        kernel.reclaim_channel_scope(self, retired, meter)
    }
}

impl<H: ReclaimScope, T: ReclaimScope> ReclaimScope for Cons<H, T> {
    fn reclaim_scope<J, M: Meter>(
        &mut self,
        kernel: &mut Kernel<J>,
        retired: Scope,
        meter: &mut M,
    ) -> Result<usize, KernelError<M::Stop>> {
        let head = self.head.reclaim_scope(kernel, retired, meter)?;
        let tail = self.tail.reclaim_scope(kernel, retired, meter)?;
        head.checked_add(tail).ok_or(KernelError::CostOverflow)
    }
}
