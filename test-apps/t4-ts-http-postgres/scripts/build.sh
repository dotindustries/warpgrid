#!/usr/bin/env bash
#
# build.sh — Build the T4 TypeScript HTTP+Postgres test handler into a Wasm component.
#
# Pipeline:
#   1. Run TypeScript type checking (npm run typecheck)
#   2. Componentize src/handler.js with jco into a WASI HTTP Wasm component
#   3. Validate the output with wasm-tools (if available)
#
# Prerequisites:
#   - Node.js 18+
#   - jco installed (run scripts/build-componentize-js.sh from project root first)
#
# Options:
#   --componentize-only   Skip type checking, only run componentization
#   --skip-validate       Skip wasm-tools validation step
#   --help                Show usage

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
PROJECT_ROOT="$(cd "${APP_DIR}/../.." && pwd)"
JCO_BIN="${PROJECT_ROOT}/build/componentize-js/node_modules/.bin/jco"
DIST_DIR="${APP_DIR}/dist"

log() { echo "==> $*" >&2; }
err() { echo "ERROR: $*" >&2; exit 1; }

usage() {
    echo "build.sh — Build T4 TypeScript HTTP+Postgres handler into Wasm component"
    echo
    echo "Options:"
    echo "  --componentize-only   Skip type checking"
    echo "  --skip-validate       Skip wasm-tools validation"
    echo "  --help                Show this help"
    exit 0
}

# ── Parse Args ────────────────────────────────────────────────────────────────

skip_typecheck=false
skip_validate=false

while [ $# -gt 0 ]; do
    case "$1" in
        --componentize-only) skip_typecheck=true ;;
        --skip-validate)     skip_validate=true ;;
        --help|-h)           usage ;;
        *)                   err "Unknown flag: $1" ;;
    esac
    shift
done

# ── Prerequisites ─────────────────────────────────────────────────────────────

if [ ! -x "${JCO_BIN}" ]; then
    err "jco not found at ${JCO_BIN}. Run 'scripts/build-componentize-js.sh' from project root first."
fi

# ── Type Checking ─────────────────────────────────────────────────────────────

if ! ${skip_typecheck}; then
    log "Running TypeScript type check..."
    (cd "${APP_DIR}" && npx tsc --noEmit) || err "TypeScript type checking failed"
    log "Type check: PASS"
fi

# ── Componentization ─────────────────────────────────────────────────────────

mkdir -p "${DIST_DIR}"

log "Componentizing handler.js → handler.wasm..."
"${JCO_BIN}" componentize \
    "${APP_DIR}/src/handler.js" \
    --wit "${APP_DIR}/wit/" \
    --world-name handler \
    --enable http \
    --enable fetch-event \
    -o "${DIST_DIR}/handler.wasm" \
    2>&1 || err "Componentization failed"

if [ ! -f "${DIST_DIR}/handler.wasm" ]; then
    err "handler.wasm was not produced"
fi

WASM_SIZE="$(wc -c < "${DIST_DIR}/handler.wasm" | tr -d ' ')"
log "Compiled handler.wasm: ${WASM_SIZE} bytes"

# ── Validation ────────────────────────────────────────────────────────────────

if ! ${skip_validate} && command -v wasm-tools &>/dev/null; then
    log "Validating component with wasm-tools..."
    if wasm-tools component wit "${DIST_DIR}/handler.wasm" > /dev/null 2>&1; then
        log "Component WIT validation: PASS"
    else
        log "Component WIT validation: could not extract WIT (may still be valid)"
    fi
fi

log "Build complete: ${DIST_DIR}/handler.wasm"
