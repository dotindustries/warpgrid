#!/usr/bin/env bash
# test-bun.sh — Run the full Bun test suite for WarpGrid.
#
# Validates: SDK unit tests, compatibility tests, compilation pipeline,
# and integration test application (T5).
#
# Usage:
#   scripts/test-bun.sh              # Run all Bun tests
#   scripts/test-bun.sh --unit       # Unit tests only (SDK + polyfills)
#   scripts/test-bun.sh --build      # Compilation pipeline tests only
#   scripts/test-bun.sh --t5         # T5 integration test app only
#   scripts/test-bun.sh --verbose    # Show full test output
#   scripts/test-bun.sh --help       # Show this help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VERBOSE=false
RUN_UNIT=true
RUN_BUILD=true
RUN_T5=true
EXIT_CODE=0

# ── Parse arguments ──────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case "$1" in
        --unit)
            RUN_BUILD=false
            RUN_T5=false
            shift
            ;;
        --build)
            RUN_UNIT=false
            RUN_T5=false
            shift
            ;;
        --t5)
            RUN_UNIT=false
            RUN_BUILD=false
            shift
            ;;
        --verbose)
            VERBOSE=true
            shift
            ;;
        --help|-h)
            awk '/^# test-bun/,/^$/' "$0" | sed 's/^# //' | sed 's/^#//'
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

# ── Helpers ──────────────────────────────────────────────────────────────

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0

pass() { echo "  PASS: $1"; PASS_COUNT=$((PASS_COUNT + 1)); }
fail() { echo "  FAIL: $1"; FAIL_COUNT=$((FAIL_COUNT + 1)); EXIT_CODE=1; }
skip() { echo "  SKIP: $1 ($2)"; SKIP_COUNT=$((SKIP_COUNT + 1)); }

run_cmd() {
    if $VERBOSE; then
        "$@"
    else
        "$@" > /dev/null 2>&1
    fi
}

# ── Prerequisites ────────────────────────────────────────────────────────

echo "=== WarpGrid Bun Test Suite ==="
echo ""

if ! command -v bun > /dev/null 2>&1; then
    echo "ERROR: bun not found. Install from https://bun.sh"
    exit 1
fi

BUN_VERSION=$(bun --version 2>/dev/null || echo "unknown")
echo "Bun version: ${BUN_VERSION}"
echo ""

# ── Unit tests: SDK packages ────────────────────────────────────────────

if $RUN_UNIT; then
    echo "--- Unit Tests ---"

    # warpgrid-bun-sdk
    SDK_DIR="${PROJECT_ROOT}/packages/warpgrid-bun-sdk"
    if [[ -d "${SDK_DIR}" ]] && [[ -f "${SDK_DIR}/package.json" ]]; then
        if run_cmd bun test --cwd "${SDK_DIR}"; then
            pass "packages/warpgrid-bun-sdk"
        else
            fail "packages/warpgrid-bun-sdk"
        fi
    else
        skip "packages/warpgrid-bun-sdk" "directory not found"
    fi

    echo ""
fi

# ── Build pipeline tests: cargo test for warp-pack bun module ───────────

if $RUN_BUILD; then
    echo "--- Build Pipeline Tests ---"

    if run_cmd cargo test -p warp-pack -- bun --nocapture; then
        pass "warp-pack bun tests (cargo test)"
    else
        fail "warp-pack bun tests (cargo test)"
    fi

    echo ""
fi

# ── T5 integration test: Bun HTTP + Postgres ────────────────────────────

if $RUN_T5; then
    echo "--- T5 Integration Test ---"

    T5_DIR="${PROJECT_ROOT}/test-apps/t5-bun-http-postgres"
    if [[ -d "${T5_DIR}" ]]; then
        # Run unit tests for the T5 handler
        if [[ -f "${T5_DIR}/package.json" ]]; then
            if run_cmd bun test --cwd "${T5_DIR}"; then
                pass "test-apps/t5-bun-http-postgres (unit tests)"
            else
                fail "test-apps/t5-bun-http-postgres (unit tests)"
            fi
        else
            skip "test-apps/t5-bun-http-postgres (unit tests)" "no package.json"
        fi

        # Run build if build.sh exists
        if [[ -x "${T5_DIR}/build.sh" ]]; then
            if run_cmd "${T5_DIR}/build.sh" --standalone; then
                pass "test-apps/t5-bun-http-postgres (build)"
            else
                fail "test-apps/t5-bun-http-postgres (build)"
            fi
        else
            skip "test-apps/t5-bun-http-postgres (build)" "build.sh not found or not executable"
        fi
    else
        skip "test-apps/t5-bun-http-postgres" "directory not found"
    fi

    echo ""
fi

# ── Summary ──────────────────────────────────────────────────────────────

echo "=== Summary ==="
echo "  Passed:  ${PASS_COUNT}"
echo "  Failed:  ${FAIL_COUNT}"
echo "  Skipped: ${SKIP_COUNT}"
echo ""

if [[ ${FAIL_COUNT} -gt 0 ]]; then
    echo "RESULT: FAIL"
else
    echo "RESULT: PASS"
fi

exit ${EXIT_CODE}
