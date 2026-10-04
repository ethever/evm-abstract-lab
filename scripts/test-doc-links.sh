#!/usr/bin/env bash
# Exercise the configured checker, including links missed by the old regex.
set -euo pipefail

config=$(realpath "$1")
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
cd "$fixture"
mkdir -p docs target .github

cat > 'docs/with space.md' <<'EOF'
# Target heading
EOF
cat > README.md <<'EOF'
# Local heading

[relative](docs/with%20space.md#target-heading)
[root-relative](/docs/with%20space.md#target-heading)
[self](#local-heading)
[reference][target]
[offline only](https://unreachable.invalid/)

[target]: <docs/with space.md#target-heading>
EOF
cat > .github/guide.md <<'EOF'
[reference][root]

[root]: ../README.md#local-heading
EOF
cat > target/generated.md <<'EOF'
[generated](missing.md)
EOF
cp README.md valid.input
cp .github/guide.md hidden.input

check() {
    lychee --config "$config" --offline --root-dir "$fixture" -- README.md '**/*.md'
}

check
for kind in relative reference anchor hidden excluded-target; do
    case "$kind" in
        relative) printf '\n[broken](missing.md)\n' >> README.md ;;
        reference) printf '\n[broken][missing]\n\n[missing]: missing.md\n' >> README.md ;;
        anchor) printf '\n[broken](docs/with%%20space.md#missing-heading)\n' >> README.md ;;
        hidden) printf '\n[broken](missing.md)\n' >> .github/guide.md ;;
        excluded-target) printf '\n[broken](target/missing.md)\n' >> README.md ;;
    esac
    if check > failure.log 2>&1; then
        echo "checker accepted broken $kind link" >&2
        exit 1
    else
        status=$?
        if [ "$status" -ne 2 ]; then
            cat failure.log >&2
            echo "expected link failure (2), got $status" >&2
            exit 1
        fi
    fi
    # Each failure must be caused by this case, not a previous broken link.
    cp valid.input README.md
    cp hidden.input .github/guide.md
    check
done
echo 'Configured lychee accepts valid local links and rejects missing files/anchors.'
