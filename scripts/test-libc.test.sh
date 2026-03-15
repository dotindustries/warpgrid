#!/usr/bin/env bash
#
# test-libc.test.sh — TDD tests for the test-libc.sh test harness.
#
# These tests validate flag parsing, output format, and JUnit XML generation
# without requiring wasi-sdk or wasmtime (which are only needed for actual
# compilation and execution of WASM test programs).
#
# Tests:
#   1. --help prints usage and exits 0
#   2. --help output contains all documented flags
#   3. Unknown flag produces error and exits non-zero
#   4. --stock flag is accepted (fails at tool detection, not flag parsing)
#   5. --all flag is accepted (fails at tool detection, not flag parsing)
#   6. --ci flag is accepted (fails at tool detection, not flag parsing)
#   7. --ci produces a JUnit XML file in test-results/
#   8. --ci XML contains <testsuite> and <testcase> elements
#   9. Summary line includes Total/Passed/Failed/Skipped counts
#  10. Default (no flags) targets patched sysroot
#
# Usage:
#   ./test-libc.test.sh          Run all tests
#   ./test-libc.test.sh --quick  Only run tests that don't need wasi-sdk

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEST_LIBC="${SCRIPT_DIR}/test-libc.sh"

PASS=0
FAIL=0
SKIP=0

# ── Helpers ──────────────────────────────────────────────────────────

log()  { echo "==> $*" >&2; }
pass() { PASS=$((PASS + 1)); echo "  PASS: $*" >&2; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL: $*" >&2; }
skip() { SKIP=$((SKIP + 1)); echo "  SKIP: $*" >&2; }

_tmpdir=""
cleanup() {
    if [ -n "${_tmpdir}" ]; then
        rm -rf "${_tmpdir}"
        _tmpdir=""
    fi
}
trap cleanup EXIT

_tmpdir="$(mktemp -d)"

# ── Parse args ───────────────────────────────────────────────────────

QUICK=false
while [ $# -gt 0 ]; do
    case "$1" in
        --quick) QUICK=true ;;
        --help|-h)
            echo "Usage: test-libc.test.sh [--quick]"
            exit 0
            ;;
        *) echo "Unknown flag: $1" >&2; exit 1 ;;
    esac
    shift
done

# ── Prerequisite: script exists and is executable ────────────────────

if [ ! -f "${TEST_LIBC}" ]; then
    echo "ERROR: test-libc.sh not found at ${TEST_LIBC}" >&2
    exit 1
fi

if [ ! -x "${TEST_LIBC}" ]; then
    echo "ERROR: test-libc.sh is not executable" >&2
    exit 1
fi

# ── Test 1: --help prints usage and exits 0 ─────────────────────────

log "Test 1: --help prints usage and exits 0"

EXIT_CODE=0
OUTPUT=$("${TEST_LIBC}" --help 2>&1) || EXIT_CODE=$?

if [ ${EXIT_CODE} -eq 0 ]; then
    if echo "${OUTPUT}" | grep -qi "usage"; then
        pass "--help prints usage and exits 0"
    else
        fail "--help exits 0 but output does not contain 'usage'. Output: ${OUTPUT}"
    fi
else
    fail "--help exited with code ${EXIT_CODE}"
fi

# ── Test 2: --help output contains all documented flags ──────────────

log "Test 2: --help output lists all flags"

EXIT_CODE=0
OUTPUT=$("${TEST_LIBC}" --help 2>&1) || EXIT_CODE=$?

MISSING_FLAGS=""
for flag in "--stock" "--patched" "--all" "--ci" "--help"; do
    if ! echo "${OUTPUT}" | grep -q -- "${flag}"; then
        MISSING_FLAGS="${MISSING_FLAGS} ${flag}"
    fi
done

if [ -z "${MISSING_FLAGS}" ]; then
    pass "--help lists all flags (--stock, --patched, --all, --ci, --help)"
else
    fail "--help missing flags:${MISSING_FLAGS}"
fi

# ── Test 3: Unknown flag produces error and exits non-zero ───────────

log "Test 3: Unknown flag produces error"

EXIT_CODE=0
OUTPUT=$("${TEST_LIBC}" --bogus-flag 2>&1) || EXIT_CODE=$?

if [ ${EXIT_CODE} -ne 0 ]; then
    pass "Unknown flag --bogus-flag exits non-zero (code ${EXIT_CODE})"
else
    fail "Unknown flag --bogus-flag should exit non-zero but got 0"
fi

# ── Test 4: --stock flag is accepted (not rejected as unknown) ───────

log "Test 4: --stock flag is accepted"

# --stock will fail at ensure_wasi_sdk (exit 2), not at flag parsing.
# We verify it does NOT fail with our "Unknown flag" error message.
EXIT_CODE=0
OUTPUT=$("${TEST_LIBC}" --stock 2>&1) || EXIT_CODE=$?

if echo "${OUTPUT}" | grep -q "Unknown flag"; then
    fail "--stock treated as unknown flag"
else
    pass "--stock flag is accepted (exits ${EXIT_CODE} at tool detection, not flag parsing)"
fi

# ── Test 5: --all flag is accepted (not rejected as unknown) ─────────

log "Test 5: --all flag is accepted"

EXIT_CODE=0
OUTPUT=$("${TEST_LIBC}" --all 2>&1) || EXIT_CODE=$?

if echo "${OUTPUT}" | grep -q "Unknown flag"; then
    fail "--all treated as unknown flag"
else
    pass "--all flag is accepted (exits ${EXIT_CODE} at tool detection, not flag parsing)"
fi

# ── Test 6: --ci flag is accepted (not rejected as unknown) ──────────

log "Test 6: --ci flag is accepted"

EXIT_CODE=0
OUTPUT=$("${TEST_LIBC}" --ci 2>&1) || EXIT_CODE=$?

if echo "${OUTPUT}" | grep -q "Unknown flag"; then
    fail "--ci treated as unknown flag"
else
    pass "--ci flag is accepted (exits ${EXIT_CODE} at tool detection, not flag parsing)"
fi

# ── Test 7: --ci produces a JUnit XML file ───────────────────────────
#
# This test requires wasi-sdk and wasmtime to actually run tests and
# generate XML. In environments without these tools, we set up a mock
# sysroot and wasi-sdk to exercise the --ci code path.

log "Test 7: --ci produces a JUnit XML file in test-results/"

RESULTS_DIR="${PROJECT_ROOT}/test-results"

# Create a mock environment so test-libc.sh gets past tool detection
MOCK_SDK="${_tmpdir}/mock-wasi-sdk"
MOCK_SYSROOT_STOCK="${PROJECT_ROOT}/build/sysroot-stock"
MOCK_SYSROOT_PATCHED="${PROJECT_ROOT}/build/sysroot-patched"
MOCK_LIBC_STOCK="${MOCK_SYSROOT_STOCK}/lib/wasm32-wasip2/libc.a"
MOCK_LIBC_PATCHED="${MOCK_SYSROOT_PATCHED}/lib/wasm32-wasip2/libc.a"

# Create mock wasi-sdk with a clang that always "fails to compile"
# This means every test will FAIL at compile, but the harness still
# runs through all tests and generates summary + XML output.
mkdir -p "${MOCK_SDK}/bin"
cat > "${MOCK_SDK}/bin/clang" << 'MOCK_CLANG'
#!/bin/sh
echo "mock: compilation not available" >&2
exit 1
MOCK_CLANG
chmod +x "${MOCK_SDK}/bin/clang"

# Create mock sysroots with libc.a so sysroot detection passes
mkdir -p "$(dirname "${MOCK_LIBC_STOCK}")"
mkdir -p "$(dirname "${MOCK_LIBC_PATCHED}")"
touch "${MOCK_LIBC_STOCK}"
touch "${MOCK_LIBC_PATCHED}"

# Create mock wasmtime
MOCK_WASMTIME="${_tmpdir}/mock-wasmtime"
cat > "${MOCK_WASMTIME}" << 'MOCK_WT'
#!/bin/sh
echo "mock: wasmtime not available" >&2
exit 1
MOCK_WT
chmod +x "${MOCK_WASMTIME}"

# Clean previous results
rm -f "${RESULTS_DIR}"/libc-*.xml 2>/dev/null || true

# Run with mock tools — all tests will fail at compile, but XML should be generated
EXIT_CODE=0
OUTPUT=$(WASI_SDK_PATH="${MOCK_SDK}" WASMTIME="${MOCK_WASMTIME}" "${TEST_LIBC}" --ci 2>&1) || EXIT_CODE=$?

# Check that a JUnit XML file was created
XML_FILES=""
if [ -d "${RESULTS_DIR}" ]; then
    XML_FILES=$(find "${RESULTS_DIR}" -name "libc-*.xml" -type f 2>/dev/null || true)
fi

if [ -n "${XML_FILES}" ]; then
    pass "--ci produces JUnit XML file(s) in test-results/"
else
    fail "--ci did not produce any libc-*.xml in test-results/. Output: ${OUTPUT}"
fi

# ── Test 8: --ci XML contains <testsuite> and <testcase> elements ────

log "Test 8: --ci XML contains JUnit elements"

if [ -n "${XML_FILES}" ]; then
    # Pick the first XML file
    FIRST_XML=$(echo "${XML_FILES}" | head -1)

    HAS_TESTSUITE=false
    HAS_TESTCASE=false

    if grep -q '<testsuite' "${FIRST_XML}"; then
        HAS_TESTSUITE=true
    fi
    if grep -q '<testcase' "${FIRST_XML}"; then
        HAS_TESTCASE=true
    fi

    if ${HAS_TESTSUITE} && ${HAS_TESTCASE}; then
        pass "JUnit XML contains <testsuite> and <testcase> elements"
    elif ${HAS_TESTSUITE}; then
        fail "JUnit XML has <testsuite> but missing <testcase>"
    elif ${HAS_TESTCASE}; then
        fail "JUnit XML has <testcase> but missing <testsuite>"
    else
        fail "JUnit XML missing both <testsuite> and <testcase>. Content: $(cat "${FIRST_XML}")"
    fi
else
    skip "No XML file to validate (Test 7 failed)"
fi

# ── Test 9: Summary line includes Total/Passed/Failed/Skipped ────────

log "Test 9: Summary includes Total/Passed/Failed/Skipped counts"

# Re-use the output from the --ci run in Test 7
HAS_TOTAL=false
HAS_PASSED=false
HAS_FAILED=false
HAS_SKIPPED=false

if echo "${OUTPUT}" | grep -q "Total:"; then
    HAS_TOTAL=true
fi
if echo "${OUTPUT}" | grep -q "Passed:"; then
    HAS_PASSED=true
fi
if echo "${OUTPUT}" | grep -q "Failed:"; then
    HAS_FAILED=true
fi
if echo "${OUTPUT}" | grep -q "Skipped:"; then
    HAS_SKIPPED=true
fi

if ${HAS_TOTAL} && ${HAS_PASSED} && ${HAS_FAILED} && ${HAS_SKIPPED}; then
    pass "Summary includes Total, Passed, Failed, and Skipped counts"
else
    MISSING=""
    ${HAS_TOTAL}   || MISSING="${MISSING} Total"
    ${HAS_PASSED}  || MISSING="${MISSING} Passed"
    ${HAS_FAILED}  || MISSING="${MISSING} Failed"
    ${HAS_SKIPPED} || MISSING="${MISSING} Skipped"
    fail "Summary missing:${MISSING}. Output: ${OUTPUT}"
fi

# ── Test 10: Default (no flags) targets patched sysroot ──────────────

log "Test 10: Default invocation targets patched sysroot"

EXIT_CODE=0
OUTPUT=$(WASI_SDK_PATH="${MOCK_SDK}" WASMTIME="${MOCK_WASMTIME}" "${TEST_LIBC}" 2>&1) || EXIT_CODE=$?

if echo "${OUTPUT}" | grep -q "patched"; then
    pass "Default invocation targets patched sysroot"
else
    fail "Default invocation does not mention 'patched'. Output: ${OUTPUT}"
fi

# ── Cleanup mock sysroots ────────────────────────────────────────────

# Only remove mock libc.a files we created — don't remove real sysroots
# We check file size: our mocks are empty (0 bytes)
if [ -f "${MOCK_LIBC_STOCK}" ] && [ ! -s "${MOCK_LIBC_STOCK}" ]; then
    rm -f "${MOCK_LIBC_STOCK}"
    rmdir -p "$(dirname "${MOCK_LIBC_STOCK}")" 2>/dev/null || true
fi
if [ -f "${MOCK_LIBC_PATCHED}" ] && [ ! -s "${MOCK_LIBC_PATCHED}" ]; then
    rm -f "${MOCK_LIBC_PATCHED}"
    rmdir -p "$(dirname "${MOCK_LIBC_PATCHED}")" 2>/dev/null || true
fi

# ── Summary ──────────────────────────────────────────────────────────

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"

if [ "${FAIL}" -gt 0 ]; then
    exit 1
fi
exit 0
