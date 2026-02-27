#!/usr/bin/env bash
#
# build-wasmtime-async.sh — Validate and verify Wasmtime WASI 0.3 async support.
#
# Wasmtime 41 ships with component-model-async and WASI Preview 3 support
# via crates.io — building from source is no longer necessary. This script
# validates that the workspace dependency is correctly configured and the
# async runtime is functional.
#
# Prerequisites:
#   - Rust toolchain (rustup with stable or nightly)
#   - wasm32-unknown-unknown target installed
#   - wasm-tools CLI
#
# Usage:
#   scripts/build-wasmtime-async.sh              # Full validation
#   scripts/build-wasmtime-async.sh --check      # Dependency check only
#   scripts/build-wasmtime-async.sh --test       # Run async integration test
#   scripts/build-wasmtime-async.sh --help       # Show this help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# ── Pinned dependency versions ────────────────────────────────────
# These are the crates.io versions providing WASI 0.3 async support.
# Update these when upgrading Wasmtime.
WASMTIME_VERSION="41"
WASMTIME_FEATURES="component-model, async, component-model-async"
WASMTIME_WASI_FEATURES="p3"

# ── Colors ────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info()  { echo -e "${GREEN}[INFO]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }

# ── Help ──────────────────────────────────────────────────────────
show_help() {
    sed -n '2,/^$/s/^# //p' "$0"
    exit 0
}

# ── Dependency check ──────────────────────────────────────────────
check_dependencies() {
    info "Checking prerequisites..."

    local ok=true

    # Rust toolchain
    if ! command -v cargo &>/dev/null; then
        error "cargo not found. Install Rust: https://rustup.rs/"
        ok=false
    else
        local rust_version
        rust_version=$(rustc --version)
        info "  Rust: ${rust_version}"
    fi

    # wasm32-unknown-unknown target
    if ! rustup target list --installed 2>/dev/null | grep -q "wasm32-unknown-unknown"; then
        warn "  wasm32-unknown-unknown target not installed. Installing..."
        rustup target add wasm32-unknown-unknown
    fi
    info "  Target: wasm32-unknown-unknown installed"

    # wasm-tools
    if ! command -v wasm-tools &>/dev/null; then
        error "wasm-tools not found. Install: cargo install wasm-tools"
        ok=false
    else
        local wt_version
        wt_version=$(wasm-tools --version 2>/dev/null || echo "unknown")
        info "  wasm-tools: ${wt_version}"
    fi

    if [ "$ok" = false ]; then
        error "Missing prerequisites. See errors above."
        exit 1
    fi

    info "All prerequisites satisfied."
}

# ── Validate workspace Cargo.toml ─────────────────────────────────
validate_workspace() {
    info "Validating workspace Wasmtime configuration..."

    local cargo_toml="${PROJECT_ROOT}/Cargo.toml"

    if ! grep -q "wasmtime.*version.*=.*\"${WASMTIME_VERSION}\"" "${cargo_toml}"; then
        error "Cargo.toml does not pin wasmtime to version ${WASMTIME_VERSION}"
        exit 1
    fi
    info "  wasmtime = \"${WASMTIME_VERSION}\" ✓"

    if ! grep -q "component-model-async" "${cargo_toml}"; then
        error "Cargo.toml missing component-model-async feature for wasmtime"
        exit 1
    fi
    info "  features: [${WASMTIME_FEATURES}] ✓"

    if ! grep -q "wasmtime-wasi" "${cargo_toml}" || ! grep -q '"p3"' "${cargo_toml}"; then
        error "Cargo.toml missing wasmtime-wasi with p3 feature"
        exit 1
    fi
    info "  wasmtime-wasi features: [${WASMTIME_WASI_FEATURES}] ✓"

    info "Workspace configuration valid."
}

# ── Compile check ─────────────────────────────────────────────────
compile_check() {
    info "Running cargo check for warpgrid-host (includes async support)..."
    cd "${PROJECT_ROOT}"
    cargo check -p warpgrid-host 2>&1
    info "Compilation check passed."
}

# ── Run async integration tests ───────────────────────────────────
run_async_tests() {
    info "Running async handler integration tests..."
    cd "${PROJECT_ROOT}"
    cargo test -p warpgrid-host -- async_handler 2>&1
    info "Async integration tests passed."
}

# ── Main ──────────────────────────────────────────────────────────
main() {
    local mode="${1:-}"

    case "${mode}" in
        --help|-h)
            show_help
            ;;
        --check)
            check_dependencies
            validate_workspace
            ;;
        --test)
            run_async_tests
            ;;
        "")
            # Full validation
            check_dependencies
            validate_workspace
            compile_check
            run_async_tests
            info ""
            info "═══════════════════════════════════════════════════════════════"
            info "  Wasmtime WASI 0.3 async support verified successfully"
            info ""
            info "  Runtime:    wasmtime ${WASMTIME_VERSION} (crates.io)"
            info "  Features:   ${WASMTIME_FEATURES}"
            info "  WASI:       wasmtime-wasi ${WASMTIME_VERSION} [${WASMTIME_WASI_FEATURES}]"
            info "  Async:      component-model-async ✓"
            info "═══════════════════════════════════════════════════════════════"
            ;;
        *)
            error "Unknown flag: ${mode}"
            show_help
            ;;
    esac
}

main "$@"
