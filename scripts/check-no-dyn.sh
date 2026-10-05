#!/usr/bin/env bash
# Check every workspace target, then compile and run doctests with the same lint.
set -euo pipefail

task_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$task_root"
task_toolchain=${DYLINT_TOOLCHAIN:-nightly-2026-08-20}
task_cpu_limit=$(( $(nproc) - 5 ))
if (( task_cpu_limit < 1 )); then
  task_cpu_limit=1
fi
task_default_jobs=$task_cpu_limit
if (( task_default_jobs > 8 )); then
  task_default_jobs=8
fi
task_jobs=${DYLINT_JOBS:-$task_default_jobs}
python3 - "$task_jobs" "$task_cpu_limit" <<'PY'
import sys

try:
    jobs, limit = map(int, sys.argv[1:])
    if not 1 <= jobs <= limit:
        raise ValueError(f"DYLINT_JOBS must be between 1 and {limit}")
except ValueError as error:
    sys.exit(str(error))
PY
export CARGO_BUILD_JOBS=$task_jobs

if [[ -n ${DYLINT_DRIVER:-} || -n ${DYLINT_LIBRARY:-} ]]; then
  if [[ -z ${DYLINT_DRIVER:-} || -z ${DYLINT_LIBRARY:-} ]]; then
    echo 'Set both DYLINT_DRIVER and DYLINT_LIBRARY for prebuilt lint artifacts.' >&2
    exit 1
  fi
  if [[ ! -x "$DYLINT_DRIVER" || ! -f "$DYLINT_LIBRARY" ]]; then
    echo 'The Dylint driver must be executable and the no_dyn library must exist.' >&2
    exit 1
  fi
  export DYLINT_DRIVER_PATH
  DYLINT_DRIVER_PATH=$(dirname -- "$(dirname -- "$DYLINT_DRIVER")")
  cargo dylint --all --workspace --fail-on-no-libraries --no-build --no-metadata \
    --lib-path "$DYLINT_LIBRARY" -- --locked --all-targets --all-features --jobs "$task_jobs"
else
  cargo dylint --all --workspace --fail-on-no-libraries -- \
    --locked --all-targets --all-features --jobs "$task_jobs"
  task_host=$(rustc "+$task_toolchain" -vV | sed -n 's/^host: //p')
  if [[ -z "$task_host" ]]; then
    echo 'Could not determine the pinned lint toolchain host.' >&2
    exit 1
  fi
  task_full_toolchain="$task_toolchain-$task_host"
  task_target=$(cargo metadata --no-deps --format-version 1 | \
    python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
  case $(uname -s) in
    Linux) task_library_suffix=so ;;
    Darwin) task_library_suffix=dylib ;;
    *) echo 'Set explicit DYLINT_DRIVER and DYLINT_LIBRARY on this platform.' >&2; exit 1 ;;
  esac
  export DYLINT_DRIVER="${DYLINT_DRIVER_PATH:-$HOME/.dylint_drivers}/$task_full_toolchain/dylint-driver"
  export DYLINT_LIBRARY="$task_target/dylint/libraries/$task_full_toolchain/release/libno_dyn@$task_full_toolchain.$task_library_suffix"
fi

export DYLINT_TOOLCHAIN=$task_toolchain
"$task_root/scripts/test-no-dyn-doctests.sh" --all-features --jobs "$task_jobs"
