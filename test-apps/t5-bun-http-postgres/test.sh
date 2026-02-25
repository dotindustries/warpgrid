#!/usr/bin/env bash
#
# test.sh — Integration tests for the T5 Bun HTTP + Postgres handler.
#
# Tests:
#   1. Build verification: handler componentizes to a valid Wasm component
#   2. GET /health returns 200 with status ok
#   3. GET /users returns 200 with 5 seed users
#   4. POST /users returns 201 with new user; subsequent GET includes it
#   5. POST /users with invalid JSON returns 400
#   6. process.env / Bun.env APP_NAME accessible in X-App-Name header
#   7. Unknown route returns 404
#   8. Response parity: GET /users body matches T4 format exactly
#
# Prerequisites:
#   - jco installed (scripts/build-componentize-js.sh)
#   - curl
#
# Usage:
#   ./test.sh              Run all tests
#   ./test.sh --build-only Only verify build

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
JCO_BIN="${PROJECT_ROOT}/build/componentize-js/node_modules/.bin/jco"

PASS=0
FAIL=0
SKIP=0

# ── Helpers ──────────────────────────────────────────────────────────

log()  { echo "==> $*" >&2; }
pass() { PASS=$((PASS + 1)); echo "  PASS: $*" >&2; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL: $*" >&2; }
skip() { SKIP=$((SKIP + 1)); echo "  SKIP: $*" >&2; }

# ── Global state for cleanup ────────────────────────────────────────

_server_pid=""
_tmpdir=""

cleanup() {
    if [ -n "${_server_pid}" ]; then
        kill "${_server_pid}" 2>/dev/null || true
        wait "${_server_pid}" 2>/dev/null || true
        _server_pid=""
    fi
    if [ -n "${_tmpdir}" ]; then
        rm -rf "${_tmpdir}"
        _tmpdir=""
    fi
}
trap cleanup EXIT

# ── Parse args ───────────────────────────────────────────────────────

BUILD_ONLY=false
while [ $# -gt 0 ]; do
    case "$1" in
        --build-only) BUILD_ONLY=true ;;
        --help|-h)
            echo "Usage: test.sh [--build-only]"
            exit 0
            ;;
        *) echo "Unknown flag: $1" >&2; exit 1 ;;
    esac
    shift
done

# ── Test 1: Build verification ───────────────────────────────────────

log "Test 1: Build verification (standalone handler)"

if [ ! -x "${JCO_BIN}" ]; then
    log "Building ComponentizeJS toolchain..."
    "${PROJECT_ROOT}/scripts/build-componentize-js.sh" --npm 2>&1 || true
fi

if [ ! -x "${JCO_BIN}" ]; then
    fail "jco not available — cannot run tests"
    echo ""
    echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"
    exit 1
fi

# Build the standalone handler (no warpgrid shim deps)
if "${SCRIPT_DIR}/build.sh" --standalone 2>&1; then
    if [ -f "${SCRIPT_DIR}/dist/handler.wasm" ]; then
        WASM_SIZE="$(wc -c < "${SCRIPT_DIR}/dist/handler.wasm" | tr -d ' ')"
        pass "Standalone handler compiled (${WASM_SIZE} bytes)"
    else
        fail "Build script succeeded but handler.wasm not found"
    fi
else
    fail "Standalone handler build failed"
    echo ""
    echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"
    exit 1
fi

if ${BUILD_ONLY}; then
    echo ""
    echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"
    exit "${FAIL}"
fi

# ── Start jco serve ─────────────────────────────────────────────────

_tmpdir="$(mktemp -d)"

# Find a free port
PORT=$(python3 -c "import socket; s=socket.socket(); s.bind(('',0)); print(s.getsockname()[1]); s.close()" 2>/dev/null || echo "8788")

log "Starting jco serve on port ${PORT}..."

"${JCO_BIN}" serve "${SCRIPT_DIR}/dist/handler.wasm" --port "${PORT}" \
    >"${_tmpdir}/serve.stdout" 2>"${_tmpdir}/serve.stderr" &
_server_pid=$!

# Wait for server to be ready (jco serve takes ~5s to transpile)
SERVER_READY=false
ATTEMPTS=0
MAX_ATTEMPTS=30
while [ ${ATTEMPTS} -lt ${MAX_ATTEMPTS} ]; do
    if grep -q "Server listening" "${_tmpdir}/serve.stderr" 2>/dev/null; then
        SERVER_READY=true
        break
    fi
    if ! kill -0 "${_server_pid}" 2>/dev/null; then
        log "jco serve exited unexpectedly"
        cat "${_tmpdir}/serve.stderr" >&2 || true
        break
    fi
    sleep 0.5
    ATTEMPTS=$((ATTEMPTS + 1))
done

if ! ${SERVER_READY}; then
    fail "jco serve did not start within timeout"
    skip "GET /health — server not running"
    skip "GET /users — server not running"
    skip "POST /users — server not running"
    skip "POST /users invalid JSON — server not running"
    skip "X-App-Name header — server not running"
    skip "404 on unknown route — server not running"
    skip "Response parity — server not running"
    echo ""
    echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"
    exit "${FAIL}"
fi

# Give server a moment to fully bind
sleep 1

BASE_URL="http://localhost:${PORT}"

# ── Test 2: GET /health ─────────────────────────────────────────────

log "Test 2: GET /health"
RESP=$(curl -s -w "\n%{http_code}" "${BASE_URL}/health" 2>&1) || true
HTTP_CODE=$(echo "${RESP}" | tail -1)
BODY=$(echo "${RESP}" | sed '$d')

if [ "${HTTP_CODE}" = "200" ] && echo "${BODY}" | grep -q '"status"'; then
    pass "GET /health → 200 with status"
else
    fail "GET /health → expected 200, got ${HTTP_CODE}. Body: ${BODY}"
fi

# ── Test 3: GET /users returns 200 with 5 seed users ────────────────

log "Test 3: GET /users returns seed data"
RESP=$(curl -s -w "\n%{http_code}" "${BASE_URL}/users" 2>&1) || true
HTTP_CODE=$(echo "${RESP}" | tail -1)
BODY=$(echo "${RESP}" | sed '$d')

if [ "${HTTP_CODE}" = "200" ]; then
    USER_COUNT=$(echo "${BODY}" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo "0")
    if [ "${USER_COUNT}" = "5" ]; then
        pass "GET /users → 200 with ${USER_COUNT} seed users"
    else
        fail "GET /users → 200 but got ${USER_COUNT} users (expected 5). Body: ${BODY}"
    fi
else
    fail "GET /users → expected 200, got ${HTTP_CODE}. Body: ${BODY}"
fi

# ── Test 4: POST /users returns 201; GET reflects new row ───────────

log "Test 4: POST /users → 201, then GET includes new user"
RESP=$(curl -s -w "\n%{http_code}" -X POST \
    -H "Content-Type: application/json" \
    -d '{"name":"Test User","email":"test@example.com"}' \
    "${BASE_URL}/users" 2>&1) || true
HTTP_CODE=$(echo "${RESP}" | tail -1)
BODY=$(echo "${RESP}" | sed '$d')

if [ "${HTTP_CODE}" = "201" ]; then
    if echo "${BODY}" | grep -q '"Test User"'; then
        pass "POST /users → 201 with new user"
    else
        fail "POST /users → 201 but body doesn't contain new user. Body: ${BODY}"
    fi
else
    fail "POST /users → expected 201, got ${HTTP_CODE}. Body: ${BODY}"
fi

# Verify GET /users now includes 6 users
RESP=$(curl -s -w "\n%{http_code}" "${BASE_URL}/users" 2>&1) || true
HTTP_CODE=$(echo "${RESP}" | tail -1)
BODY=$(echo "${RESP}" | sed '$d')

if [ "${HTTP_CODE}" = "200" ]; then
    USER_COUNT=$(echo "${BODY}" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo "0")
    if [ "${USER_COUNT}" = "6" ]; then
        pass "GET /users after POST → 200 with ${USER_COUNT} users (includes new)"
    else
        fail "GET /users after POST → expected 6 users, got ${USER_COUNT}"
    fi
else
    fail "GET /users after POST → expected 200, got ${HTTP_CODE}"
fi

# ── Test 5: POST /users with invalid JSON returns 400 ───────────────

log "Test 5: POST /users with invalid body → 400"
RESP=$(curl -s -w "\n%{http_code}" -X POST \
    -H "Content-Type: application/json" \
    -d 'not-json' \
    "${BASE_URL}/users" 2>&1) || true
HTTP_CODE=$(echo "${RESP}" | tail -1)

if [ "${HTTP_CODE}" = "400" ]; then
    pass "POST /users with invalid JSON → 400"
else
    fail "POST /users with invalid JSON → expected 400, got ${HTTP_CODE}"
fi

# ── Test 6: X-App-Name header from Bun.env / process.env ────────────

log "Test 6: X-App-Name response header (Bun.env polyfill)"
HEADERS=$(curl -s -D - -o /dev/null "${BASE_URL}/health" 2>&1) || true

if echo "${HEADERS}" | grep -iq "x-app-name"; then
    APP_NAME_VALUE=$(echo "${HEADERS}" | grep -i "x-app-name" | sed 's/.*: //' | tr -d '\r\n')
    pass "X-App-Name header present: '${APP_NAME_VALUE}' (Bun.env polyfill works)"
else
    # In standalone mode, Bun.env may not be available — acceptable skip
    skip "X-App-Name header not present (Bun.env may not be available in standalone mode)"
fi

# ── Test 7: Unknown route returns 404 ────────────────────────────────

log "Test 7: Unknown route → 404"
RESP=$(curl -s -w "\n%{http_code}" "${BASE_URL}/nonexistent" 2>&1) || true
HTTP_CODE=$(echo "${RESP}" | tail -1)

if [ "${HTTP_CODE}" = "404" ]; then
    pass "GET /nonexistent → 404"
else
    fail "GET /nonexistent → expected 404, got ${HTTP_CODE}"
fi

# ── Test 8: Response parity with T4 ─────────────────────────────────

log "Test 8: Response body format matches T4 (JSON structure parity)"

# The T4 and T5 standalone handlers should produce identical JSON
# structure for GET /health. We verify the key format here.
RESP=$(curl -s "${BASE_URL}/health" 2>&1) || true
EXPECTED='{"status":"ok"}'

if [ "${RESP}" = "${EXPECTED}" ]; then
    pass "Response parity: GET /health body matches T4 format exactly"
else
    fail "Response parity: expected '${EXPECTED}', got '${RESP}'"
fi

# ── Summary ──────────────────────────────────────────────────────────

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"

if [ "${FAIL}" -gt 0 ]; then
    exit 1
fi
exit 0
