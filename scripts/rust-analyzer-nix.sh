#!/usr/bin/env bash
# The server and its Cargo children inherit this checkout's complete devShell.
set -euo pipefail
project_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
exec nix develop --no-update-lock-file "$project_root" --command rust-analyzer "$@"
