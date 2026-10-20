//! Client-owned vocabulary shared by independent producer/consumer modules.
use evm_abstract::{
    analysis::cooperation::ExtraStores,
    kernel::{
        Capability, Channel, Cons, Descending, Here, Kernel, KernelError, Meter, Nil, RefinePolicy,
        There,
    },
};
use std::{collections::BTreeSet, convert::Infallible};

/// Bottom-based active-stack slots proved zero in this exact Subject.
/// An empty set is a ready answer with no guarantee, distinct from Pending.
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

// The EVM adapter owns routes 0 (Inputs) and 1 (Refinements).
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
        // More proved-zero slots are a stronger conjunction of guarantees.
        Ok(a.union(b).copied().collect())
    }
    fn equivalent(&self, a: &Self::Value, b: &Self::Value) -> bool {
        a == b
    }
}

pub struct Stores;
impl ExtraStores for Stores {
    type Stores = Cons<Channel<ZeroGuards, Descending<Conjoin>>, Nil>;
    fn create<J, M: Meter>(
        &self,
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self::Stores, KernelError<M::Stop>> {
        // Called afresh for each input generation, on the original root ledger.
        Ok(Cons::new(kernel.channel(Descending(Conjoin), meter)?, Nil))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Failure<K, P> {
    #[error("capability operation failed: {0}")]
    Infrastructure(K),
    #[error("publication failed: {0}")]
    Publication(P),
    #[error("immutable input belongs to another Subject")]
    WrongSubject,
}
