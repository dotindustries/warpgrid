#!/usr/bin/env bash
#
# build.sh — Componentize the bun-json-api handler into a WASI Wasm component.
#
# US-605: Validates the `warp pack --lang bun` compilation pipeline with a
# realistic JSON API handler.
#
# Pipeline (mirrors `warp pack --lang bun`):
#   1. jco componentize handler.js → WASI HTTP Wasm component
#   2. wasm-tools validation (if available)
#
# Prerequisites:
#   - jco installed via scripts/build-componentize-js.sh
#   - wasm-tools (optional, for component validation)
#
# Output:
#   dist/handler.wasm — the compiled WASI HTTP component

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
JCO_BIN="${PROJECT_ROOT}/build/componentize-js/node_modules/.bin/jco"
DIST_DIR="${SCRIPT_DIR}/dist"

log() { echo "==> $*" >&2; }
err() { echo "ERROR: $*" >&2; exit 1; }

# ── Prerequisite checks ─────────────────────────────────────────────

if [ ! -x "${JCO_BIN}" ]; then
    log "jco not found. Building ComponentizeJS toolchain..."
    "${PROJECT_ROOT}/scripts/build-componentize-js.sh" --npm
fi

if [ ! -x "${JCO_BIN}" ]; then
    err "jco still not found at ${JCO_BIN}. Run: scripts/build-componentize-js.sh"
fi

HANDLER="${SCRIPT_DIR}/src/handler.js"
if [ ! -f "${HANDLER}" ]; then
    err "Handler not found: ${HANDLER}"
fi

# ── Componentize ─────────────────────────────────────────────────────

mkdir -p "${DIST_DIR}"

log "Componentizing ${HANDLER} → dist/handler.wasm ..."

if ! "${JCO_BIN}" componentize \
    "${HANDLER}" \
    --wit "${SCRIPT_DIR}/wit/" \
    --world-name "handler" \
    --enable http \
    --enable fetch-event \
    -o "${DIST_DIR}/handler.wasm" 2>&1; then
    err "Componentization failed. Check handler source and WIT definitions."
fi

if [ ! -f "${DIST_DIR}/handler.wasm" ]; then
    err "Componentization produced no output."
fi

WASM_SIZE="$(wc -c < "${DIST_DIR}/handler.wasm" | tr -d ' ')"
log "Component size: ${WASM_SIZE} bytes"

# ── Validate component ───────────────────────────────────────────────

if command -v wasm-tools &>/dev/null; then
    log "Validating component with wasm-tools..."
    if wasm-tools component wit "${DIST_DIR}/handler.wasm" > /dev/null 2>&1; then
        log "Component WIT validation: PASS"
    else
        log "Component WIT extraction: could not extract (may still be valid)"
    fi
fi

log "Build complete: ${DIST_DIR}/handler.wasm"
