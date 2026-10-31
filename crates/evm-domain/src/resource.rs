//! Internal adapters preserve the numeric domain's shared ledger types.

pub(crate) use evm_resource::{ResourceError as CloneError, WorkBudget};
pub(crate) mod clone_payment {
    pub(crate) use crate::payment::value_work;
    pub(crate) use evm_resource::{checkpoint, pay};
}
