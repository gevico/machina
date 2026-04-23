#!/usr/bin/env bash
#
# rCore-Tutorial ch1-ch8 base smoke test under Machina.
#
# Runs each chapter's kernel through Machina, validates
# output with tg-rcore-tutorial-checker, and reports
# pass/fail per chapter.
#
# Usage:
#   ./scripts/run-rcore-tutorial-smoke.sh
#   RCORE_DIR=../tg-rcore-tutorial ./scripts/run-rcore-tutorial-smoke.sh

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"

MACHINA_BIN="${MACHINA_BIN:-${REPO_ROOT}/target/release/machina}"
# On Windows (MSYS2) cargo produces machina.exe; resolve the real path.
if [ ! -x "${MACHINA_BIN}" ] && [ -x "${MACHINA_BIN}.exe" ]; then
    MACHINA_BIN="${MACHINA_BIN}.exe"
fi
RCORE_DIR="${RCORE_DIR:-${REPO_ROOT}/../tg-rcore-tutorial}"
LOG_DIR="${REPO_ROOT}/target/rcore-smoke"

# Chapter-specific time budgets (seconds).
# ch5-ch8 run many usertests and need generous
# budgets, especially on debug builds.
declare -A TIMEOUTS=(
    [1]=30  [2]=30  [3]=60  [4]=60
    [5]=180 [6]=300 [7]=600 [8]=600
)

mkdir -p "${LOG_DIR}"

if [ ! -x "${MACHINA_BIN}" ]; then
    echo "error: machina binary not found: ${MACHINA_BIN}"
    echo "       run: cargo build --release"
    exit 1
fi

if ! command -v tg-rcore-tutorial-checker &>/dev/null; then
    echo "error: tg-rcore-tutorial-checker not found"
    echo "       run: cargo install tg-rcore-tutorial-checker"
    exit 1
fi

ok=0
bad=0
total=8

run_ch() {
    local ch="$1"
    local ch_dir="${RCORE_DIR}/tg-rcore-tutorial-ch${ch}"
    local log="${LOG_DIR}/ch${ch}.log"
    local timeout_s="${TIMEOUTS[$ch]}"

    if [ ! -d "${ch_dir}" ]; then
        echo "SKIP ch${ch}: directory not found"
        return 1
    fi

    echo -n "ch${ch} ... "

    # Build the kernel. ch5-ch8 need CHAPTER env var.
    # ch8 needs release mode for acceptable performance.
    local build_env=()
    local build_flags=()
    local profile="debug"
    if [ "${ch}" -ge 5 ]; then
        build_env=(env "CHAPTER=-${ch}")
    fi
    if [ "${ch}" -eq 8 ]; then
        build_flags=(--release)
        profile="release"
    fi

    if ! ( cd "${ch_dir}" && "${build_env[@]}" \
        cargo build ${build_flags[@]+"${build_flags[@]}"} 2>"${log}.build" ); then
        echo "FAIL (build)"
        cat "${log}.build" >> "${log}"
        return 1
    fi

    # Locate the kernel ELF.
    local kernel
    kernel="$(find "${ch_dir}/target/riscv64gc-unknown-none-elf/${profile}" \
        -maxdepth 1 -name "tg-rcore-tutorial-ch${ch}" \
        -type f ! -name '*.d' ! -name '*.rmeta' \
        | head -1)"
    if [ -z "${kernel}" ]; then
        echo "FAIL (kernel not found)"
        return 1
    fi

    # Build Machina command.
    local machina_args=(
        "${MACHINA_BIN}"
        -M riscv64-ref
        -nographic
        -bios none
    )

    # ch6-ch8: attach VirtIO block device.
    if [ "${ch}" -ge 6 ]; then
        # build.rs always puts fs.img in debug/ regardless of profile
        local fs_img="${ch_dir}/target/riscv64gc-unknown-none-elf/debug/fs.img"
        if [ ! -f "${fs_img}" ]; then
            echo "FAIL (fs.img not found: ${fs_img})"
            return 1
        fi
        machina_args+=(
            -drive "file=${fs_img},if=none,format=raw,id=x0"
            -device "virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0"
        )
    fi

    machina_args+=(-kernel "${kernel}")

    # Run Machina and capture output.
    local st=0
    timeout "${timeout_s}s" "${machina_args[@]}" \
        >"${log}" 2>&1 || st=$?
    if [ "${st}" -ne 0 ]; then
        if [ "${st}" -eq 124 ]; then
            echo "FAIL (timeout ${timeout_s}s)"
        else
            echo "FAIL (machina exit ${st})"
        fi
        return 1
    fi

    # Validate output.
    if [ "${ch}" -eq 1 ]; then
        # ch1: simple string match.
        if grep -q "Hello, world!" "${log}"; then
            echo "ok"
            return 0
        else
            echo "FAIL (no 'Hello, world!')"
            return 1
        fi
    else
        # ch2-ch8: use the checker.
        if tg-rcore-tutorial-checker --ch "${ch}" < "${log}" \
            >"${log}.checker" 2>&1; then
            echo "ok"
            return 0
        else
            echo "FAIL (checker)"
            cat "${log}.checker"
            return 1
        fi
    fi
}

echo "=== rCore-Tutorial smoke test (Machina) ==="
echo "machina: ${MACHINA_BIN}"
echo "rcore:   ${RCORE_DIR}"
echo ""

for ch in $(seq 1 8); do
    if run_ch "${ch}"; then
        ok=$((ok + 1))
    else
        bad=$((bad + 1))
    fi
done

echo ""
echo "summary: ${ok}/${total} passed, ${bad} failed"

if [ "${bad}" -gt 0 ]; then
    echo "logs: ${LOG_DIR}/"
    exit 1
fi
