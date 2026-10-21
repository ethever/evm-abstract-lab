//! Statically composed cleanup of generation-owned channel cells.
//!
//! Atomicity is per channel. Earlier successful children remain reclaimed if a
//! later child refuses; propagate that error and retry the same retired scope.
//! World-scoped aggregates are retained when reclaiming a different local scope.

use crate::{
    Capability, Channel, Cons, Kernel, KernelError, Meter, Nil, Scope, algebra::PublicationPolicy,
    kernel::reserve,
};

/// Ownership seam for built-in and adapter-owned static provider stores.
pub trait ReclaimScope {
    /// Reclaim only this already-retired scope. Counts are checked; partial
    /// progress before a later child error is retained and safe to retry.
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
        _: &mut M,
    ) -> Result<usize, KernelError<M::Stop>> {
        Ok(0)
    }
}
impl<Q: Capability, P: PublicationPolicy<Value = Q::Value>> ReclaimScope for Channel<Q, P> {
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
        reserve(meter, 2)?;
        let head = self.head.reclaim_scope(kernel, retired, meter)?;
        let tail = self.tail.reclaim_scope(kernel, retired, meter)?;
        head.checked_add(tail).ok_or(KernelError::CostOverflow)
    }
}
