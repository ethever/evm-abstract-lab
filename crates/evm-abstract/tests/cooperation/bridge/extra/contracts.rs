//! Private client vocabulary: these types do not exist in the interpreter API.
use analysis_kernel::{
    Capability, Channel, Cons, Descending, Epoch, Here, Kernel, KernelError, Meter, Nil,
    RefinePolicy, There,
};
use evm_abstract::analysis::cooperation::ExtraStores;
use std::{cell::Cell, collections::BTreeSet, convert::Infallible};

/// Bottom-based stack positions proved equal to zero under this Subject's
/// immutable assumptions. Empty is a ready empty guarantee, never Pending.
pub struct ZeroGuards;
impl Capability for ZeroGuards {
    type Key = ();
    type Value = BTreeSet<usize>;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(value: &Self::Value) -> usize {
        value.len().saturating_add(1)
    }
}
pub type ProofRoute = There<There<Here>>;
pub struct Conjoin;
impl RefinePolicy for Conjoin {
    type Value = BTreeSet<usize>;
    type Error = Infallible;
    fn work(&self, a: &Self::Value, b: &Self::Value) -> usize {
        a.len()
            .saturating_add(b.len())
            .saturating_add(1)
            .saturating_pow(2)
            .saturating_mul(8)
    }
    fn refine(&self, a: &Self::Value, b: &Self::Value) -> Result<Self::Value, Infallible> {
        Ok(a.union(b).copied().collect())
    }
    fn equivalent(&self, a: &Self::Value, b: &Self::Value) -> bool {
        a == b
    }
}
#[derive(Default)]
pub struct Stores {
    pub created: Cell<usize>,
    pub epoch: Cell<Option<Epoch>>,
}
impl ExtraStores for Stores {
    type Stores = Cons<Channel<ZeroGuards, Descending<Conjoin>>, Nil>;
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        meter.reserve(2).map_err(KernelError::Stopped)?;
        self.created.set(self.created.get().saturating_add(1));
        if let Some(epoch) = self.epoch.get() {
            assert_eq!(epoch, kernel.epoch());
        }
        self.epoch.set(Some(kernel.epoch()));
        Ok(Cons::new(kernel.channel(Descending(Conjoin), meter)?, Nil))
    }
}
