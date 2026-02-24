#!/usr/bin/env bash
#
# build.sh — Componentize the T4 TypeScript HTTP handler into a WASI Wasm component.
#
# Modes:
#   (default)     Build the full handler with warpgrid:shim imports (requires US-403/404)
#   --standalone  Build the standalone handler (in-memory data, no shim deps)
#
# Prerequisites:
#   - jco installed via scripts/build-componentize-js.sh
#   - wasm-tools (for component validation)
#
# Output:
#   dist/handler.wasm — the compiled WASI HTTP component

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
JCO_BIN="${PROJECT_ROOT}/build/componentize-js/node_modules/.bin/jco"
DIST_DIR="${SCRIPT_DIR}/dist"

log() { echo "==> $*" >&2; }
err() { echo "ERROR: $*" >&2; exit 1; }

# ── Parse args ───────────────────────────────────────────────────────

MODE="full"
while [ $# -gt 0 ]; do
    case "$1" in
        --standalone) MODE="standalone" ;;
        --help|-h)
            echo "Usage: build.sh [--standalone]"
            echo "  (default)     Build full handler (requires warpgrid shim bridge)"
            echo "  --standalone  Build standalone handler (in-memory data)"
            exit 0
            ;;
        *) err "Unknown flag: $1" ;;
    esac
    shift
done

# ── Prerequisite checks ─────────────────────────────────────────────

if [ ! -x "${JCO_BIN}" ]; then
    log "jco not found. Building ComponentizeJS toolchain..."
    "${PROJECT_ROOT}/scripts/build-componentize-js.sh" --npm
fi

if [ ! -x "${JCO_BIN}" ]; then
    err "jco still not found at ${JCO_BIN}. Run: scripts/build-componentize-js.sh"
fi

# ── Select handler and WIT ───────────────────────────────────────────

if [ "${MODE}" = "standalone" ]; then
    HANDLER="${SCRIPT_DIR}/src/handler-standalone.js"
    WORLD_NAME="handler-standalone"
    log "Building standalone handler (in-memory data)..."
else
    HANDLER="${SCRIPT_DIR}/src/handler.js"
    WORLD_NAME="handler"
    log "Building full handler (warpgrid:shim database proxy)..."
fi

if [ ! -f "${HANDLER}" ]; then
    err "Handler not found: ${HANDLER}"
fi

# ── Componentize ─────────────────────────────────────────────────────

mkdir -p "${DIST_DIR}"

log "Componentizing ${HANDLER} → dist/handler.wasm ..."

if ! "${JCO_BIN}" componentize \
    "${HANDLER}" \
    --wit "${SCRIPT_DIR}/wit/" \
    --world-name "${WORLD_NAME}" \
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
