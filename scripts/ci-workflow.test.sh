#!/usr/bin/env bash
#
# ci-workflow.test.sh — TDD tests for .github/workflows/integration-tests.yml
#
# Validates the CI workflow YAML structure against all acceptance criteria
# for US-709 (cross-domain integration tests) without executing the workflow.
#
# Tests:
#   1.  Workflow triggers on push to main
#   2.  Workflow triggers on pull_request to main
#   3.  Workflow triggers on schedule (nightly cron)
#   4.  Workflow triggers on workflow_dispatch (manual)
#   5.  T1 job exists as separate job
#   6.  T2 job exists as separate job
#   7.  T3 job exists as separate job
#   8.  T4 job exists as separate job
#   9.  T5 job exists as separate job
#  10.  T6 job exists as separate job
#  11.  Summary gate job (integration-summary) exists
#  12.  Summary gate depends on all T1–T6 jobs
#  13.  Summary gate uses if: always()
#  14.  All T1–T6 jobs have timeout-minutes: 15
#  15.  All T1–T6 jobs have actions/checkout@v4
#  16.  All T1–T6 jobs upload artifacts via actions/upload-artifact@v4
#  17.  Jobs with Rust use Swatinem/rust-cache@v2 or actions/cache
#  18.  Docker services have health check configurations
#  19.  Concurrency group is configured with cancel-in-progress
#  20.  T3 job has Postgres service (DATABASE_URL requires it)
#  21.  wasm-tools installs use caching (actions/cache or cargo-binstall)
#  22.  Nightly schedule context is annotated in summary job
#
# Usage:
#   ./ci-workflow.test.sh          Run all tests
#   ./ci-workflow.test.sh --help   Show this help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKFLOW="${PROJECT_ROOT}/.github/workflows/integration-tests.yml"

PASS=0
FAIL=0
SKIP=0

# ── Helpers ──────────────────────────────────────────────────────────

log()  { echo "==> $*" >&2; }
pass() { PASS=$((PASS + 1)); echo "  PASS: $*" >&2; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL: $*" >&2; }
skip() { SKIP=$((SKIP + 1)); echo "  SKIP: $*" >&2; }

# ── Parse args ───────────────────────────────────────────────────────

while [ $# -gt 0 ]; do
    case "$1" in
        --help|-h)
            echo "Usage: ci-workflow.test.sh [--help]"
            echo "Validates integration-tests.yml against acceptance criteria."
            exit 0
            ;;
        *) echo "Unknown flag: $1" >&2; exit 1 ;;
    esac
    shift
done

# ── Prerequisite: workflow file exists ────────────────────────────────

if [ ! -f "${WORKFLOW}" ]; then
    echo "ERROR: integration-tests.yml not found at ${WORKFLOW}" >&2
    exit 1
fi

log "Validating ${WORKFLOW}"

# Read the workflow file once
WF_CONTENT="$(cat "${WORKFLOW}")"

# Helper: extract a job block (from "  jobname:" to the next top-level job or EOF)
# Uses awk to extract content between job start and next job at same indent level
job_block() {
    local job_id="$1"
    echo "${WF_CONTENT}" | awk "
        /^  ${job_id}:/ { found=1; print; next }
        found && /^  [a-zA-Z]/ { exit }
        found { print }
    "
}

# ── Test 1: Workflow triggers on push to main ────────────────────────

log "Test 1: push trigger to main"
if echo "${WF_CONTENT}" | grep -q "push:" && echo "${WF_CONTENT}" | grep -q "branches:.*\[main\]"; then
    pass "push trigger to main"
else
    fail "push trigger to main not found"
fi

# ── Test 2: Workflow triggers on pull_request to main ─────────────────

log "Test 2: pull_request trigger to main"
if echo "${WF_CONTENT}" | grep -q "pull_request:"; then
    pass "pull_request trigger"
else
    fail "pull_request trigger not found"
fi

# ── Test 3: Workflow triggers on schedule (nightly cron) ──────────────

log "Test 3: schedule trigger (nightly cron)"
if echo "${WF_CONTENT}" | grep -q "schedule:" && echo "${WF_CONTENT}" | grep -qE "cron:.*['\"]"; then
    pass "schedule trigger with cron"
else
    fail "schedule trigger with cron not found"
fi

# ── Test 4: Workflow triggers on workflow_dispatch (manual) ───────────

log "Test 4: workflow_dispatch trigger"
if echo "${WF_CONTENT}" | grep -q "workflow_dispatch:"; then
    pass "workflow_dispatch trigger"
else
    fail "workflow_dispatch trigger not found"
fi

# ── Tests 5-10: Each T1–T6 job exists ────────────────────────────────

JOBS=(
    "t1-rust-http-postgres"
    "t2-rust-http-redis-postgres"
    "t3-go-http-postgres"
    "t4-ts-http-postgres"
    "t5-bun-http-postgres"
    "t6-multi-service"
)

test_num=5
for job in "${JOBS[@]}"; do
    log "Test ${test_num}: ${job} job exists"
    if echo "${WF_CONTENT}" | grep -qE "^  ${job}:"; then
        pass "${job} job exists"
    else
        fail "${job} job not found"
    fi
    test_num=$((test_num + 1))
done

# ── Test 11: Summary gate job exists ──────────────────────────────────

log "Test 11: integration-summary job exists"
if echo "${WF_CONTENT}" | grep -qE "^  integration-summary:"; then
    pass "integration-summary job exists"
else
    fail "integration-summary job not found"
fi

# ── Test 12: Summary gate depends on all T1–T6 jobs ──────────────────

log "Test 12: integration-summary depends on all T1–T6 jobs"
SUMMARY_BLOCK="$(job_block "integration-summary")"
ALL_DEPS=true
for job in "${JOBS[@]}"; do
    if ! echo "${SUMMARY_BLOCK}" | grep -qF -- "- ${job}"; then
        fail "integration-summary missing dependency: ${job}"
        ALL_DEPS=false
    fi
done
if [ "${ALL_DEPS}" = true ]; then
    pass "integration-summary depends on all T1–T6 jobs"
fi

# ── Test 13: Summary gate uses if: always() ───────────────────────────

log "Test 13: integration-summary uses if: always()"
if echo "${SUMMARY_BLOCK}" | grep -qE "if:.*always\(\)"; then
    pass "integration-summary uses if: always()"
else
    fail "integration-summary missing if: always()"
fi

# ── Test 14: All T1–T6 jobs have timeout-minutes: 15 ─────────────────

log "Test 14: All T1–T6 jobs have timeout-minutes: 15"
ALL_TIMEOUTS=true
for job in "${JOBS[@]}"; do
    BLOCK="$(job_block "${job}")"
    if ! echo "${BLOCK}" | grep -qE "timeout-minutes:.*15"; then
        fail "${job} missing timeout-minutes: 15"
        ALL_TIMEOUTS=false
    fi
done
if [ "${ALL_TIMEOUTS}" = true ]; then
    pass "all T1–T6 jobs have timeout-minutes: 15"
fi

# ── Test 15: All T1–T6 jobs have actions/checkout@v4 ─────────────────

log "Test 15: All T1–T6 jobs have actions/checkout@v4"
ALL_CHECKOUT=true
for job in "${JOBS[@]}"; do
    BLOCK="$(job_block "${job}")"
    if ! echo "${BLOCK}" | grep -q "actions/checkout@v4"; then
        fail "${job} missing actions/checkout@v4"
        ALL_CHECKOUT=false
    fi
done
if [ "${ALL_CHECKOUT}" = true ]; then
    pass "all T1–T6 jobs have actions/checkout@v4"
fi

# ── Test 16: All T1–T6 jobs upload artifacts ──────────────────────────

log "Test 16: All T1–T6 jobs upload artifacts via actions/upload-artifact@v4"
ALL_UPLOAD=true
for job in "${JOBS[@]}"; do
    BLOCK="$(job_block "${job}")"
    if ! echo "${BLOCK}" | grep -q "actions/upload-artifact@v4"; then
        fail "${job} missing actions/upload-artifact@v4"
        ALL_UPLOAD=false
    fi
done
if [ "${ALL_UPLOAD}" = true ]; then
    pass "all T1–T6 jobs upload artifacts"
fi

# ── Test 17: Jobs with Rust use caching ───────────────────────────────

log "Test 17: Rust jobs use Swatinem/rust-cache@v2 or actions/cache"
RUST_JOBS=("t1-rust-http-postgres" "t2-rust-http-redis-postgres" "t6-multi-service")
ALL_RUST_CACHE=true
for job in "${RUST_JOBS[@]}"; do
    BLOCK="$(job_block "${job}")"
    if echo "${BLOCK}" | grep -qE "Swatinem/rust-cache|actions/cache"; then
        true
    else
        fail "${job} missing Rust caching"
        ALL_RUST_CACHE=false
    fi
done
if [ "${ALL_RUST_CACHE}" = true ]; then
    pass "Rust jobs use caching"
fi

# ── Test 18: Docker services have health checks ──────────────────────

log "Test 18: Docker services have health check configurations"
# Jobs with services: T1, T2, T3, T6
SERVICE_JOBS=("t1-rust-http-postgres" "t2-rust-http-redis-postgres" "t3-go-http-postgres" "t6-multi-service")
ALL_HEALTH=true
for job in "${SERVICE_JOBS[@]}"; do
    BLOCK="$(job_block "${job}")"
    if echo "${BLOCK}" | grep -q "services:"; then
        if ! echo "${BLOCK}" | grep -q "health-cmd"; then
            fail "${job} services missing health check"
            ALL_HEALTH=false
        fi
    else
        fail "${job} expected to have services block but none found"
        ALL_HEALTH=false
    fi
done
if [ "${ALL_HEALTH}" = true ]; then
    pass "all Docker services have health checks"
fi

# ── Test 19: Concurrency with cancel-in-progress ─────────────────────

log "Test 19: concurrency with cancel-in-progress"
if echo "${WF_CONTENT}" | grep -q "concurrency:" && echo "${WF_CONTENT}" | grep -q "cancel-in-progress: true"; then
    pass "concurrency with cancel-in-progress"
else
    fail "concurrency with cancel-in-progress not found"
fi

# ── Test 20: T3 has Postgres service ──────────────────────────────────

log "Test 20: T3 job has Postgres service"
T3_BLOCK="$(job_block "t3-go-http-postgres")"
if echo "${T3_BLOCK}" | grep -q "services:" && echo "${T3_BLOCK}" | grep -q "postgres:"; then
    pass "T3 has Postgres service"
else
    fail "T3 missing Postgres service (but uses DATABASE_URL)"
fi

# ── Test 21: wasm-tools installs use caching ──────────────────────────

log "Test 21: wasm-tools installs use caching"
WASM_JOBS=("t4-ts-http-postgres" "t5-bun-http-postgres" "t6-multi-service")
ALL_WASM_CACHE=true
for job in "${WASM_JOBS[@]}"; do
    BLOCK="$(job_block "${job}")"
    if echo "${BLOCK}" | grep -q "wasm-tools"; then
        # Check that there's a cache step near wasm-tools, or cargo-binstall is used
        if echo "${BLOCK}" | grep -qE "cache.*wasm-tools|wasm-tools.*cache|cargo-binstall|wasm-tools-cache"; then
            true
        elif echo "${BLOCK}" | grep -B5 "cargo install wasm-tools" | grep -qE "actions/cache|cache.*cargo"; then
            true
        else
            # Check for a dedicated cache step for wasm-tools binary
            if echo "${BLOCK}" | grep -qE "key:.*wasm-tools"; then
                true
            else
                fail "${job} installs wasm-tools without caching"
                ALL_WASM_CACHE=false
            fi
        fi
    fi
done
if [ "${ALL_WASM_CACHE}" = true ]; then
    pass "wasm-tools installs use caching"
fi

# ── Test 22: Nightly schedule annotation in summary ───────────────────

log "Test 22: Nightly schedule context annotated in summary job"
if echo "${SUMMARY_BLOCK}" | grep -qiE "schedule|nightly|cron|github.event_name"; then
    pass "nightly schedule context annotated in summary"
else
    fail "summary job missing nightly schedule annotation"
fi

# ── Results ───────────────────────────────────────────────────────────

echo "" >&2
log "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"

if [ "${FAIL}" -gt 0 ]; then
    exit 1
fi
exit 0
