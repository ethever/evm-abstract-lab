//! Kernel-owned regressions; external acceptance tests have separate ownership.

#[path = "../tests/support/analyzer.rs"]
mod analyzer;
mod entry_lifecycle;
mod generations;
mod invalidation;
mod refinement;
mod reporting;
mod snapshot;
mod suspension;

use crate::{Kernel, Scope, ScopeKey};
#[path = "../tests/support/meter.rs"]
mod meter;
use analyzer::Noop;
use meter::{Budget, Stop};

fn scope<J>(kernel: &mut Kernel<J>, meter: &mut Budget, context: u64, generation: u64) -> Scope {
    kernel
        .scope(
            ScopeKey {
                context,
                generation,
                policy: 1,
            },
            meter,
        )
        .unwrap()
}
