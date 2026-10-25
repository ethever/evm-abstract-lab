# no_dyn

Repository-local Dylint `LateLintPass` forbidding trait objects and function pointers in the application workspace. It checks resolved types, signatures, inferred expressions and coercions, including macro expansions. Concrete function items, closures, generics and `impl Trait` remain supported.

Framework-required erased callbacks and futures are confined to the exact lexical modules `evm_abstract_web::framework` and `evm_abstract::world::rpc::framework`. Matching includes the crate identity and real definition parents; similarly named modules and implementations outside those adapters remain checked.

Serde's real `Deserialize` derive may implicitly coerce an error expectation to the dependency-owned `serde_core::de::Expected` only when directly calling its `Error::invalid_type`, `invalid_value` or `invalid_length` method. The lint checks compiler-resolved macro, trait and method definitions; user field types, aliases, callbacks, handwritten calls and lookalike macros remain forbidden. This exception does not cover a whole derive, application crate or `std::error::Error::source`.

See [setup, scope and error API changes](../../docs/no-dynamic-dispatch.md). From the repository root, run `cargo dylint --all --workspace -- --locked --all-targets --all-features`. In `nix develop`, run `no-dyn-ui` for the acceptance/rejection fixtures and `no-dyn` for the workspace and doctest gate.

The compiler and lint dependencies are pinned separately from the application workspace. This tool is excluded from that workspace because Dylint's own registration API requires a boxed dynamic lint pass. Compiler-generated Debug formatting and test/proc-macro callbacks are excluded; user-written test and macro code remains checked.
