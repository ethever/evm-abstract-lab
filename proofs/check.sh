#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 1 || $1 != /* ]]; then
  echo "usage: bash proofs/check.sh ABSOLUTE_OUTPUT_DIRECTORY" >&2
  exit 2
fi

proof_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
output_dir=$1
lean_bin=${LEAN_BIN:-lean}
version=$("$lean_bin" --version)
if [[ $version != 'Lean (version 4.30.0,'* ]]; then
  echo "expected Lean 4.30.0; got: $version" >&2
  exit 1
fi

mkdir -p "$output_dir/olean/NoSmt"
printf '%s\n' "$version" > "$output_dir/checker-version.txt"
export LEAN_PATH="$output_dir/olean"

# Std is supplied by the pinned checker. All project imports are compiled here,
# before the intentional rejection; no Lake resolver or preexisting oleans run.
modules=(PartialFacts Facts ConditionalFacts Snapshots ReturnLengths NumericCarrier ByteProduct ExactIte Effects Lifecycle RootEnvelope RootCoverageCapability Logs Engine LateFailure)
for module in "${modules[@]}"; do
  source="$proof_root/NoSmt/$module.lean"
  "$lean_bin" --root="$proof_root" -DwarningAsError=true \
    -o "$output_dir/olean/NoSmt/$module.olean" "$source" \
    > "$output_dir/$module.log" 2>&1 || {
      status=$?
      cat "$output_dir/$module.log"
      exit "$status"
    }
  cat "$output_dir/$module.log"
done

if "$lean_bin" --root="$proof_root" -DwarningAsError=true \
  "$proof_root/counterexamples/OldJoinRejected.lean" \
  > "$output_dir/OldJoinRejected.log" 2>&1; then
  echo "the old unsound join witness unexpectedly checked" >&2
  exit 1
else
  rejected_exit=$?
fi

# Ignore ambient PYTHONOPTIMIZE/PYTHONPATH so the rejection assertions remain on.
python3 -I "$proof_root/audit.py" "$proof_root" "$output_dir" "$rejected_exit"
