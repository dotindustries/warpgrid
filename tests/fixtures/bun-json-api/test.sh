#!/usr/bin/env bash
#
# test.sh — Integration tests for the bun-json-api handler.
#
# US-605: Validates that the realistic Bun handler compiles via the
# `warp pack --lang bun` pipeline and produces correct HTTP responses.
#
# Tests:
#   1. Build verification: handler componentizes to a valid Wasm component
#   2. POST /transform with valid JSON → 200, correct transformation
#   3. POST /transform with invalid JSON → 400, error message
#   4. POST /transform missing required fields → 400
#   5. POST /validate with valid schema → 200
#   6. GET /health → 200, {"success":true,"data":{"status":"ok"}}
#   7. GET /nonexistent → 404
#   8. Content-Type header verification
#
# Note: jco serve's fetch-event transpilation only supports one request
# per module lifetime, so each HTTP test starts a fresh server instance.
#
# Prerequisites:
#   - jco installed (scripts/build-componentize-js.sh)
#   - curl, python3 (for JSON parsing and port allocation)
#
# Usage:
#   ./test.sh              Run all tests
#   ./test.sh --build-only Only verify build

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
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

# Start a fresh jco serve instance on a random port.
start_jco_serve() {
    if [ -n "${_server_pid}" ]; then
        kill "${_server_pid}" 2>/dev/null || true
        wait "${_server_pid}" 2>/dev/null || true
        _server_pid=""
    fi

    local port
    port=$(python3 -c "import socket; s=socket.socket(); s.bind(('',0)); print(s.getsockname()[1]); s.close()" 2>/dev/null || echo "8789")

    "${JCO_BIN}" serve "${SCRIPT_DIR}/dist/handler.wasm" --port "${port}" \
        >"${_tmpdir}/serve.stdout" 2>"${_tmpdir}/serve.stderr" &
    _server_pid=$!

    local attempts=0
    local max_attempts=60
    while [ ${attempts} -lt ${max_attempts} ]; do
        if grep -q "Server listening" "${_tmpdir}/serve.stderr" 2>/dev/null; then
            sleep 1
            echo "${port}"
            return 0
        fi
        if ! kill -0 "${_server_pid}" 2>/dev/null; then
            return 1
        fi
        sleep 0.5
        attempts=$((attempts + 1))
    done

    return 1
}

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

# ── Test 1: Build verification ────────────────────────────────────────

log "Test 1: Build verification"

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

if "${SCRIPT_DIR}/build.sh" 2>&1; then
    if [ -f "${SCRIPT_DIR}/dist/handler.wasm" ]; then
        WASM_SIZE="$(wc -c < "${SCRIPT_DIR}/dist/handler.wasm" | tr -d ' ')"
        pass "Handler compiled (${WASM_SIZE} bytes)"
    else
        fail "Build script succeeded but handler.wasm not found"
    fi
else
    fail "Handler build failed"
    echo ""
    echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"
    exit 1
fi

if ${BUILD_ONLY}; then
    echo ""
    echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"
    exit "${FAIL}"
fi

_tmpdir="$(mktemp -d)"

# ── Test 2: POST /transform with valid JSON ───────────────────────────

log "Test 2: POST /transform → 200 with transformed data"

if PORT=$(start_jco_serve); then
    RESP=$(curl -s --max-time 30 -w "\n%{http_code}" -X POST \
        -H "Content-Type: application/json" \
        -d '{"name":"alice johnson","email":"alice@example.com","tags":["dev","go"]}' \
        "http://localhost:${PORT}/transform" 2>&1) || true
    HTTP_CODE=$(echo "${RESP}" | tail -1)
    BODY=$(echo "${RESP}" | sed '$d')

    if [ "${HTTP_CODE}" = "200" ]; then
        # Verify transformation results
        DISPLAY_NAME=$(echo "${BODY}" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['displayName'])" 2>/dev/null || echo "")
        SLUG=$(echo "${BODY}" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['slug'])" 2>/dev/null || echo "")
        DOMAIN=$(echo "${BODY}" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['emailDomain'])" 2>/dev/null || echo "")
        TAG_COUNT=$(echo "${BODY}" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['tagCount'])" 2>/dev/null || echo "")

        if [ "${DISPLAY_NAME}" = "ALICE JOHNSON" ] && [ "${SLUG}" = "alice-johnson" ] && [ "${DOMAIN}" = "example.com" ] && [ "${TAG_COUNT}" = "2" ]; then
            pass "POST /transform → 200 with correct transformation (displayName=${DISPLAY_NAME}, slug=${SLUG}, domain=${DOMAIN}, tagCount=${TAG_COUNT})"
        else
            fail "POST /transform → 200 but data mismatch: displayName='${DISPLAY_NAME}', slug='${SLUG}', domain='${DOMAIN}', tagCount='${TAG_COUNT}'"
        fi
    else
        fail "POST /transform → expected 200, got ${HTTP_CODE}. Body: ${BODY}"
    fi
else
    fail "jco serve did not start for /transform test"
fi

# ── Test 3: POST /transform with invalid JSON → 400 ──────────────────

log "Test 3: POST /transform with invalid JSON → 400"

if PORT=$(start_jco_serve); then
    RESP=$(curl -s --max-time 30 -w "\n%{http_code}" -X POST \
        -H "Content-Type: application/json" \
        -d 'not-json{{{' \
        "http://localhost:${PORT}/transform" 2>&1) || true
    HTTP_CODE=$(echo "${RESP}" | tail -1)
    BODY=$(echo "${RESP}" | sed '$d')

    if [ "${HTTP_CODE}" = "400" ]; then
        SUCCESS=$(echo "${BODY}" | python3 -c "import sys,json; print(json.load(sys.stdin)['success'])" 2>/dev/null || echo "")
        if [ "${SUCCESS}" = "False" ]; then
            pass "POST /transform with invalid JSON → 400 with success=false"
        else
            fail "POST /transform with invalid JSON → 400 but success is not false. Body: ${BODY}"
        fi
    else
        fail "POST /transform with invalid JSON → expected 400, got ${HTTP_CODE}. Body: ${BODY}"
    fi
else
    fail "jco serve did not start for invalid JSON test"
fi

# ── Test 4: POST /transform missing required fields → 400 ────────────

log "Test 4: POST /transform missing fields → 400"

if PORT=$(start_jco_serve); then
    RESP=$(curl -s --max-time 30 -w "\n%{http_code}" -X POST \
        -H "Content-Type: application/json" \
        -d '{"tags":["orphan"]}' \
        "http://localhost:${PORT}/transform" 2>&1) || true
    HTTP_CODE=$(echo "${RESP}" | tail -1)
    BODY=$(echo "${RESP}" | sed '$d')

    if [ "${HTTP_CODE}" = "400" ]; then
        ERROR=$(echo "${BODY}" | python3 -c "import sys,json; print(json.load(sys.stdin).get('error',''))" 2>/dev/null || echo "")
        if echo "${ERROR}" | grep -q "name"; then
            pass "POST /transform missing fields → 400 with field-specific error"
        else
            fail "POST /transform missing fields → 400 but error doesn't mention 'name'. Error: ${ERROR}"
        fi
    else
        fail "POST /transform missing fields → expected 400, got ${HTTP_CODE}. Body: ${BODY}"
    fi
else
    fail "jco serve did not start for missing fields test"
fi

# ── Test 5: POST /validate with valid user schema ─────────────────────

log "Test 5: POST /validate → 200 with valid=true"

if PORT=$(start_jco_serve); then
    RESP=$(curl -s --max-time 30 -w "\n%{http_code}" -X POST \
        -H "Content-Type: application/json" \
        -d '{"schema":"user","payload":{"name":"Alice","email":"alice@test.com","age":30}}' \
        "http://localhost:${PORT}/validate" 2>&1) || true
    HTTP_CODE=$(echo "${RESP}" | tail -1)
    BODY=$(echo "${RESP}" | sed '$d')

    if [ "${HTTP_CODE}" = "200" ]; then
        VALID=$(echo "${BODY}" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['valid'])" 2>/dev/null || echo "")
        if [ "${VALID}" = "True" ]; then
            pass "POST /validate → 200 with valid=true"
        else
            fail "POST /validate → 200 but valid is not true. Body: ${BODY}"
        fi
    else
        fail "POST /validate → expected 200, got ${HTTP_CODE}. Body: ${BODY}"
    fi
else
    fail "jco serve did not start for /validate test"
fi

# ── Test 6: GET /health → 200 ─────────────────────────────────────────

log "Test 6: GET /health → 200"

if PORT=$(start_jco_serve); then
    RESP=$(curl -s --max-time 30 -w "\n%{http_code}" "http://localhost:${PORT}/health" 2>&1) || true
    HTTP_CODE=$(echo "${RESP}" | tail -1)
    BODY=$(echo "${RESP}" | sed '$d')

    if [ "${HTTP_CODE}" = "200" ]; then
        STATUS=$(echo "${BODY}" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['status'])" 2>/dev/null || echo "")
        if [ "${STATUS}" = "ok" ]; then
            pass "GET /health → 200 with status=ok"
        else
            fail "GET /health → 200 but status is not 'ok'. Body: ${BODY}"
        fi
    else
        fail "GET /health → expected 200, got ${HTTP_CODE}. Body: ${BODY}"
    fi
else
    fail "jco serve did not start for /health test"
fi

# ── Test 7: Unknown route → 404 ───────────────────────────────────────

log "Test 7: Unknown route → 404"

if PORT=$(start_jco_serve); then
    RESP=$(curl -s --max-time 30 -w "\n%{http_code}" "http://localhost:${PORT}/nonexistent" 2>&1) || true
    HTTP_CODE=$(echo "${RESP}" | tail -1)

    if [ "${HTTP_CODE}" = "404" ]; then
        pass "GET /nonexistent → 404"
    else
        fail "GET /nonexistent → expected 404, got ${HTTP_CODE}"
    fi
else
    fail "jco serve did not start for 404 test"
fi

# ── Test 8: Content-Type header check ─────────────────────────────────

log "Test 8: Content-Type: application/json header"

if PORT=$(start_jco_serve); then
    CT=$(curl -s --max-time 30 -o /dev/null -w "%{content_type}" "http://localhost:${PORT}/health" 2>&1) || true

    if echo "${CT}" | grep -qi "application/json"; then
        pass "Content-Type header is application/json"
    else
        fail "Content-Type expected application/json, got '${CT}'"
    fi
else
    fail "jco serve did not start for Content-Type test"
fi

# ── Summary ──────────────────────────────────────────────────────────

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"

if [ "${FAIL}" -gt 0 ]; then
    exit 1
fi
exit 0
