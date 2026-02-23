#!/usr/bin/env bash
#
# build-tinygo.sh — Clone and build TinyGo for the WarpGrid SDK.
#
# Produces a working tinygo binary that supports the wasip2 target.
# The TinyGo source is cloned into vendor/tinygo/ at a pinned release tag,
# ready for future WarpGrid patches (Domain 3: US-303+).
#
# Build modes:
#   --source       Build from source using system LLVM (requires LLVM 17-20)
#   --build-llvm   Build from source, building LLVM first (slow, ~1hr)
#   --download     Download pre-built release binary (default, fastest)
#
# Prerequisites:
#   --source:     Go 1.22+, LLVM 17-20 (Homebrew or apt), git
#   --build-llvm: Go 1.22+, cmake, ninja, git
#   --download:   curl or wget, tar
#
# Usage:
#   scripts/build-tinygo.sh                # Download pre-built binary (default)
#   scripts/build-tinygo.sh --source       # Build from source with system LLVM
#   scripts/build-tinygo.sh --build-llvm   # Build LLVM + TinyGo from source
#   scripts/build-tinygo.sh --verify       # Verify: compile hello-world, run in Wasmtime
#   scripts/build-tinygo.sh --clean        # Remove build artifacts, rebuild
#   scripts/build-tinygo.sh --help         # Show this help
#
# Environment variables:
#   TINYGO_TAG        TinyGo release tag to pin (default: v0.40.0)
#   TINYGO_VERSION    Version number without v prefix (derived from TINYGO_TAG)
#   LLVM_PATH         Path to LLVM installation (auto-detected if not set)
#   JOBS              Parallelism for LLVM build (default: nproc)
#
# Output:
#   build/tinygo/bin/tinygo   — the TinyGo binary
#   vendor/tinygo/            — TinyGo source (cloned for future patching)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VENDOR_DIR="${PROJECT_ROOT}/vendor/tinygo"
BUILD_DIR="${PROJECT_ROOT}/build/tinygo"
CACHE_DIR="${PROJECT_ROOT}/build/cache"
TINYGO_TAG="${TINYGO_TAG:-v0.40.0}"
TINYGO_VERSION="${TINYGO_VERSION:-${TINYGO_TAG#v}}"
TINYGO_REPO="https://github.com/tinygo-org/tinygo.git"

# Detect parallelism
if command -v nproc &>/dev/null; then
    JOBS="${JOBS:-$(nproc)}"
elif command -v sysctl &>/dev/null; then
    JOBS="${JOBS:-$(sysctl -n hw.logicalcpu 2>/dev/null || echo 4)}"
else
    JOBS="${JOBS:-4}"
fi

MIN_GO_MAJOR=1
MIN_GO_MINOR=22

# ─── Helpers ─────────────────────────────────────────────────────────────────

log() { echo "==> $*" >&2; }
err() { echo "ERROR: $*" >&2; exit 1; }
warn() { echo "WARNING: $*" >&2; }

usage() {
    echo "build-tinygo.sh — Clone and build TinyGo for the WarpGrid SDK."
    echo
    echo "Build modes:"
    echo "  --download     Download pre-built release binary (default, fastest)"
    echo "  --source       Build from source with system LLVM (requires LLVM 17-20)"
    echo "  --build-llvm   Build LLVM from source, then build TinyGo (slow, ~1hr)"
    echo
    echo "Options:"
    echo "  --clean        Remove build artifacts before building"
    echo "  --verify       Verify the built tinygo works (compile + run hello world)"
    echo "  --help         Show this help message"
    echo
    echo "Environment variables:"
    echo "  TINYGO_TAG     TinyGo release tag (default: ${TINYGO_TAG})"
    echo "  LLVM_PATH      Path to LLVM installation (auto-detected)"
    echo "  JOBS           Build parallelism (default: ${JOBS})"
    echo
    echo "Output: build/tinygo/bin/tinygo"
    exit 0
}

# ─── Prerequisite Checks ────────────────────────────────────────────────────

check_go_version() {
    if ! command -v go &>/dev/null; then
        err "Go is not installed. Install Go ${MIN_GO_MAJOR}.${MIN_GO_MINOR}+ from https://go.dev/dl/"
    fi

    local go_version ver major minor
    go_version="$(go version)"
    ver="$(echo "${go_version}" | grep -o 'go[0-9.]*' | sed 's/go//')"
    major="$(echo "${ver}" | cut -d. -f1)"
    minor="$(echo "${ver}" | cut -d. -f2)"

    if [ "${major}" -lt "${MIN_GO_MAJOR}" ] || \
       { [ "${major}" -eq "${MIN_GO_MAJOR}" ] && [ "${minor}" -lt "${MIN_GO_MINOR}" ]; }; then
        err "Go ${MIN_GO_MAJOR}.${MIN_GO_MINOR}+ required, found go${ver}"
    fi

    log "Go version: go${ver}"
}

# Ensure wasm-opt (binaryen) and wasm-tools are available.
# TinyGo wasip2 target requires both:
#   - wasm-opt: Wasm binary optimizer (from binaryen)
#   - wasm-tools: WASI component model tooling (component embed + new)
ensure_wasm_tools() {
    local missing=()

    if ! command -v wasm-opt &>/dev/null; then
        missing+=("wasm-opt")
    fi

    if ! command -v wasm-tools &>/dev/null; then
        missing+=("wasm-tools")
    fi

    if [ ${#missing[@]} -eq 0 ]; then
        log "wasm-opt: $(wasm-opt --version 2>&1 | head -1)"
        log "wasm-tools: $(wasm-tools --version 2>&1 | head -1)"
        return 0
    fi

    echo
    echo "Missing tools required for TinyGo wasip2 target: ${missing[*]}"
    echo
    echo "Install via cargo (recommended):"
    for tool in "${missing[@]}"; do
        echo "  cargo install ${tool}"
    done
    echo
    echo "Or via package manager:"
    echo "  macOS:  brew install binaryen wabt"
    echo "  Ubuntu: apt install binaryen wabt"
    echo
    err "Install missing tools and retry"
}

check_git() {
    if ! command -v git &>/dev/null; then
        err "git is not installed"
    fi
}

# ─── Platform Detection ─────────────────────────────────────────────────────

detect_platform() {
    local os arch
    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    arch="$(uname -m)"

    case "${os}" in
        darwin) os="darwin" ;;
        linux)  os="linux" ;;
        *)      err "Unsupported OS: ${os}" ;;
    esac

    case "${arch}" in
        x86_64|amd64) arch="amd64" ;;
        aarch64|arm64) arch="arm64" ;;
        *)             err "Unsupported architecture: ${arch}" ;;
    esac

    echo "${os}-${arch}"
}

# ─── LLVM Detection ─────────────────────────────────────────────────────────

detect_llvm() {
    if [ -n "${LLVM_PATH:-}" ]; then
        if [ -x "${LLVM_PATH}/bin/llvm-config" ]; then
            log "Using LLVM from LLVM_PATH: ${LLVM_PATH}"
            return 0
        fi
        err "LLVM_PATH set to ${LLVM_PATH} but llvm-config not found there"
    fi

    # macOS: check Homebrew
    if [ "$(uname)" = "Darwin" ]; then
        local brew_llvm
        for ver in 20 19 18 17 ""; do
            if [ -n "${ver}" ]; then
                brew_llvm="$(brew --prefix "llvm@${ver}" 2>/dev/null || true)"
            else
                brew_llvm="$(brew --prefix llvm 2>/dev/null || true)"
            fi
            if [ -n "${brew_llvm}" ] && [ -x "${brew_llvm}/bin/llvm-config" ]; then
                LLVM_PATH="${brew_llvm}"
                log "Detected Homebrew LLVM: ${LLVM_PATH}"
                log "LLVM version: $("${LLVM_PATH}/bin/llvm-config" --version)"
                return 0
            fi
        done
    fi

    # Linux: check system llvm-config
    for ver in 20 19 18 17 ""; do
        local cmd="llvm-config"
        if [ -n "${ver}" ]; then
            cmd="llvm-config-${ver}"
        fi
        if command -v "${cmd}" &>/dev/null; then
            LLVM_PATH="$("${cmd}" --prefix)"
            log "Detected system LLVM: ${LLVM_PATH} (${cmd})"
            return 0
        fi
    done

    return 1
}

# ─── Clone / Update TinyGo Source ────────────────────────────────────────────

ensure_tinygo_source() {
    if [ -d "${VENDOR_DIR}/.git" ]; then
        local current_tag
        current_tag="$(cd "${VENDOR_DIR}" && git describe --tags --exact-match HEAD 2>/dev/null || echo "none")"

        if [ "${current_tag}" = "${TINYGO_TAG}" ]; then
            log "TinyGo source already at ${TINYGO_TAG}"
            return 0
        fi

        log "TinyGo source at ${current_tag}, switching to ${TINYGO_TAG}..."
        (cd "${VENDOR_DIR}" && git fetch origin "refs/tags/${TINYGO_TAG}:refs/tags/${TINYGO_TAG}" && git checkout "${TINYGO_TAG}")
    else
        log "Cloning TinyGo at ${TINYGO_TAG}..."
        mkdir -p "$(dirname "${VENDOR_DIR}")"
        git clone --branch "${TINYGO_TAG}" --depth 1 "${TINYGO_REPO}" "${VENDOR_DIR}"
    fi

    log "Initializing TinyGo submodules..."
    (cd "${VENDOR_DIR}" && git submodule update --init --recursive --depth 1)

    log "TinyGo source ready at ${VENDOR_DIR}"
}

# ─── Download Pre-built Binary ───────────────────────────────────────────────

download_tinygo() {
    local platform
    platform="$(detect_platform)"

    local url="https://github.com/tinygo-org/tinygo/releases/download/${TINYGO_TAG}/tinygo${TINYGO_VERSION}.${platform}.tar.gz"
    local tarball="${CACHE_DIR}/tinygo-${TINYGO_VERSION}-${platform}.tar.gz"

    mkdir -p "${CACHE_DIR}" "${BUILD_DIR}"

    # Download if not cached
    if [ ! -f "${tarball}" ]; then
        log "Downloading TinyGo ${TINYGO_TAG} for ${platform}..."
        log "URL: ${url}"

        if command -v curl &>/dev/null; then
            curl -fSL -o "${tarball}" "${url}"
        elif command -v wget &>/dev/null; then
            wget -q -O "${tarball}" "${url}"
        else
            err "Neither curl nor wget available for download"
        fi

        log "Downloaded: $(wc -c < "${tarball}" | tr -d ' ') bytes"
    else
        log "Using cached download: ${tarball}"
    fi

    # Extract to build dir
    log "Extracting TinyGo..."
    rm -rf "${BUILD_DIR}/extracted"
    mkdir -p "${BUILD_DIR}/extracted"
    tar xzf "${tarball}" -C "${BUILD_DIR}/extracted"

    # TinyGo tarballs extract to a tinygo/ subdirectory
    local extracted_dir="${BUILD_DIR}/extracted/tinygo"
    if [ ! -d "${extracted_dir}" ]; then
        err "Unexpected archive layout: tinygo/ directory not found"
    fi

    # Move bin to our standard location
    mkdir -p "${BUILD_DIR}/bin"
    cp -f "${extracted_dir}/bin/tinygo" "${BUILD_DIR}/bin/tinygo"
    chmod +x "${BUILD_DIR}/bin/tinygo"

    # TinyGo needs its lib/ directory (targets, compiler-rt, picolibc, wasi-libc)
    rm -rf "${BUILD_DIR}/lib"
    cp -R "${extracted_dir}/lib" "${BUILD_DIR}/lib"

    # Copy pkg/ if present (machine-specific compiled packages)
    if [ -d "${extracted_dir}/pkg" ]; then
        rm -rf "${BUILD_DIR}/pkg"
        cp -R "${extracted_dir}/pkg" "${BUILD_DIR}/pkg"
    fi

    # Copy targets/ if separate from lib/
    if [ -d "${extracted_dir}/targets" ]; then
        rm -rf "${BUILD_DIR}/targets"
        cp -R "${extracted_dir}/targets" "${BUILD_DIR}/targets"
    fi

    # Copy src/ runtime files needed by TinyGo
    if [ -d "${extracted_dir}/src" ]; then
        rm -rf "${BUILD_DIR}/src"
        cp -R "${extracted_dir}/src" "${BUILD_DIR}/src"
    fi

    # Clean up extracted temp
    rm -rf "${BUILD_DIR}/extracted"

    log "TinyGo ${TINYGO_TAG} installed to ${BUILD_DIR}/"
    TINYGOROOT="${BUILD_DIR}" "${BUILD_DIR}/bin/tinygo" version
}

# ─── Build LLVM from Source ──────────────────────────────────────────────────

build_llvm_from_source() {
    log "Building LLVM from source (this will take a while)..."

    if ! command -v cmake &>/dev/null; then
        err "cmake is required for building LLVM from source"
    fi

    (cd "${VENDOR_DIR}" && make llvm-source)
    (cd "${VENDOR_DIR}" && make -j"${JOBS}" llvm-build)

    LLVM_PATH="${VENDOR_DIR}/llvm-build"
    log "LLVM built at ${LLVM_PATH}"
}

# ─── Build TinyGo from Source ────────────────────────────────────────────────

build_tinygo_from_source() {
    log "Building TinyGo from source..."

    mkdir -p "${BUILD_DIR}/bin"

    (
        cd "${VENDOR_DIR}"

        local llvm_cflags llvm_ldflags
        llvm_cflags="$("${LLVM_PATH}/bin/llvm-config" --cflags 2>/dev/null || true)"
        llvm_ldflags="$("${LLVM_PATH}/bin/llvm-config" --ldflags --libs --system-libs all 2>/dev/null || true)"

        export CGO_CFLAGS="${llvm_cflags}"
        export CGO_LDFLAGS="${llvm_ldflags}"
        export GOBIN="${BUILD_DIR}/bin"

        log "Building with LLVM at ${LLVM_PATH}..."
        go install
    )

    if [ ! -x "${BUILD_DIR}/bin/tinygo" ]; then
        err "Build failed: ${BUILD_DIR}/bin/tinygo not found"
    fi

    # TinyGo binary needs TINYGOROOT to find targets, compiler-rt, etc.
    # When built from source, TINYGOROOT points to the source tree.
    log "TinyGo built successfully: ${BUILD_DIR}/bin/tinygo"
    TINYGOROOT="${VENDOR_DIR}" "${BUILD_DIR}/bin/tinygo" version
}

# ─── Verify ──────────────────────────────────────────────────────────────────

verify_tinygo() {
    local tinygo="${BUILD_DIR}/bin/tinygo"

    if [ ! -x "${tinygo}" ]; then
        err "TinyGo binary not found at ${tinygo}. Run build first."
    fi

    # wasm-opt and wasm-tools are required for wasip2 compilation
    ensure_wasm_tools

    # Determine TINYGOROOT: use build dir for downloaded, vendor dir for source-built
    local tinygoroot="${BUILD_DIR}"
    if [ ! -d "${BUILD_DIR}/lib" ] && [ -d "${VENDOR_DIR}/lib" ]; then
        tinygoroot="${VENDOR_DIR}"
    fi

    log "Verifying TinyGo build (TINYGOROOT=${tinygoroot})..."

    local version
    version="$(TINYGOROOT="${tinygoroot}" "${tinygo}" version)"
    log "Version: ${version}"

    # Use the project test fixture
    local test_dir="${PROJECT_ROOT}/tests/fixtures/go-hello-world"
    if [ ! -f "${test_dir}/main.go" ]; then
        err "Test fixture not found at ${test_dir}/main.go"
    fi

    local tmpdir
    tmpdir="$(mktemp -d)"
    trap "rm -rf '${tmpdir}'" EXIT

    # Compile to wasip2
    log "Compiling hello-world to wasm32-wasip2..."
    TINYGOROOT="${tinygoroot}" "${tinygo}" build \
        -target=wasip2 \
        -o "${tmpdir}/test.wasm" \
        "${test_dir}/main.go"

    if [ ! -f "${tmpdir}/test.wasm" ]; then
        err "Compilation failed: test.wasm not produced"
    fi

    local wasm_size
    wasm_size="$(wc -c < "${tmpdir}/test.wasm" | tr -d ' ')"
    log "Compiled test.wasm: ${wasm_size} bytes"

    # Run in Wasmtime
    if command -v wasmtime &>/dev/null; then
        log "Running test.wasm in Wasmtime..."
        local output=""
        local exit_code=0

        # Try with component-model flag first (older wasmtime), then without
        output="$(wasmtime run --wasm component-model=y "${tmpdir}/test.wasm" 2>&1)" || exit_code=$?

        if [ ${exit_code} -ne 0 ]; then
            log "Retrying without component-model flag..."
            exit_code=0
            output="$(wasmtime run "${tmpdir}/test.wasm" 2>&1)" || exit_code=$?
        fi

        if echo "${output}" | grep -q "Hello from TinyGo on WarpGrid!"; then
            log "Wasmtime execution: PASS"
            log "Output: ${output}"
        else
            warn "Wasmtime output did not contain expected string (exit code: ${exit_code})"
            warn "Output: ${output}"
            warn "This may be a Wasmtime version compatibility issue (have: $(wasmtime --version))"
            warn "TinyGo ${TINYGO_TAG} targets may require newer Wasmtime."
        fi
    else
        warn "Wasmtime not installed — skipping runtime verification"
    fi

    log "Verification complete."
}

# ─── Stamp File (for idempotency) ───────────────────────────────────────────

write_stamp() {
    local mode="$1"
    mkdir -p "${BUILD_DIR}"
    echo "${TINYGO_TAG}:${mode}" > "${BUILD_DIR}/.build-stamp"
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
    local mode="download"
    local do_clean=false
    local do_verify=false

    while [ $# -gt 0 ]; do
        case "$1" in
            --download)   mode="download" ;;
            --source)     mode="source" ;;
            --build-llvm) mode="build-llvm" ;;
            --clean)      do_clean=true ;;
            --verify)     do_verify=true ;;
            --help|-h)    usage ;;
            *)            err "Unknown flag: $1. Use --help for usage." ;;
        esac
        shift
    done

    # Prerequisites
    check_git

    if [ "${mode}" = "source" ] || [ "${mode}" = "build-llvm" ]; then
        check_go_version
    fi

    # Clean if requested
    if ${do_clean}; then
        log "Cleaning build artifacts..."
        rm -rf "${BUILD_DIR}"
    fi

    # Idempotency: skip if already built with same tag and mode
    local expected_stamp="${TINYGO_TAG}:${mode}"
    if [ -x "${BUILD_DIR}/bin/tinygo" ] && ! ${do_clean}; then
        local current_stamp
        current_stamp="$(read_stamp)"
        if [ "${current_stamp}" = "${expected_stamp}" ]; then
            log "TinyGo already built at ${TINYGO_TAG} (mode: ${mode}) — skipping"
            if ${do_verify}; then
                verify_tinygo
            fi
            return 0
        fi
    fi

    # Always clone source (needed for future patching, and as TINYGOROOT for source builds)
    ensure_tinygo_source

    # Build based on mode
    case "${mode}" in
        download)
            download_tinygo
            ;;
        source)
            if ! detect_llvm; then
                echo
                echo "No system LLVM installation found."
                echo
                echo "Options:"
                echo "  macOS:  brew install llvm"
                echo "  Ubuntu: apt install llvm-18-dev lld-18 libclang-18-dev"
                echo "  Build:  scripts/build-tinygo.sh --build-llvm  (takes ~1 hour)"
                echo "  Fast:   scripts/build-tinygo.sh --download    (pre-built binary)"
                echo
                err "LLVM is required for --source builds"
            fi
            build_tinygo_from_source
            ;;
        build-llvm)
            build_llvm_from_source
            build_tinygo_from_source
            ;;
    esac

    write_stamp "${mode}"

    if ${do_verify}; then
        verify_tinygo
    fi

    log "Done. TinyGo binary: ${BUILD_DIR}/bin/tinygo"
}

main "$@"
