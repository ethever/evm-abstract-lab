# Register during prePatch: stdenv snapshots postPatchHooks before running the
# postPatch body. Crane's already-registered injection then runs before cleanup.
''
  postPatchHooks+=(clearCargoConcurrencyLimits)
  clearCargoConcurrencyLimits() { unset CARGO_BUILD_JOBS RUST_TEST_THREADS; }
''
