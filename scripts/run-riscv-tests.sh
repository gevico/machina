#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"

RISCV_TESTS_REPO="${RISCV_TESTS_REPO:-https://github.com/riscv-software-src/riscv-tests.git}"
RISCV_TESTS_REF="${RISCV_TESTS_REF:-}"
RISCV_TESTS_DIR="${RISCV_TESTS_DIR:-${REPO_ROOT}/../riscv-tests}"
RISCV_PREFIX="${RISCV_PREFIX:-riscv64-unknown-elf-}"
XLEN="${XLEN:-64}"
TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-8}"
MACHINA_BIN="${MACHINA_BIN:-${REPO_ROOT}/target/release/machina}"
ARTIFACT_DIR="${ARTIFACT_DIR:-${REPO_ROOT}/target/riscv-tests}"

PASS_FILE="${ARTIFACT_DIR}/pass.txt"
FAIL_FILE="${ARTIFACT_DIR}/fail.txt"
TIMEOUT_FILE="${ARTIFACT_DIR}/timeout.txt"
SUMMARY_FILE="${ARTIFACT_DIR}/summary.txt"

clone_riscv_tests() {
    if [ -d "${RISCV_TESTS_DIR}/.git" ] || [ -d "${RISCV_TESTS_DIR}/isa" ]; then
        return
    fi

    git clone --recursive "${RISCV_TESTS_REPO}" "${RISCV_TESTS_DIR}"
}

checkout_riscv_tests_ref() {
    if [ -z "${RISCV_TESTS_REF}" ]; then
        return
    fi

    git -C "${RISCV_TESTS_DIR}" fetch --depth 1 origin "${RISCV_TESTS_REF}"
    git -C "${RISCV_TESTS_DIR}" checkout --detach FETCH_HEAD
}

prepare_riscv_tests() {
    clone_riscv_tests
    git -C "${RISCV_TESTS_DIR}" submodule update --init --recursive
    checkout_riscv_tests_ref
}

build_riscv_tests() {
    # -k: keep going on errors. The -v (virtual memory)
    # variants may fail to link with riscv64-linux-gnu-gcc
    # due to GOT relocation issues.  We run whatever
    # tests compile successfully.
    make -C "${RISCV_TESTS_DIR}/isa" \
        XLEN="${XLEN}" \
        RISCV_PREFIX="${RISCV_PREFIX}" \
        -k -j"$(nproc)" || true
}

build_machina() {
    cargo build -p machina-emu --release
}

collect_tests() {
    find "${RISCV_TESTS_DIR}/isa" -maxdepth 1 -type f -name "rv${XLEN}*" \
        ! -name "*.dump" \
        ! -name "*.hex" \
        ! -name "*.itb" \
        ! -name "*.map" \
        ! -name "*.objdump" \
        ! -name "*.readelf" \
        -printf "%f\n" | sort
}

run_tests() {
    mkdir -p "${ARTIFACT_DIR}"
    : > "${PASS_FILE}"
    : > "${FAIL_FILE}"
    : > "${TIMEOUT_FILE}"

    mapfile -t tests < <(collect_tests)
    if [ "${#tests[@]}" -eq 0 ]; then
        echo "no riscv-tests binaries found under ${RISCV_TESTS_DIR}/isa" >&2
        return 1
    fi

    local total=0
    local ok=0
    local bad=0
    local tout=0
    local test_name

    for test_name in "${tests[@]}"; do
        total=$((total + 1))
        echo "==> ${test_name}"

        local output
        local status
        output="$(
            timeout "${TIMEOUT_SECONDS}s" \
                "${MACHINA_BIN}" \
                -M riscv64-ref \
                -m 128 \
                -bios none \
                -kernel "${RISCV_TESTS_DIR}/isa/${test_name}" \
                -nographic 2>&1
        )" || status=$?
        status="${status:-0}"

        if [ "${status}" -eq 0 ]; then
            echo "${test_name}" >> "${PASS_FILE}"
            ok=$((ok + 1))
        elif [ "${status}" -eq 124 ]; then
            echo "${test_name}" >> "${TIMEOUT_FILE}"
            tout=$((tout + 1))
        else
            local code
            code="$(printf '%s\n' "${output}" | grep -oE 'fail \(code 0x[0-9a-f]+\)' | tail -n1 || true)"
            [ -n "${code}" ] || code="exit:${status}"
            printf '%s\t%s\n' "${test_name}" "${code}" >> "${FAIL_FILE}"
            bad=$((bad + 1))
        fi

        unset status
        if [ $((total % 50)) -eq 0 ]; then
            echo "progress total=${total} ok=${ok} fail=${bad} timeout=${tout}"
        fi
    done

    {
        echo "riscv-tests dir: ${RISCV_TESTS_DIR}"
        echo "riscv-tests repo: ${RISCV_TESTS_REPO}"
        echo "riscv-tests ref: $(git -C "${RISCV_TESTS_DIR}" rev-parse HEAD)"
        echo "machina bin: ${MACHINA_BIN}"
        echo "cross prefix: ${RISCV_PREFIX}"
        echo "timeout seconds: ${TIMEOUT_SECONDS}"
        echo "summary total=${total} ok=${ok} fail=${bad} timeout=${tout}"
    } | tee "${SUMMARY_FILE}"

    echo "failures file: ${FAIL_FILE}"
    if [ -s "${FAIL_FILE}" ]; then
        cat "${FAIL_FILE}"
    else
        echo "(no failed tests)"
    fi

    if [ "${bad}" -ne 0 ] || [ "${tout}" -ne 0 ]; then
        return 1
    fi
}

main() {
    cd "${REPO_ROOT}"
    prepare_riscv_tests
    build_riscv_tests
    build_machina
    run_tests
}

main "$@"
