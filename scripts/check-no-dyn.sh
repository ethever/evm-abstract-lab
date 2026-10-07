#!/usr/bin/env bash
# Keep this convenience entry point; Nix owns the toolchain and lint artifacts.
set -euo pipefail

task_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$task_root"
exec nix develop --no-update-lock-file --command no-dyn "$@"
