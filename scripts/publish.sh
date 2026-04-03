#!/usr/bin/env bash
# Publish all machina library crates to crates.io in dependency order.

set -euo pipefail

PROG="$(basename "$0")"

usage() {
    cat <<EOF
Usage: $PROG [OPTIONS]

Publish all machina library crates to crates.io in dependency order.

Options:
  -d, --dry-run      Validate packages without uploading
  -a, --allow-dirty  Allow publishing with uncommitted changes
  -h, --help         Show this help message

Environment:
  SLEEP_SECS   Base seconds between publishes (default: 160)
  MAX_RETRIES  Max retry attempts per crate (default: 4)

Examples:
  $PROG                        # publish all crates
  $PROG --dry-run              # dry-run validation
  $PROG -d -a                  # dry-run with uncommitted changes
  SLEEP_SECS=1 $PROG -d -a      # fast dry-run (skip rate limit)
EOF
}

SLEEP_SECS="${SLEEP_SECS:-160}"
MAX_RETRIES="${MAX_RETRIES:-4}"
REGISTRY="--registry crates-io"
DRY_RUN=""
ALLOW_DIRTY=""
NO_VERIFY=""

# Temporarily disable source replacement (e.g. aliyun mirror)
# so that dependency verification during packaging resolves
# directly from crates.io. Without this, just-published
# workspace crates may not be found due to mirror sync delay.
CARGO_CONFIG="${HOME}/.cargo/config.toml"
disable_mirror() {
    if [ -f "$CARGO_CONFIG" ] && \
       grep -q '^replace-with' "$CARGO_CONFIG"; then
        sed -i 's/^replace-with/#&/' "$CARGO_CONFIG"
        echo "Disabled source mirror in ${CARGO_CONFIG}"
    fi
}
restore_mirror() {
    if [ -f "$CARGO_CONFIG" ] && \
       grep -q '^#replace-with' "$CARGO_CONFIG"; then
        sed -i 's/^#\(replace-with\)/\1/' "$CARGO_CONFIG"
        echo "Restored source mirror in ${CARGO_CONFIG}"
    fi
}
trap restore_mirror EXIT
disable_mirror

while [[ $# -gt 0 ]]; do
    case "$1" in
        -d|--dry-run)      DRY_RUN="--dry-run"; shift ;;
        -a|--allow-dirty)  ALLOW_DIRTY="--allow-dirty"; shift ;;
        -n|--no-verify)    NO_VERIFY="--no-verify"; shift ;;
        -h|--help)         usage; exit 0 ;;
        *)                 echo "unknown argument: $1"; usage; exit 1 ;;
    esac
done

# Publish order follows the dependency DAG (leaf -> root).
CRATES=(
    machina-core
    machina-decode
    machina-disas
    machina-util
    machina-difftest
    machina-memory
    machina-hw-core
    machina-monitor
    machina-accel
    machina-hw-char
    machina-hw-intc
    machina-hw-virtio
    machina-softfloat
    machina-guest-riscv
    machina-system
    machina-hw-riscv
    machina-emu
)

echo "Publishing ${#CRATES[@]} crates (sleep=${SLEEP_SECS}s between each)..."
echo

ok=0
fail=0

for crate in "${CRATES[@]}"; do
    echo ">>> Publishing ${crate} ..."
    published=false
    for attempt in $(seq 1 "$MAX_RETRIES"); do
        rc=0
        output=$(cargo publish -p "${crate}" \
            ${REGISTRY} ${DRY_RUN} ${ALLOW_DIRTY} \
            ${NO_VERIFY} 2>&1) || rc=$?
        if [ $rc -eq 0 ]; then
            echo "    ${crate} OK"
            ok=$((ok + 1)); published=true; break
        elif echo "$output" | grep -q "already exists"
        then
            echo "    ${crate} SKIPPED (already published)"
            ok=$((ok + 1)); published=skipped; break
        fi
        # Exponential backoff: SLEEP_SECS * attempt
        delay=$((SLEEP_SECS * attempt))
        echo "$output" | tail -3
        echo "    ${crate} FAILED [${attempt}/${MAX_RETRIES}]" \
             "(retry in ${delay}s...)"
        sleep "$delay"
    done
    if [ "$published" = false ]; then
        echo "    ${crate} FAILED (all retries exhausted)"
        fail=$((fail + 1))
    fi
    # Only sleep for rate limit if we actually uploaded.
    if [ "$published" = true ]; then
        sleep "${SLEEP_SECS}"
    fi
done

echo
echo "Done: ${ok} succeeded, ${fail} failed out of ${#CRATES[@]} crates."
exit $fail
