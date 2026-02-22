#!/usr/bin/env bash
#
# test-libc.sh — Test harness for the WarpGrid wasi-libc sysroot.
#
# Compiles test programs from libc-patches/tests/ against stock and/or
# patched sysroots, runs them in Wasmtime, and reports results.
#
# Usage:
#   scripts/test-libc.sh                  # Test patched sysroot (default)
#   scripts/test-libc.sh --stock          # Test stock sysroot
#   scripts/test-libc.sh --patched        # Test patched sysroot
#   scripts/test-libc.sh --all            # Test both sysroots
#   scripts/test-libc.sh --help           # Show this help
#
# Environment variables:
#   WASI_SDK_PATH    Path to wasi-sdk (uses build cache if unset)
#   WASMTIME         Path to wasmtime binary (auto-detected)
#   TARGET_TRIPLE    WASI target triple (default: wasm32-wasip2)
#
# Exit codes:
#   0  All tests passed
#   1  One or more tests failed
#   2  Configuration error (missing tools, sysroot not built, etc.)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_DIR="${PROJECT_ROOT}/build"
TESTS_DIR="${PROJECT_ROOT}/libc-patches/tests"
TARGET_TRIPLE="${TARGET_TRIPLE:-wasm32-wasip2}"

# Counters
TOTAL=0
PASSED=0
FAILED=0
SKIPPED=0

# ─── Helpers ─────────────────────────────────────────────────────────────────

log() { echo "==> $*" >&2; }
err() { echo "ERROR: $*" >&2; exit 2; }

usage() {
    echo "Usage: scripts/test-libc.sh [--stock|--patched|--all|--help]"
    echo
    echo "Flags:"
    echo "  --stock          Test against stock (unpatched) sysroot"
    echo "  --patched        Test against patched sysroot (default)"
    echo "  --all            Test against both sysroots"
    echo "  --ci             Exit non-zero on any failure, produce JUnit XML"
    echo "  --help           Show this help message"
    echo
    echo "Environment:"
    echo "  WASI_SDK_PATH    Path to wasi-sdk (uses build cache if unset)"
    echo "  WASMTIME         Path to wasmtime binary"
    echo "  TARGET_TRIPLE    Target triple (default: ${TARGET_TRIPLE})"
}

# ─── Tool detection ──────────────────────────────────────────────────────────

ensure_wasi_sdk() {
    if [[ -n "${WASI_SDK_PATH:-}" ]] && [[ -x "${WASI_SDK_PATH}/bin/clang" ]]; then
        return
    fi

    # Try build cache
    local cached
    cached=$(ls -d "${BUILD_DIR}/cache/wasi-sdk-"* 2>/dev/null | head -1)
    if [[ -n "${cached}" && -x "${cached}/bin/clang" ]]; then
        WASI_SDK_PATH="${cached}"
        log "Using cached wasi-sdk at ${WASI_SDK_PATH}"
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

# ─── Test execution ──────────────────────────────────────────────────────────

# Compile a C test program against a sysroot and return path to .wasm
compile_test() {
    local src="${1}"
    local sysroot="${2}"
    local output="${3}"

    "${WASI_SDK_PATH}/bin/clang" \
        --target="${TARGET_TRIPLE}" \
        --sysroot="${sysroot}" \
        -O1 \
        -o "${output}" \
        "${src}" 2>&1
}

# Run a .wasm file in wasmtime and capture output
run_test() {
    local wasm="${1}"
    local timeout_sec="${2:-10}"

    # wasmtime needs different flags depending on target
    local wasmtime_flags=()
    case "${TARGET_TRIPLE}" in
        *wasip2*) wasmtime_flags+=("--wasm" "component-model=y") ;;
        *wasip1*) ;; # no extra flags needed
    esac

    # macOS doesn't have `timeout`; use perl as portable fallback
    if command -v timeout &>/dev/null; then
        timeout "${timeout_sec}" "${WASMTIME}" run "${wasmtime_flags[@]}" "${wasm}" 2>&1
    else
        perl -e "alarm ${timeout_sec}; exec @ARGV" -- "${WASMTIME}" run "${wasmtime_flags[@]}" "${wasm}" 2>&1
    fi
}

# Run a single test: compile + execute
run_single_test() {
    local src="${1}"
    local sysroot="${2}"
    local variant="${3}"
    local test_name
    test_name="$(basename "${src}" .c)"

    local wasm_dir="${BUILD_DIR}/test-wasm-${variant}"
    mkdir -p "${wasm_dir}"
    local wasm_file="${wasm_dir}/${test_name}.wasm"

    TOTAL=$((TOTAL + 1))

    # Check if this test requires shims (has WARPGRID_SHIM marker)
    if grep -q 'WARPGRID_SHIM_REQUIRED' "${src}" 2>/dev/null; then
        if [[ "${variant}" == "stock" ]]; then
            printf "  %-30s  SKIP (shim not available)\n" "${test_name}"
            SKIPPED=$((SKIPPED + 1))
            return
        fi
    fi

    # Compile
    local compile_output
    if ! compile_output=$(compile_test "${src}" "${sysroot}" "${wasm_file}" 2>&1); then
        printf "  %-30s  FAIL (compile error)\n" "${test_name}"
        if [[ -n "${compile_output}" ]]; then
            echo "    ${compile_output}" | head -5 | sed 's/^/    /'
        fi
        FAILED=$((FAILED + 1))
        return
    fi

    # Run
    local run_output
    local exit_code=0
    run_output=$(run_test "${wasm_file}" 10 2>&1) || exit_code=$?

    if [[ ${exit_code} -eq 0 ]]; then
        printf "  %-30s  PASS\n" "${test_name}"
        PASSED=$((PASSED + 1))
    else
        printf "  %-30s  FAIL (exit code ${exit_code})\n" "${test_name}"
        if [[ -n "${run_output}" ]]; then
            echo "${run_output}" | head -5 | sed 's/^/    /'
        fi
        FAILED=$((FAILED + 1))
    fi
}

# Run all tests against a sysroot
run_test_suite() {
    local variant="${1}"
    local sysroot="${BUILD_DIR}/sysroot-${variant}"

    if [[ ! -d "${sysroot}" ]]; then
        err "Sysroot not found at ${sysroot}. Run 'scripts/build-libc.sh --${variant}' first."
    fi

    local libc_a="${sysroot}/lib/${TARGET_TRIPLE}/libc.a"
    if [[ ! -f "${libc_a}" ]]; then
        err "libc.a not found in sysroot at ${libc_a}"
    fi

    echo
    echo "─── Testing ${variant} sysroot ───"
    echo "  Sysroot: ${sysroot}"
    echo "  Target:  ${TARGET_TRIPLE}"
    echo

    local test_files=()
    while IFS= read -r -d '' src; do
        test_files+=("${src}")
    done < <(find "${TESTS_DIR}" -maxdepth 1 -name '*.c' -print0 | sort -z)

    if [[ ${#test_files[@]} -eq 0 ]]; then
        log "No test files found in ${TESTS_DIR}/"
        return
    fi

    for src in "${test_files[@]}"; do
        run_single_test "${src}" "${sysroot}" "${variant}"
    done
}

# Print summary
print_summary() {
    echo
    echo "─── Summary ───"
    echo "  Total:   ${TOTAL}"
    echo "  Passed:  ${PASSED}"
    echo "  Failed:  ${FAILED}"
    echo "  Skipped: ${SKIPPED}"
    echo
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
    local test_stock=false
    local test_patched=false
    local ci_mode=false

    # Default to patched if no flags given
    if [[ $# -eq 0 ]]; then
        test_patched=true
    fi

    while [[ $# -gt 0 ]]; do
        case "${1}" in
            --stock)   test_stock=true ;;
            --patched) test_patched=true ;;
            --all)     test_stock=true; test_patched=true ;;
            --ci)      ci_mode=true; test_stock=true; test_patched=true ;;
            --help|-h) usage; exit 0 ;;
            *)         err "Unknown flag: ${1}. Use --help for usage." ;;
        esac
        shift
    done

    # Ensure tools are available
    ensure_wasi_sdk
    ensure_wasmtime

    log "wasi-sdk: ${WASI_SDK_PATH}"
    log "wasmtime: ${WASMTIME}"

    # Run test suites
    if ${test_stock}; then
        run_test_suite "stock"
    fi

    if ${test_patched}; then
        run_test_suite "patched"
    fi

    print_summary

    # Exit code
    if [[ ${FAILED} -gt 0 ]]; then
        exit 1
    fi
}

main "$@"
