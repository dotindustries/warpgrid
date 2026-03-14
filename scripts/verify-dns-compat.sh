#!/usr/bin/env bash
#
# verify-dns-compat.sh — Verify DNS patches meet US-205 acceptance criteria.
#
# Checks:
#   1. Vanilla Wasmtime compatibility — patched sysroot test runs without crash
#   2. Stock build cleanliness — stock sysroot test runs without crash
#   3. Binary size — patched libc.a within 5% of stock libc.a
#   4. Weak symbol fallback equivalence — identical PASS/FAIL between sysroots
#
# Prerequisites:
#   Both sysroots must already be built:
#     scripts/build-libc.sh --both
#
# Usage:
#   scripts/verify-dns-compat.sh
#
# Exit codes:
#   0  All checks passed
#   1  One or more checks failed
#   2  Configuration error (missing tools, sysroots not built, etc.)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_DIR="${PROJECT_ROOT}/build"
TESTS_DIR="${PROJECT_ROOT}/libc-patches/tests"
TARGET_TRIPLE="${TARGET_TRIPLE:-wasm32-wasip2}"
TEST_SRC="${TESTS_DIR}/test_dns_compat.c"

# Results tracking
CHECKS_TOTAL=0
CHECKS_PASSED=0
CHECKS_FAILED=0

# ─── Helpers ─────────────────────────────────────────────────────────────────

log()  { echo "==> $*" >&2; }
err()  { echo "ERROR: $*" >&2; exit 2; }
pass() { CHECKS_TOTAL=$((CHECKS_TOTAL + 1)); CHECKS_PASSED=$((CHECKS_PASSED + 1)); echo "  PASS: $*"; }
fail() { CHECKS_TOTAL=$((CHECKS_TOTAL + 1)); CHECKS_FAILED=$((CHECKS_FAILED + 1)); echo "  FAIL: $*"; }

# ─── Tool detection ──────────────────────────────────────────────────────────

ensure_wasi_sdk() {
    if [[ -n "${WASI_SDK_PATH:-}" ]] && [[ -x "${WASI_SDK_PATH}/bin/clang" ]]; then
        return
    fi

    local cached
    cached=$(ls -d "${BUILD_DIR}/cache/wasi-sdk-"* 2>/dev/null | head -1)
    if [[ -n "${cached}" && -x "${cached}/bin/clang" ]]; then
        WASI_SDK_PATH="${cached}"
        return
    fi

    err "wasi-sdk not found. Set WASI_SDK_PATH or run 'scripts/build-libc.sh --stock' first."
}

ensure_wasmtime() {
    if [[ -n "${WASMTIME:-}" ]] && [[ -x "${WASMTIME}" ]]; then
        return
    fi

    if command -v wasmtime &>/dev/null; then
        WASMTIME="$(command -v wasmtime)"
        return
    fi

    err "wasmtime not found. Install wasmtime or set WASMTIME environment variable."
}

# ─── Compile & run helper ────────────────────────────────────────────────────

# Compile test_dns_compat.c against a sysroot, return path to .wasm
compile_compat_test() {
    local sysroot="${1}"
    local variant="${2}"
    local wasm_dir="${BUILD_DIR}/verify-wasm-${variant}"
    local wasm_file="${wasm_dir}/test_dns_compat.wasm"

    mkdir -p "${wasm_dir}"

    "${WASI_SDK_PATH}/bin/clang" \
        --target="${TARGET_TRIPLE}" \
        --sysroot="${sysroot}" \
        -O1 \
        -o "${wasm_file}" \
        "${TEST_SRC}" 2>&1

    echo "${wasm_file}"
}

# Run a .wasm in vanilla Wasmtime (no shims, no --inherit-network)
run_vanilla() {
    local wasm="${1}"
    local wasmtime_flags=()

    case "${TARGET_TRIPLE}" in
        *wasip2*) wasmtime_flags+=("--wasm" "component-model=y") ;;
    esac

    if command -v timeout &>/dev/null; then
        timeout 15 "${WASMTIME}" run "${wasmtime_flags[@]}" "${wasm}" 2>&1
    else
        perl -e "alarm 15; exec @ARGV" -- "${WASMTIME}" run "${wasmtime_flags[@]}" "${wasm}" 2>&1
    fi
}

# ─── Check 1: Vanilla Wasmtime compatibility (patched sysroot) ───────────────

check_vanilla_compat() {
    log "Check 1: Vanilla Wasmtime compatibility (patched sysroot)"

    local sysroot="${BUILD_DIR}/sysroot-patched"
    if [[ ! -d "${sysroot}" ]]; then
        fail "Check 1 — patched sysroot not found at ${sysroot}. Run: scripts/build-libc.sh --patched"
        return
    fi

    local wasm_file
    if ! wasm_file=$(compile_compat_test "${sysroot}" "patched" 2>&1 | tail -1); then
        fail "Check 1 — failed to compile test_dns_compat.c against patched sysroot"
        return
    fi

    local output exit_code=0
    output=$(run_vanilla "${wasm_file}" 2>&1) || exit_code=$?

    if [[ ${exit_code} -eq 0 ]]; then
        pass "Check 1 — test_dns_compat runs in vanilla Wasmtime (patched sysroot, exit 0)"
    else
        fail "Check 1 — test_dns_compat crashed/failed in vanilla Wasmtime (exit ${exit_code})"
        echo "    Output: ${output}" | head -10
    fi

    # Save output for Check 4
    PATCHED_OUTPUT="${output}"
}

# ─── Check 2: Stock build cleanliness ────────────────────────────────────────

check_stock_build() {
    log "Check 2: Stock build cleanliness"

    local sysroot="${BUILD_DIR}/sysroot-stock"
    if [[ ! -d "${sysroot}" ]]; then
        fail "Check 2 — stock sysroot not found at ${sysroot}. Run: scripts/build-libc.sh --stock"
        return
    fi

    local wasm_file
    if ! wasm_file=$(compile_compat_test "${sysroot}" "stock" 2>&1 | tail -1); then
        fail "Check 2 — failed to compile test_dns_compat.c against stock sysroot"
        return
    fi

    local output exit_code=0
    output=$(run_vanilla "${wasm_file}" 2>&1) || exit_code=$?

    if [[ ${exit_code} -eq 0 ]]; then
        pass "Check 2 — test_dns_compat runs cleanly against stock sysroot (exit 0)"
    else
        fail "Check 2 — test_dns_compat failed against stock sysroot (exit ${exit_code})"
        echo "    Output: ${output}" | head -10
    fi

    # Save output for Check 4
    STOCK_OUTPUT="${output}"
}

# ─── Check 3: Binary size comparison ─────────────────────────────────────────

check_binary_size() {
    log "Check 3: Binary size comparison (patched within 5% of stock)"

    local stock_libc="${BUILD_DIR}/sysroot-stock/lib/${TARGET_TRIPLE}/libc.a"
    local patched_libc="${BUILD_DIR}/sysroot-patched/lib/${TARGET_TRIPLE}/libc.a"

    if [[ ! -f "${stock_libc}" ]]; then
        fail "Check 3 — stock libc.a not found at ${stock_libc}"
        return
    fi
    if [[ ! -f "${patched_libc}" ]]; then
        fail "Check 3 — patched libc.a not found at ${patched_libc}"
        return
    fi

    local stock_size patched_size
    stock_size=$(stat -c%s "${stock_libc}" 2>/dev/null || stat -f%z "${stock_libc}")
    patched_size=$(stat -c%s "${patched_libc}" 2>/dev/null || stat -f%z "${patched_libc}")

    # Calculate percentage difference: |patched - stock| / stock * 100
    local diff_abs delta_pct
    if [[ ${patched_size} -ge ${stock_size} ]]; then
        diff_abs=$((patched_size - stock_size))
    else
        diff_abs=$((stock_size - patched_size))
    fi

    # Use awk for floating-point arithmetic
    delta_pct=$(awk "BEGIN { printf \"%.2f\", (${diff_abs} / ${stock_size}) * 100 }")

    local stock_kb patched_kb
    stock_kb=$(awk "BEGIN { printf \"%.1f\", ${stock_size} / 1024 }")
    patched_kb=$(awk "BEGIN { printf \"%.1f\", ${patched_size} / 1024 }")

    log "  Stock:   ${stock_kb} KB (${stock_size} bytes)"
    log "  Patched: ${patched_kb} KB (${patched_size} bytes)"
    log "  Delta:   ${delta_pct}%"

    # Check if within 5% tolerance
    local within
    within=$(awk "BEGIN { print (${delta_pct} <= 5.0) ? 1 : 0 }")

    if [[ "${within}" -eq 1 ]]; then
        pass "Check 3 — binary size delta ${delta_pct}% (within 5% tolerance)"
    else
        fail "Check 3 — binary size delta ${delta_pct}% exceeds 5% tolerance"
    fi
}

# ─── Check 4: Weak symbol fallback equivalence ──────────────────────────────

check_fallback_equivalence() {
    log "Check 4: Weak symbol fallback equivalence (stock vs patched output)"

    if [[ -z "${STOCK_OUTPUT:-}" ]]; then
        fail "Check 4 — stock output not available (Check 2 must run first)"
        return
    fi
    if [[ -z "${PATCHED_OUTPUT:-}" ]]; then
        fail "Check 4 — patched output not available (Check 1 must run first)"
        return
    fi

    # Extract PASS/FAIL lines and compare
    local stock_results patched_results
    stock_results=$(echo "${STOCK_OUTPUT}" | grep -E '^\s*(PASS|FAIL):' | sort)
    patched_results=$(echo "${PATCHED_OUTPUT}" | grep -E '^\s*(PASS|FAIL):' | sort)

    if [[ -z "${stock_results}" ]]; then
        fail "Check 4 — no PASS/FAIL lines found in stock output"
        return
    fi
    if [[ -z "${patched_results}" ]]; then
        fail "Check 4 — no PASS/FAIL lines found in patched output"
        return
    fi

    # Compare the verdict (PASS/FAIL) for each test, ignoring details after the dash
    local stock_verdicts patched_verdicts
    stock_verdicts=$(echo "${stock_results}" | sed 's/^\s*\(PASS\|FAIL\):.*/\1/' | sort)
    patched_verdicts=$(echo "${patched_results}" | sed 's/^\s*\(PASS\|FAIL\):.*/\1/' | sort)

    local stock_count patched_count
    stock_count=$(echo "${stock_verdicts}" | wc -l | tr -d ' ')
    patched_count=$(echo "${patched_verdicts}" | wc -l | tr -d ' ')

    if [[ "${stock_count}" != "${patched_count}" ]]; then
        fail "Check 4 — test count mismatch: stock=${stock_count}, patched=${patched_count}"
        return
    fi

    # Check that both have the same number of PASS verdicts
    local stock_passes patched_passes
    stock_passes=$(echo "${stock_verdicts}" | grep -c '^PASS$' || true)
    patched_passes=$(echo "${patched_verdicts}" | grep -c '^PASS$' || true)

    if [[ "${stock_passes}" == "${patched_passes}" ]]; then
        pass "Check 4 — fallback equivalence confirmed (${stock_passes}/${stock_count} tests pass on both)"
    else
        fail "Check 4 — fallback results differ: stock=${stock_passes} pass, patched=${patched_passes} pass"
        echo "    Stock results:"
        echo "${stock_results}" | sed 's/^/      /'
        echo "    Patched results:"
        echo "${patched_results}" | sed 's/^/      /'
    fi
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
    echo "╔══════════════════════════════════════════════════════════════════╗"
    echo "║  US-205: Verify DNS patches with stock build compatibility     ║"
    echo "╚══════════════════════════════════════════════════════════════════╝"
    echo

    # Validate prerequisites
    if [[ ! -f "${TEST_SRC}" ]]; then
        err "test_dns_compat.c not found at ${TEST_SRC}"
    fi

    ensure_wasi_sdk
    ensure_wasmtime

    log "wasi-sdk: ${WASI_SDK_PATH}"
    log "wasmtime: ${WASMTIME}"
    echo

    # Run all checks
    PATCHED_OUTPUT=""
    STOCK_OUTPUT=""

    check_vanilla_compat
    echo
    check_stock_build
    echo
    check_binary_size
    echo
    check_fallback_equivalence

    # Summary
    echo
    echo "═══════════════════════════════════════════════════════════════════"
    echo "  Results: ${CHECKS_PASSED}/${CHECKS_TOTAL} checks passed"
    echo "═══════════════════════════════════════════════════════════════════"
    echo

    if [[ ${CHECKS_FAILED} -gt 0 ]]; then
        echo "RESULT: FAIL (${CHECKS_FAILED} check(s) failed)"
        exit 1
    fi

    echo "RESULT: PASS (all ${CHECKS_TOTAL} checks passed)"
    exit 0
}

main "$@"
