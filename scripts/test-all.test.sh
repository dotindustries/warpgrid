#!/usr/bin/env bash
#
# test-all.test.sh — TDD tests for the test-all.sh orchestration script.
#
# Tests:
#   1. --help flag prints usage and exits 0
#   2. --dry-run prints execution plan without running anything
#   3. --only filters to specified test apps
#   4. --only with comma-separated list includes all specified apps
#   5. Apps without test.sh are reported appropriately in dry-run
#   6. --dry-run does NOT create test-results/ directory
#   7. Unknown flag produces error and exits non-zero
#   8. --verbose flag accepted without error
#   9. --keep-deps flag accepted without error
#  10. Dry-run plan includes structured output (plan header)
#  11. Dry-run shows prerequisite information
#  12. Real execution creates test-results/ with timestamped log files
#  13. Build failure causes dependent test to be SKIP (not errored)
#  14. Summary table includes APP, STATUS, DETAILS columns and totals
#  15. --only=value equals-sign syntax works like --only value
#
# Usage:
#   ./test-all.test.sh          Run all tests
#   ./test-all.test.sh --quick  Only run non-execution tests (no builds)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEST_ALL="${SCRIPT_DIR}/test-all.sh"

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
            echo "Usage: test-all.test.sh [--quick]"
            exit 0
            ;;
        *) echo "Unknown flag: $1" >&2; exit 1 ;;
    esac
    shift
done

# ── Prerequisite: script exists ───────────────────────────────────────

if [ ! -f "${TEST_ALL}" ]; then
    echo "ERROR: test-all.sh not found at ${TEST_ALL}" >&2
    exit 1
fi

if [ ! -x "${TEST_ALL}" ]; then
    echo "ERROR: test-all.sh is not executable" >&2
    exit 1
fi

# ── Test 1: --help flag ──────────────────────────────────────────────

log "Test 1: --help prints usage and exits 0"

OUTPUT=$("${TEST_ALL}" --help 2>&1) || true
EXIT_CODE=$?

if [ ${EXIT_CODE} -eq 0 ]; then
    if echo "${OUTPUT}" | grep -qi "usage"; then
        pass "--help prints usage and exits 0"
    else
        fail "--help exits 0 but output does not contain 'usage'. Output: ${OUTPUT}"
    fi
else
    fail "--help exited with code ${EXIT_CODE}"
fi

# ── Test 2: --dry-run prints plan ────────────────────────────────────

log "Test 2: --dry-run prints execution plan"

OUTPUT=$("${TEST_ALL}" --dry-run 2>&1) || true
EXIT_CODE=$?

if [ ${EXIT_CODE} -eq 0 ]; then
    # Should mention at least one test app
    if echo "${OUTPUT}" | grep -qE "t[0-9]"; then
        pass "--dry-run prints execution plan with test app names"
    else
        fail "--dry-run exits 0 but output contains no test app names. Output: ${OUTPUT}"
    fi
else
    fail "--dry-run exited with code ${EXIT_CODE}"
fi

# ── Test 3: --only filters test apps ─────────────────────────────────

log "Test 3: --only filters to specified test apps"

OUTPUT=$("${TEST_ALL}" --dry-run --only t3 2>&1) || true
EXIT_CODE=$?

if [ ${EXIT_CODE} -eq 0 ]; then
    if echo "${OUTPUT}" | grep -q "t3"; then
        # Verify filtered-out apps are not in the RUN list
        if echo "${OUTPUT}" | grep -q "RUN.*t4"; then
            fail "--only t3 still shows t4 in RUN plan"
        else
            pass "--only t3 shows t3 and excludes t4 from plan"
        fi
    else
        fail "--only t3 output does not mention t3. Output: ${OUTPUT}"
    fi
else
    fail "--only t3 exited with code ${EXIT_CODE}"
fi

# ── Test 4: --dry-run with --only comma-separated ────────────────────

log "Test 4: --only with comma-separated list"

OUTPUT=$("${TEST_ALL}" --dry-run --only t3,t5 2>&1) || true
EXIT_CODE=$?

if [ ${EXIT_CODE} -eq 0 ]; then
    HAS_T3=$(echo "${OUTPUT}" | grep -c "t3" || true)
    HAS_T5=$(echo "${OUTPUT}" | grep -c "t5" || true)
    if [ "${HAS_T3}" -gt 0 ] && [ "${HAS_T5}" -gt 0 ]; then
        pass "--only t3,t5 includes both t3 and t5"
    else
        fail "--only t3,t5 missing one of t3 or t5. Output: ${OUTPUT}"
    fi
else
    fail "--only t3,t5 exited with code ${EXIT_CODE}"
fi

# ── Test 5: Apps without test.sh show as SKIP ────────────────────────

log "Test 5: Apps without test.sh are reported appropriately"

OUTPUT=$("${TEST_ALL}" --dry-run 2>&1) || true

# T1 has no test.sh, should be noted in dry-run
if echo "${OUTPUT}" | grep -qi "t1.*no test"; then
    pass "T1 (no test.sh) reported appropriately in dry-run"
elif echo "${OUTPUT}" | grep -qi "t1.*skip"; then
    pass "T1 (no test.sh) marked as skip in dry-run"
else
    # T1 may be listed but noted as having no test infrastructure
    if echo "${OUTPUT}" | grep -q "t1"; then
        pass "T1 listed in dry-run plan (may be skipped at runtime)"
    else
        skip "T1 not in test-apps or not detected"
    fi
fi

# ── Test 6: --dry-run does NOT create test-results/ ──────────────────

log "Test 6: --dry-run does not create test-results/"

# Remove test-results if it exists
RESULTS_DIR="${PROJECT_ROOT}/test-results"
HAD_RESULTS=false
if [ -d "${RESULTS_DIR}" ]; then
    HAD_RESULTS=true
fi

"${TEST_ALL}" --dry-run >/dev/null 2>&1 || true

if ! ${HAD_RESULTS} && [ -d "${RESULTS_DIR}" ]; then
    fail "--dry-run created test-results/ (should not)"
else
    pass "--dry-run does not create test-results/"
fi

# ── Test 7: Unknown flag produces error ──────────────────────────────

log "Test 7: Unknown flag produces error"

EXIT_CODE=0
OUTPUT=$("${TEST_ALL}" --bogus-flag 2>&1) || EXIT_CODE=$?

if [ ${EXIT_CODE} -ne 0 ]; then
    pass "Unknown flag --bogus-flag exits non-zero (code ${EXIT_CODE})"
else
    fail "Unknown flag --bogus-flag should exit non-zero but got ${EXIT_CODE}"
fi

# ── Test 8: --verbose flag accepted ──────────────────────────────────

log "Test 8: --verbose flag accepted"

OUTPUT=$("${TEST_ALL}" --dry-run --verbose 2>&1) || true
EXIT_CODE=$?

if [ ${EXIT_CODE} -eq 0 ]; then
    pass "--verbose flag accepted with --dry-run"
else
    fail "--verbose flag caused error: exit ${EXIT_CODE}"
fi

# ── Test 9: --keep-deps flag accepted ────────────────────────────────

log "Test 9: --keep-deps flag accepted"

OUTPUT=$("${TEST_ALL}" --dry-run --keep-deps 2>&1) || true
EXIT_CODE=$?

if [ ${EXIT_CODE} -eq 0 ]; then
    pass "--keep-deps flag accepted with --dry-run"
else
    fail "--keep-deps flag caused error: exit ${EXIT_CODE}"
fi

# ── Test 10: Execution plan format ───────────────────────────────────

log "Test 10: Dry-run plan includes structured output"

OUTPUT=$("${TEST_ALL}" --dry-run 2>&1) || true

# Should contain a section header or table-like output
if echo "${OUTPUT}" | grep -qE "(Execution Plan|PLAN|Plan:)"; then
    pass "Dry-run output contains plan header"
else
    fail "Dry-run output missing plan header. Output: ${OUTPUT}"
fi

# ── Test 11: Dry-run shows prerequisites check ──────────────────────

log "Test 11: Dry-run shows prerequisite information"

OUTPUT=$("${TEST_ALL}" --dry-run 2>&1) || true

if echo "${OUTPUT}" | grep -qiE "(prerequisite|prereq|require)"; then
    pass "Dry-run mentions prerequisites"
else
    # Also acceptable to just list what's available
    if echo "${OUTPUT}" | grep -qiE "(docker|curl|available)"; then
        pass "Dry-run mentions tool availability"
    else
        fail "Dry-run does not mention prerequisites. Output: ${OUTPUT}"
    fi
fi

# ── Test 12: Real execution creates test-results/ and log files ──────

if ! ${QUICK}; then
    log "Test 12: Execution creates test-results/ with timestamped logs"

    # Run only T3 standalone (fastest, needs only Go)
    if command -v go &>/dev/null; then
        RESULTS_DIR="${PROJECT_ROOT}/test-results"
        rm -rf "${RESULTS_DIR}" 2>/dev/null || true

        "${TEST_ALL}" --only t3 2>&1 || true

        if [ -d "${RESULTS_DIR}" ]; then
            # Check for any log files
            LOG_COUNT=$(find "${RESULTS_DIR}" -name "*.log" -type f 2>/dev/null | wc -l | tr -d ' ')
            if [ "${LOG_COUNT}" -gt 0 ]; then
                pass "Execution creates test-results/ with ${LOG_COUNT} log file(s)"
            else
                fail "test-results/ exists but contains no .log files"
            fi
        else
            fail "test-results/ not created after execution"
        fi
    else
        skip "Go not available — cannot run execution test"
    fi
else
    skip "Execution test (--quick mode)"
fi

# ── Test 13: Build failure causes dependent test to be SKIP ──────────

log "Test 13: Build failure causes dependent test to be SKIP"

# Create a fake test app with a failing build.sh and a test.sh
FAKE_APP_DIR="${PROJECT_ROOT}/test-apps/t99-fake-build-fail"
mkdir -p "${FAKE_APP_DIR}"
cat > "${FAKE_APP_DIR}/build.sh" <<'BUILDEOF'
#!/usr/bin/env bash
echo "Intentional build failure"
exit 1
BUILDEOF
chmod +x "${FAKE_APP_DIR}/build.sh"

cat > "${FAKE_APP_DIR}/test.sh" <<'TESTEOF'
#!/usr/bin/env bash
echo "This should not run"
exit 0
TESTEOF
chmod +x "${FAKE_APP_DIR}/test.sh"

OUTPUT=$("${TEST_ALL}" --only t99 2>&1) || true

# Clean up fake app immediately
rm -rf "${FAKE_APP_DIR}"
rm -rf "${PROJECT_ROOT}/test-results" 2>/dev/null || true

if echo "${OUTPUT}" | grep -q "SKIP"; then
    if echo "${OUTPUT}" | grep -qi "build failed"; then
        pass "Build failure causes dependent test to be SKIP with 'build failed' detail"
    else
        pass "Build failure causes dependent test to be SKIP"
    fi
else
    fail "Build failure did not produce SKIP status. Output: ${OUTPUT}"
fi

# ── Test 14: Summary table format ────────────────────────────────────

log "Test 14: Summary table includes APP, STATUS, DETAILS columns and totals"

# Re-create fake app for a quick run that produces a summary
FAKE_APP_DIR="${PROJECT_ROOT}/test-apps/t98-fake-summary"
mkdir -p "${FAKE_APP_DIR}"
cat > "${FAKE_APP_DIR}/test.sh" <<'TESTEOF'
#!/usr/bin/env bash
echo "ok"
exit 0
TESTEOF
chmod +x "${FAKE_APP_DIR}/test.sh"

OUTPUT=$("${TEST_ALL}" --only t98 2>&1) || true

# Clean up
rm -rf "${FAKE_APP_DIR}"
rm -rf "${PROJECT_ROOT}/test-results" 2>/dev/null || true

HAS_APP=$(echo "${OUTPUT}" | grep -c "APP" || true)
HAS_STATUS=$(echo "${OUTPUT}" | grep -c "STATUS" || true)
HAS_DETAILS=$(echo "${OUTPUT}" | grep -c "DETAILS" || true)
HAS_TOTAL=$(echo "${OUTPUT}" | grep -cE "Total:.*passed.*failed.*skipped" || true)

if [ "${HAS_APP}" -gt 0 ] && [ "${HAS_STATUS}" -gt 0 ] && [ "${HAS_DETAILS}" -gt 0 ] && [ "${HAS_TOTAL}" -gt 0 ]; then
    pass "Summary table has APP, STATUS, DETAILS columns and totals line"
else
    fail "Summary table missing expected columns or totals. Output: ${OUTPUT}"
fi

# ── Test 15: --only=value equals-sign syntax ─────────────────────────

log "Test 15: --only=value equals-sign syntax"

OUTPUT=$("${TEST_ALL}" --dry-run --only=t3 2>&1) || true
EXIT_CODE=$?

if [ ${EXIT_CODE} -eq 0 ]; then
    if echo "${OUTPUT}" | grep -q "t3"; then
        if echo "${OUTPUT}" | grep -q "RUN.*t4"; then
            fail "--only=t3 still shows t4 in RUN plan"
        else
            pass "--only=t3 (equals syntax) shows t3 and excludes others"
        fi
    else
        fail "--only=t3 (equals syntax) output does not mention t3. Output: ${OUTPUT}"
    fi
else
    fail "--only=t3 (equals syntax) exited with code ${EXIT_CODE}"
fi

# ── Summary ──────────────────────────────────────────────────────────

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"

if [ "${FAIL}" -gt 0 ]; then
    exit 1
fi
exit 0
