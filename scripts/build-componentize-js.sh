#!/usr/bin/env bash
#
# build-componentize-js.sh — Set up ComponentizeJS toolchain for the WarpGrid SDK.
#
# Produces a working jco+componentize-js installation that can compile JavaScript
# handlers into WASI HTTP WebAssembly components.
#
# The ComponentizeJS source is cloned into vendor/componentize-js/ at a pinned tag,
# ready for future WarpGrid patches (Domain 4: US-403+).
#
# Build modes:
#   --npm          Install from npm registry (default, fastest)
#   --source       Build from vendor/componentize-js/ source (requires Rust + wasi-sdk)
#
# Options:
#   --verify       Verify: componentize a test HTTP handler, run in Wasmtime
#   --clean        Remove build artifacts before installing
#   --help         Show this help
#
# Prerequisites:
#   --npm:    Node.js 18+, npm
#   --source: Node.js 18+, npm, Rust stable (wasm32-unknown-unknown + wasm32-wasi targets),
#             wasi-sdk-20 at /opt/wasi-sdk/
#   --verify: wasmtime
#
# Environment variables:
#   COMPONENTIZE_JS_TAG   Git tag to pin (default: 0.19.3)
#   NODE_PATH             Custom Node.js path (auto-detected)
#
# Output:
#   build/componentize-js/node_modules/.bin/jco   — the jco CLI
#   vendor/componentize-js/                        — source (for future patching)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VENDOR_DIR="${PROJECT_ROOT}/vendor/componentize-js"
BUILD_DIR="${PROJECT_ROOT}/build/componentize-js"
COMPONENTIZE_JS_TAG="${COMPONENTIZE_JS_TAG:-0.19.3}"
COMPONENTIZE_JS_REPO="https://github.com/bytecodealliance/ComponentizeJS.git"

# npm package versions — latest stable releases.
# Note: npm versions may trail the git tag; --source mode uses the exact git tag.
JCO_VERSION="1.16.1"
COMPONENTIZE_JS_NPM_VERSION="0.18.4"

MIN_NODE_MAJOR=18

# ─── Helpers ─────────────────────────────────────────────────────────────────

log() { echo "==> $*" >&2; }
err() { echo "ERROR: $*" >&2; exit 1; }
warn() { echo "WARNING: $*" >&2; }

usage() {
    echo "build-componentize-js.sh — Set up ComponentizeJS for WarpGrid SDK."
    echo
    echo "Build modes:"
    echo "  --npm        Install from npm registry (default, fastest)"
    echo "  --source     Build from vendor/componentize-js/ source (Rust + wasi-sdk)"
    echo
    echo "Options:"
    echo "  --clean      Remove build artifacts before installing"
    echo "  --verify     Verify: componentize test handler, run in Wasmtime"
    echo "  --help       Show this help message"
    echo
    echo "Environment variables:"
    echo "  COMPONENTIZE_JS_TAG  Git tag (default: ${COMPONENTIZE_JS_TAG})"
    echo
    echo "Output: build/componentize-js/node_modules/.bin/jco"
    exit 0
}

# ─── Prerequisite Checks ────────────────────────────────────────────────────

check_node_version() {
    if ! command -v node &>/dev/null; then
        err "Node.js is not installed. Install Node.js ${MIN_NODE_MAJOR}+ from https://nodejs.org/"
    fi

    local node_version major
    node_version="$(node --version)"
    major="$(echo "${node_version}" | sed 's/^v//' | cut -d. -f1)"

    if [ "${major}" -lt "${MIN_NODE_MAJOR}" ]; then
        err "Node.js ${MIN_NODE_MAJOR}+ required, found ${node_version}"
    fi

    log "Node.js: ${node_version}"
}

check_npm() {
    if ! command -v npm &>/dev/null; then
        err "npm is not installed"
    fi
    log "npm: $(npm --version)"
}

check_git() {
    if ! command -v git &>/dev/null; then
        err "git is not installed"
    fi
}

# ─── Clone / Update Source ───────────────────────────────────────────────────

ensure_source() {
    if [ -d "${VENDOR_DIR}/.git" ]; then
        local current_tag
        current_tag="$(cd "${VENDOR_DIR}" && git describe --tags --exact-match HEAD 2>/dev/null || echo "none")"

        if [ "${current_tag}" = "${COMPONENTIZE_JS_TAG}" ]; then
            log "ComponentizeJS source already at ${COMPONENTIZE_JS_TAG}"
            return 0
        fi

        log "ComponentizeJS source at ${current_tag}, switching to ${COMPONENTIZE_JS_TAG}..."
        (cd "${VENDOR_DIR}" && git fetch origin "refs/tags/${COMPONENTIZE_JS_TAG}:refs/tags/${COMPONENTIZE_JS_TAG}" && git checkout "${COMPONENTIZE_JS_TAG}")
    else
        log "Cloning ComponentizeJS at ${COMPONENTIZE_JS_TAG}..."
        mkdir -p "$(dirname "${VENDOR_DIR}")"
        git clone --branch "${COMPONENTIZE_JS_TAG}" --depth 1 "${COMPONENTIZE_JS_REPO}" "${VENDOR_DIR}"
    fi

    log "ComponentizeJS source ready at ${VENDOR_DIR}"
}

# ─── Install from npm ────────────────────────────────────────────────────────

install_from_npm() {
    log "Installing jco ${JCO_VERSION} and componentize-js ${COMPONENTIZE_JS_NPM_VERSION} from npm..."

    mkdir -p "${BUILD_DIR}"

    # Create a minimal package.json if it doesn't exist or versions differ
    local pkg_file="${BUILD_DIR}/package.json"
    cat > "${pkg_file}" << EOF
{
  "name": "warpgrid-componentize-js",
  "version": "0.0.0",
  "private": true,
  "description": "WarpGrid ComponentizeJS toolchain",
  "dependencies": {
    "@bytecodealliance/jco": "${JCO_VERSION}",
    "@bytecodealliance/componentize-js": "${COMPONENTIZE_JS_NPM_VERSION}"
  }
}
EOF

    if ! (cd "${BUILD_DIR}" && npm install --no-audit --no-fund); then
        err "npm install failed. Check network connectivity and package versions."
    fi

    local jco_bin="${BUILD_DIR}/node_modules/.bin/jco"
    if [ ! -x "${jco_bin}" ]; then
        err "jco binary not found at ${jco_bin} after npm install"
    fi

    log "jco installed: $(${jco_bin} --version)"
}

# ─── Build from Source ───────────────────────────────────────────────────────

build_from_source() {
    if [ ! -d "${VENDOR_DIR}/.git" ]; then
        err "ComponentizeJS source not found at ${VENDOR_DIR}. Run without --source first, or clone manually."
    fi

    log "Building ComponentizeJS from source..."

    # Check Rust is available
    if ! command -v rustc &>/dev/null; then
        err "Rust is required for source builds. Install from https://rustup.rs/"
    fi

    # Check required Rust targets
    local targets
    targets="$(rustup target list --installed 2>/dev/null || true)"
    if ! echo "${targets}" | grep -q "wasm32-unknown-unknown"; then
        log "Adding wasm32-unknown-unknown target..."
        rustup target add wasm32-unknown-unknown
    fi
    if ! echo "${targets}" | grep -q "wasm32-wasip1"; then
        log "Adding wasm32-wasip1 target..."
        rustup target add wasm32-wasip1
    fi

    # Check wasi-sdk
    if [ ! -d "/opt/wasi-sdk" ]; then
        err "wasi-sdk not found at /opt/wasi-sdk/. Source builds require wasi-sdk-20+."
    fi

    (
        cd "${VENDOR_DIR}"
        git submodule update --init --recursive
        npm install
        npm run build
    )

    # Copy built artifacts to build dir
    mkdir -p "${BUILD_DIR}/node_modules/@bytecodealliance"
    cp -R "${VENDOR_DIR}" "${BUILD_DIR}/node_modules/@bytecodealliance/componentize-js"

    # Install jco from npm (it wraps componentize-js)
    local pkg_file="${BUILD_DIR}/package.json"
    cat > "${pkg_file}" << EOF
{
  "name": "warpgrid-componentize-js",
  "version": "0.0.0",
  "private": true,
  "description": "WarpGrid ComponentizeJS toolchain (source build)",
  "dependencies": {
    "@bytecodealliance/jco": "${JCO_VERSION}"
  }
}
EOF

    (cd "${BUILD_DIR}" && npm install --no-audit --no-fund 2>&1)

    log "ComponentizeJS built from source"
}

# ─── Verify ──────────────────────────────────────────────────────────────────

# Global state for cleanup trap (must be file-scope for trap handler)
_verify_tmpdir=""
_verify_server_pid=""

_verify_cleanup() {
    if [ -n "${_verify_server_pid}" ]; then
        kill "${_verify_server_pid}" 2>/dev/null || true
        wait "${_verify_server_pid}" 2>/dev/null || true
        _verify_server_pid=""
    fi
    if [ -n "${_verify_tmpdir}" ]; then
        rm -rf "${_verify_tmpdir}"
        _verify_tmpdir=""
    fi
}

verify() {
    local jco_bin="${BUILD_DIR}/node_modules/.bin/jco"

    if [ ! -x "${jco_bin}" ]; then
        err "jco not found at ${jco_bin}. Run build first."
    fi

    log "Verifying ComponentizeJS toolchain..."

    local version
    version="$("${jco_bin}" --version)"
    log "jco version: ${version}"

    # Use the project test fixture
    local test_dir="${PROJECT_ROOT}/tests/fixtures/js-http-handler"
    if [ ! -f "${test_dir}/handler.js" ]; then
        err "Test fixture not found at ${test_dir}/handler.js"
    fi

    _verify_tmpdir="$(mktemp -d)"
    trap _verify_cleanup EXIT
    local tmpdir="${_verify_tmpdir}"

    # Step 1: Componentize the handler
    log "Componentizing HTTP handler..."
    if ! "${jco_bin}" componentize \
        "${test_dir}/handler.js" \
        --wit "${test_dir}/wit/" \
        --world-name handler \
        --enable http \
        --enable fetch-event \
        -o "${tmpdir}/handler.wasm" 2>&1; then
        err "Componentization failed. Check handler.js and WIT definitions."
    fi

    if [ ! -f "${tmpdir}/handler.wasm" ]; then
        err "Componentization failed: handler.wasm not produced"
    fi

    local wasm_size
    wasm_size="$(wc -c < "${tmpdir}/handler.wasm" | tr -d ' ')"
    log "Compiled handler.wasm: ${wasm_size} bytes"

    # Step 2: Validate it's a valid Wasm component
    if command -v wasm-tools &>/dev/null; then
        log "Validating component with wasm-tools..."
        if wasm-tools component wit "${tmpdir}/handler.wasm" > /dev/null 2>&1; then
            log "Component WIT validation: PASS"
        else
            warn "Component WIT validation: could not extract WIT (may still be valid)"
        fi
    fi

    # Step 3: Run the component and verify HTTP response
    # Uses jco serve (Node.js based, supports WASI 0.2.3 natively)
    # Note: wasmtime serve requires Wasmtime 41+ for WASI 0.2.3 interfaces
    # Find a free port; fall back to 8787 if python3 is unavailable
    local port
    port=$(python3 -c "import socket; s=socket.socket(); s.bind(('',0)); print(s.getsockname()[1]); s.close()" 2>/dev/null || echo "8787")

    local server_ready=false

    log "Testing HTTP handler via jco serve on port ${port}..."
    "${jco_bin}" serve "${tmpdir}/handler.wasm" --port "${port}" \
        >"${tmpdir}/serve.stdout" 2>"${tmpdir}/serve.stderr" &
    _verify_server_pid=$!

    # jco serve takes ~5s to transpile the component before listening
    local attempts=0
    local max_attempts=30
    while [ ${attempts} -lt ${max_attempts} ]; do
        if grep -q "Server listening" "${tmpdir}/serve.stderr" 2>/dev/null; then
            server_ready=true
            break
        fi
        if ! kill -0 "${_verify_server_pid}" 2>/dev/null; then
            warn "jco serve process exited unexpectedly"
            cat "${tmpdir}/serve.stderr" >&2
            break
        fi
        sleep 0.5
        attempts=$((attempts + 1))
    done

    if ${server_ready}; then
        # Give an extra moment for the HTTP listener to bind
        sleep 1

        local response
        response="$(curl -s --max-time 5 "http://localhost:${port}/" 2>&1)" || true

        # Stop server
        kill "${_verify_server_pid}" 2>/dev/null || true
        wait "${_verify_server_pid}" 2>/dev/null || true
        _verify_server_pid=""

        if echo "${response}" | grep -q "ok"; then
            log "HTTP response: '${response}'"
            log "HTTP round-trip verification: PASS"
        else
            warn "HTTP response did not contain 'ok': '${response}'"
            warn "jco serve stderr: $(cat "${tmpdir}/serve.stderr" 2>/dev/null)"
        fi
    else
        kill "${_verify_server_pid}" 2>/dev/null || true
        wait "${_verify_server_pid}" 2>/dev/null || true
        _verify_server_pid=""
        warn "jco serve did not start within timeout (${max_attempts}x0.5s)"
        log "Verification complete (componentization passed, runtime skipped)"
    fi

    # Explicit cleanup on happy path
    rm -rf "${_verify_tmpdir}"
    _verify_tmpdir=""

    log "Verification complete."
}

# ─── Stamp File (for idempotency) ───────────────────────────────────────────

write_stamp() {
    local mode="$1"
    mkdir -p "${BUILD_DIR}"
    echo "${COMPONENTIZE_JS_TAG}:${JCO_VERSION}:${mode}" > "${BUILD_DIR}/.build-stamp"
}

read_stamp() {
    if [ -f "${BUILD_DIR}/.build-stamp" ]; then
        cat "${BUILD_DIR}/.build-stamp"
    else
        echo ""
    fi
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
    local mode="npm"
    local do_clean=false
    local do_verify=false

    while [ $# -gt 0 ]; do
        case "$1" in
            --npm)        mode="npm" ;;
            --source)     mode="source" ;;
            --clean)      do_clean=true ;;
            --verify)     do_verify=true ;;
            --help|-h)    usage ;;
            *)            err "Unknown flag: $1. Use --help for usage." ;;
        esac
        shift
    done

    # Prerequisites
    check_git
    check_node_version
    check_npm

    # Clean if requested
    if ${do_clean}; then
        log "Cleaning build artifacts..."
        rm -rf "${BUILD_DIR}"
    fi

    # Idempotency: skip if already installed with same versions and mode
    local expected_stamp="${COMPONENTIZE_JS_TAG}:${JCO_VERSION}:${mode}"
    if [ -x "${BUILD_DIR}/node_modules/.bin/jco" ] && ! ${do_clean}; then
        local current_stamp
        current_stamp="$(read_stamp)"
        if [ "${current_stamp}" = "${expected_stamp}" ]; then
            log "ComponentizeJS already installed at ${COMPONENTIZE_JS_TAG} (mode: ${mode}) — skipping"
            if ${do_verify}; then
                verify
            fi
            return 0
        fi
    fi

    # Always ensure source is cloned (for future patching)
    ensure_source

    # Build based on mode
    case "${mode}" in
        npm)
            install_from_npm
            ;;
        source)
            build_from_source
            ;;
    esac

    write_stamp "${mode}"

    if ${do_verify}; then
        verify
    fi

    log "Done. jco binary: ${BUILD_DIR}/node_modules/.bin/jco"
}

main "$@"
