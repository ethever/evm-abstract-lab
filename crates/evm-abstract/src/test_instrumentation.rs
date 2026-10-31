//! Execution test instrumentation uses the shared scoped counter implementation.

pub(crate) use evm_resource::test_support::{Counter, Scope};

#[path = "../tests/support/work_boundary.rs"]
pub(crate) mod work_boundary;
