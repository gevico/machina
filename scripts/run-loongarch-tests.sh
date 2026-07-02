#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"

# By default run the committed LoongArch smoke binaries under
# tests/firmware/. Point LOONGARCH_TESTS_DIR / LOONGARCH_TESTS_GLOB at an
# external corpus (e.g. built with a LoongArch cross-toolchain) to extend
# coverage; every binary just needs to terminate the run via the
# test-finisher MMIO register (write 0x5555 -> pass, 0x3333 -> fail).
LOONGARCH_TESTS_DIR="${LOONGARCH_TESTS_DIR:-${REPO_ROOT}/tests/firmware}"
LOONGARCH_TESTS_GLOB="${LOONGARCH_TESTS_GLOB:-loongarch_*.bin}"
TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-8}"
MACHINA_BIN="${MACHINA_BIN:-${REPO_ROOT}/target/release/machina}"
ARTIFACT_DIR="${ARTIFACT_DIR:-${REPO_ROOT}/target/loongarch-tests}"

PASS_FILE="${ARTIFACT_DIR}/pass.txt"
FAIL_FILE="${ARTIFACT_DIR}/fail.txt"
TIMEOUT_FILE="${ARTIFACT_DIR}/timeout.txt"
SUMMARY_FILE="${ARTIFACT_DIR}/summary.txt"

build_machina() {
    cargo build -p machina-emu --release
}

collect_tests() {
    find "${LOONGARCH_TESTS_DIR}" -maxdepth 1 -type f \
        -name "${LOONGARCH_TESTS_GLOB}" -printf "%f\n" | sort
}

run_tests() {
    mkdir -p "${ARTIFACT_DIR}"
    : > "${PASS_FILE}"
    : > "${FAIL_FILE}"
    : > "${TIMEOUT_FILE}"

    mapfile -t tests < <(collect_tests)
    if [ "${#tests[@]}" -eq 0 ]; then
        echo "no loongarch test binaries under ${LOONGARCH_TESTS_DIR}" >&2
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
                -M loongarch64-ref \
                -m 128 \
                -kernel "${LOONGARCH_TESTS_DIR}/${test_name}" \
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
            code="$(printf '%s\n' "${output}" \
                | grep -oE 'fail \(code 0x[0-9a-f]+\)' | tail -n1 || true)"
            [ -n "${code}" ] || code="exit:${status}"
            printf '%s\t%s\n' "${test_name}" "${code}" >> "${FAIL_FILE}"
            # Surface the offending binary and a log tail to aid triage.
            echo "FAILED ${test_name} (${code})" >&2
            printf '%s\n' "${output}" | tail -n 20 >&2
            bad=$((bad + 1))
        fi

        unset status
    done

    {
        echo "loongarch tests dir: ${LOONGARCH_TESTS_DIR}"
        echo "machina bin: ${MACHINA_BIN}"
        echo "timeout seconds: ${TIMEOUT_SECONDS}"
        echo "summary total=${total} ok=${ok} fail=${bad} timeout=${tout}"
    } | tee "${SUMMARY_FILE}"

    if [ "${bad}" -ne 0 ] || [ "${tout}" -ne 0 ]; then
        return 1
    fi
}

main() {
    cd "${REPO_ROOT}"
    build_machina
    run_tests
}

main "$@"
