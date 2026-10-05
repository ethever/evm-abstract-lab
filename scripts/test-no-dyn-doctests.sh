#!/usr/bin/env bash
# Prove the doctest compiler loads no_dyn, then check the repository's doctests.
set -euo pipefail

task_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$task_root"
task_toolchain=${DYLINT_TOOLCHAIN:-nightly-2026-08-20}
if [[ -z ${DYLINT_DRIVER:-} || -z ${DYLINT_LIBRARY:-} ]]; then
  echo 'DYLINT_DRIVER and DYLINT_LIBRARY are required; run scripts/check-no-dyn.sh locally.' >&2
  exit 1
fi
if [[ ! -x "$DYLINT_DRIVER" || ! -f "$DYLINT_LIBRARY" ]]; then
  echo 'The Dylint driver must be executable and the no_dyn library must exist.' >&2
  exit 1
fi
export DYLINT_DRIVER
DYLINT_DRIVER=$(realpath -- "$DYLINT_DRIVER")

# Encode paths as JSON and rustdoc arguments separately, preserving spaces.
export DYLINT_LIBS
DYLINT_LIBS=$(python3 - "$DYLINT_LIBRARY" <<'PY'
import json
from pathlib import Path
import sys

print(json.dumps([str(Path(sys.argv[1]).resolve(strict=True))]))
PY
)
task_rustdoc_flags=()
if [[ -n ${CARGO_ENCODED_RUSTDOCFLAGS:-} ]]; then
  IFS=$'\x1f' read -r -a task_rustdoc_flags <<< "$CARGO_ENCODED_RUSTDOCFLAGS"
elif [[ -n ${RUSTDOCFLAGS:-} ]]; then
  read -r -a task_rustdoc_flags <<< "$RUSTDOCFLAGS"
fi
task_rustdoc_flags+=(
  -Z unstable-options --test-builder "$DYLINT_DRIVER" --doctest-build-arg=-Fno_dyn
)
printf -v CARGO_ENCODED_RUSTDOCFLAGS '%s\x1f' "${task_rustdoc_flags[@]}"
export CARGO_ENCODED_RUSTDOCFLAGS=${CARGO_ENCODED_RUSTDOCFLAGS%$'\x1f'}
unset RUSTDOCFLAGS
if [[ -n ${RUSTDOC:-} ]]; then
  task_rustdoc=("$RUSTDOC")
else
  task_rustdoc=(rustdoc "+$task_toolchain")
fi

task_fixture=$(mktemp -d)
trap 'rm -rf -- "$task_fixture"' EXIT
cat > "$task_fixture/accepted.rs" <<'EOF'
/// ```
/// let _sum = [1u8, 2, 3].into_iter().sum::<u8>();
/// ```
pub struct StaticDoctest;
EOF
cat > "$task_fixture/rejected.rs" <<'EOF'
/// ```
/// let value = 1u8;
/// let _object: &dyn std::fmt::Display = &value;
/// ```
pub struct DynamicDoctest;
EOF

if ! "${task_rustdoc[@]}" --test --edition=2024 "${task_rustdoc_flags[@]}" \
    "$task_fixture/accepted.rs" > "$task_fixture/accepted.log" 2>&1; then
  cat "$task_fixture/accepted.log" >&2
  echo 'The statically dispatched doctest control failed.' >&2
  exit 1
fi
if "${task_rustdoc[@]}" --test --edition=2024 "${task_rustdoc_flags[@]}" \
    "$task_fixture/rejected.rs" > "$task_fixture/rejected.log" 2>&1; then
  echo 'The doctest compiler accepted dynamic dispatch; no_dyn was not enforced.' >&2
  exit 1
fi
python3 - "$task_fixture/rejected.log" <<'PY'
from pathlib import Path
import sys

output = Path(sys.argv[1]).read_text()
if "dynamic dispatch is forbidden" not in output:
    sys.stderr.write(output)
    sys.exit("The rejected doctest failed without a no_dyn diagnostic.")
PY
echo 'Doctest controls passed: static dispatch compiles, dyn dispatch is rejected by no_dyn.'

cargo "+$task_toolchain" test --workspace --doc --locked "$@"
