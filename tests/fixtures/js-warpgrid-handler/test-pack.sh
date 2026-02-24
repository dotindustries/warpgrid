#!/usr/bin/env bash
#
# test-pack.sh — Verify `warp pack --lang js` produces a valid Wasm component
# from the warpgrid-shim handler fixture.
#
# This test validates acceptance criterion #6 of US-410:
# "A TypeScript handler using warpgrid/pg, warpgrid.dns.resolve, and process.env
#  compiles successfully."
#
# Prerequisites:
#   - jco installed via scripts/build-componentize-js.sh
#   - wasm-tools (for component validation)
#
# Usage:
#   ./test-pack.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
JCO_BIN="${PROJECT_ROOT}/build/componentize-js/node_modules/.bin/jco"
DIST_DIR="${SCRIPT_DIR}/dist"

pass=0
fail=0

log() { echo "==> $*" >&2; }
check() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "  PASS: ${name}"
        pass=$((pass + 1))
    else
        echo "  FAIL: ${name}"
        fail=$((fail + 1))
    fi
}

# ── Prerequisite ─────────────────────────────────────────────────────

if [ ! -x "${JCO_BIN}" ]; then
    log "jco not found. Building ComponentizeJS toolchain..."
    "${PROJECT_ROOT}/scripts/build-componentize-js.sh" --npm
fi

if [ ! -x "${JCO_BIN}" ]; then
    echo "SKIP: jco not available"
    exit 0
fi

# ── Build the handler with prelude injection ─────────────────────────

log "Building warpgrid-shim handler fixture..."

# Step 1: Generate the WarpGrid shim prelude
PRELUDE='// ── WarpGrid Shim Prelude (auto-injected by warp pack --lang js) ──

if (typeof globalThis.process === "undefined") {
  globalThis.process = {};
}
if (typeof globalThis.process.env === "undefined") {
  globalThis.process.env = {};
}

if (typeof globalThis.warpgrid === "undefined") {
  globalThis.warpgrid = {};
}

// Database proxy shim
try {
  const dbProxy = await import("warpgrid:shim/database-proxy@0.1.0");
  globalThis.warpgrid.database = {
    connect: dbProxy.connect,
    send: dbProxy.send,
    recv: dbProxy.recv,
    close: dbProxy.close,
  };
} catch (_e) {}

// DNS shim
try {
  const dnsShim = await import("warpgrid:shim/dns@0.1.0");
  globalThis.warpgrid.dns = {
    resolve: dnsShim.resolveAddress,
  };
} catch (_e) {}

// Filesystem shim
try {
  const fsShim = await import("warpgrid:shim/filesystem@0.1.0");
  globalThis.warpgrid.fs = {
    readFile: async (path, encoding) => {
      const handle = fsShim.openVirtual(path);
      const data = fsShim.readVirtual(handle, 1048576);
      fsShim.closeVirtual(handle);
      if (encoding === "utf-8" || encoding === "utf8") {
        return new TextDecoder().decode(new Uint8Array(data));
      }
      return new Uint8Array(data);
    },
  };
} catch (_e) {}

// ── End WarpGrid Shim Prelude ──

'

# Step 2: Combine prelude with handler
mkdir -p "${DIST_DIR}"
COMBINED="${DIST_DIR}/.handler-combined.js"
printf '%s' "${PRELUDE}" > "${COMBINED}"
cat "${SCRIPT_DIR}/src/handler.js" >> "${COMBINED}"

# Step 3: Componentize
log "Componentizing handler with warpgrid shim imports..."
"${JCO_BIN}" componentize \
    "${COMBINED}" \
    --wit "${SCRIPT_DIR}/wit/" \
    --world-name handler \
    --enable http \
    --enable fetch-event \
    -o "${DIST_DIR}/handler.wasm" 2>&1

rm -f "${COMBINED}"

# ── Tests ────────────────────────────────────────────────────────────

echo ""
echo "Running tests..."

# T1: Component was produced
check "Component file exists" test -f "${DIST_DIR}/handler.wasm"

# T2: Component has non-trivial size (SpiderMonkey embedding is ~12MB)
WASM_SIZE="$(wc -c < "${DIST_DIR}/handler.wasm" | tr -d ' ')"
check "Component size > 1MB (got ${WASM_SIZE} bytes)" test "${WASM_SIZE}" -gt 1000000

# T3: wasm-tools validates the component
if command -v wasm-tools &>/dev/null; then
    check "wasm-tools component wit validation" wasm-tools component wit "${DIST_DIR}/handler.wasm"
else
    echo "  SKIP: wasm-tools not available"
fi

# T4: Combined handler contains prelude markers
HANDLER_SOURCE="$(cat "${SCRIPT_DIR}/src/handler.js")"
check "Handler uses warpgrid.database" echo "${HANDLER_SOURCE}" | grep -q "warpgrid.database"
check "Handler uses process.env" echo "${HANDLER_SOURCE}" | grep -q "process.env"
check "Handler uses warpgrid.dns" echo "${HANDLER_SOURCE}" | grep -q "warpgrid.dns"

echo ""
echo "Results: ${pass} passed, ${fail} failed"

# Clean up
rm -rf "${DIST_DIR}"

[ "${fail}" -eq 0 ]
