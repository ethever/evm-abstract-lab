use analysis_kernel::{Capability, JoinPolicy, Kernel, Scope, ScopeKey};
use std::convert::Infallible;

pub use crate::meter::{Budget, Stop};

pub fn scope<J>(kernel: &mut Kernel<J>, budget: &mut Budget, generation: u64) -> Scope {
    kernel
        .scope(
            ScopeKey {
                context: 3,
                generation,
                policy: 1,
            },
            budget,
        )
        .unwrap()
}

pub struct Numbers;
impl Capability for Numbers {
    type Key = u8;
    type Value = u32;
    fn key_work(_: &u8) -> usize {
        1
    }
    fn value_work(_: &u32) -> usize {
        1
    }
}
#[derive(Clone, Copy)]
pub struct Maximum;
impl JoinPolicy for Maximum {
    type Value = u32;
    type Error = Infallible;
    fn work(&self, _: &u32, _: &u32) -> usize {
        3
    }
    fn join(&self, a: &u32, b: &u32) -> Result<u32, Infallible> {
        Ok((*a).max(*b))
    }
    fn equivalent(&self, a: &u32, b: &u32) -> bool {
        a == b
    }
}
