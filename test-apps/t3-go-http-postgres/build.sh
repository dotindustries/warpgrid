#!/usr/bin/env bash
#
# build.sh — Compile the T3 Go HTTP handler for WarpGrid.
#
# Modes:
#   --standalone  Build and run Go unit tests only (no TinyGo/Wasm compilation)
#   --wasm        Compile to Wasm component via TinyGo wasip2 (requires TinyGo)
#   (default)     Same as --standalone
#
# Prerequisites:
#   - Go 1.22+ (for unit tests)
#   - TinyGo 0.40+ (for --wasm mode, installed via scripts/build-tinygo.sh)
#
# Output:
#   dist/handler.wasm — the compiled WASI component (--wasm mode only)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
TINYGO_BIN="${PROJECT_ROOT}/build/tinygo/bin/tinygo"
DIST_DIR="${SCRIPT_DIR}/dist"

log() { echo "==> $*" >&2; }
err() { echo "ERROR: $*" >&2; exit 1; }

# ── Parse args ───────────────────────────────────────────────────────

MODE="standalone"
while [ $# -gt 0 ]; do
    case "$1" in
        --standalone) MODE="standalone" ;;
        --wasm) MODE="wasm" ;;
        --help|-h)
            echo "Usage: build.sh [--standalone|--wasm]"
            echo "  --standalone  Run Go unit tests (default)"
            echo "  --wasm        Compile to Wasm component via TinyGo wasip2"
            exit 0
            ;;
        *) err "Unknown flag: $1" ;;
    esac
    shift
done

# ── Standalone mode: Go unit tests ──────────────────────────────────

if [ "${MODE}" = "standalone" ]; then
    log "Building standalone (Go unit tests)..."

    if ! command -v go &>/dev/null; then
        err "Go not found. Install Go 1.22+ from https://go.dev"
    fi

    log "Running go vet..."
    if ! go vet "${SCRIPT_DIR}/..."; then
        err "go vet failed"
    fi

    log "Running go test..."
    if ! go test -v -count=1 "${SCRIPT_DIR}/..."; then
        err "go test failed"
    fi

    log "Standalone build complete: all Go tests pass"
    exit 0
fi

# ── Wasm mode: TinyGo compilation ───────────────────────────────────

log "Building Wasm component (TinyGo wasip2)..."

if [ ! -x "${TINYGO_BIN}" ]; then
    log "TinyGo not found. Building TinyGo toolchain..."
    "${PROJECT_ROOT}/scripts/build-tinygo.sh" 2>&1 || true
fi

if [ ! -x "${TINYGO_BIN}" ]; then
    err "TinyGo not found at ${TINYGO_BIN}. Run: scripts/build-tinygo.sh"
fi

mkdir -p "${DIST_DIR}"

log "Compiling ${SCRIPT_DIR}/main.go → dist/handler.wasm ..."

# TinyGo wasip2 produces a WASI Preview 2 component.
# When US-306/307 (warpgrid/net/http overlay) is ready, this will
# produce a full wasi:http/incoming-handler component.
if ! "${TINYGO_BIN}" build \
    -target=wasip2 \
    -o "${DIST_DIR}/handler.wasm" \
    "${SCRIPT_DIR}/main.go" 2>&1; then
    err "TinyGo compilation failed."
fi

if [ ! -f "${DIST_DIR}/handler.wasm" ]; then
    err "TinyGo produced no output."
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
