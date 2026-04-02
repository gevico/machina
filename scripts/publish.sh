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
  SLEEP_SECS  Seconds to wait between publishes (default: 1)

Examples:
  $PROG                        # publish all crates
  $PROG --dry-run              # dry-run validation
  $PROG -d -a                  # dry-run with uncommitted changes
  SLEEP_SECS=5 $PROG           # wait 5 seconds between publishes
EOF
}

SLEEP_SECS="${SLEEP_SECS:-1}"
REGISTRY="--registry crates-io"
DRY_RUN=""
ALLOW_DIRTY=""
NO_VERIFY=""

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
    # Skip already-published crates.
    output=$(cargo publish -p "${crate}" ${REGISTRY} ${DRY_RUN} ${ALLOW_DIRTY} ${NO_VERIFY} 2>&1)
    rc=$?
    if [ $rc -eq 0 ]; then
        echo "    ${crate} OK"
        ok=$((ok + 1))
    elif echo "$output" | grep -q "already exists"; then
        echo "    ${crate} SKIPPED (already published)"
        ok=$((ok + 1))
    else
        echo "$output" | tail -3
        echo "    ${crate} FAILED (retrying in ${SLEEP_SECS}s...)"
        sleep "${SLEEP_SECS}"
        if cargo publish -p "${crate}" ${REGISTRY} ${DRY_RUN} ${ALLOW_DIRTY} ${NO_VERIFY}; then
            echo "    ${crate} OK (retry)"
            ok=$((ok + 1))
        else
            echo "    ${crate} FAILED"
            fail=$((fail + 1))
        fi
    fi
    sleep "${SLEEP_SECS}"
done

echo
echo "Done: ${ok} succeeded, ${fail} failed out of ${#CRATES[@]} crates."
exit $fail
