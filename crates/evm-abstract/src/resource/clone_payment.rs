//! Shared payment primitives and cached numeric-value copy quotation.

pub(crate) use evm_domain::payment::value_work;
pub(crate) use evm_resource::{checkpoint, finish_quote, pay};

#[cfg(test)]
pub(crate) mod tests;
