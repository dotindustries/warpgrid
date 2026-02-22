#!/usr/bin/env bash
#
# build-libc.sh — Build wasi-libc from the WarpGrid pinned upstream ref.
#
# Produces a sysroot under build/sysroot-stock/ (unpatched) or
# build/sysroot-patched/ (with WarpGrid patches applied).
#
# Prerequisites:
#   - cmake >= 3.26
#   - wasi-sdk (auto-downloaded if WASI_SDK_PATH is not set)
#   - git
#
# Usage:
#   scripts/build-libc.sh --stock       # Build unpatched wasi-libc
#   scripts/build-libc.sh --patched     # Build with WarpGrid patches applied
#   scripts/build-libc.sh --both        # Build both variants
#   scripts/build-libc.sh --help        # Show this help
#
# Environment variables:
#   WASI_SDK_PATH    Path to an existing wasi-sdk installation (skips download)
#   WASI_SDK_VERSION Version of wasi-sdk to download (default: 30.0)
#   TARGET_TRIPLE    WASI target triple (default: wasm32-wasip2)
#   JOBS             Parallelism for cmake build (default: $(nproc) or sysctl)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
UPSTREAM_REF_FILE="${PROJECT_ROOT}/libc-patches/UPSTREAM_REF"
PATCHES_DIR="${PROJECT_ROOT}/libc-patches"
BUILD_DIR="${PROJECT_ROOT}/build"
CACHE_DIR="${BUILD_DIR}/cache"

# Defaults
WASI_SDK_VERSION="${WASI_SDK_VERSION:-30.0}"
TARGET_TRIPLE="${TARGET_TRIPLE:-wasm32-wasip2}"

# Detect parallelism
if command -v nproc &>/dev/null; then
    JOBS="${JOBS:-$(nproc)}"
elif command -v sysctl &>/dev/null; then
    JOBS="${JOBS:-$(sysctl -n hw.logicalcpu 2>/dev/null || echo 4)}"
else
    JOBS="${JOBS:-4}"
fi

# ─── Helpers ─────────────────────────────────────────────────────────────────

log() { echo "==> $*" >&2; }
err() { echo "ERROR: $*" >&2; exit 1; }

usage() {
    echo "build-libc.sh — Build wasi-libc from the WarpGrid pinned upstream ref."
    echo
    echo "Flags:"
    echo "  --stock       Build unpatched (stock) wasi-libc"
    echo "  --patched     Build wasi-libc with WarpGrid patches applied"
    echo "  --both        Build both stock and patched variants"
    echo "  --clean       Remove build artifacts before building"
    echo "  --help        Show this help message"
    echo
    echo "Environment:"
    echo "  WASI_SDK_PATH    Path to wasi-sdk (auto-downloaded if unset)"
    echo "  WASI_SDK_VERSION wasi-sdk version to download (default: ${WASI_SDK_VERSION})"
    echo "  TARGET_TRIPLE    Target triple (default: ${TARGET_TRIPLE})"
    echo "  JOBS             Build parallelism (default: ${JOBS})"
}

# Parse UPSTREAM_REF file
parse_upstream_ref() {
    if [[ ! -f "${UPSTREAM_REF_FILE}" ]]; then
        err "UPSTREAM_REF file not found at ${UPSTREAM_REF_FILE}"
    fi

    local tag commit
    tag=$(grep '^TAG=' "${UPSTREAM_REF_FILE}" | cut -d= -f2)
    commit=$(grep '^COMMIT=' "${UPSTREAM_REF_FILE}" | cut -d= -f2)

    if [[ -z "${tag}" || -z "${commit}" ]]; then
        err "UPSTREAM_REF must contain TAG=<tag> and COMMIT=<hash>"
    fi

    UPSTREAM_TAG="${tag}"
    UPSTREAM_COMMIT="${commit}"
}

# ─── wasi-sdk management ────────────────────────────────────────────────────

detect_platform() {
    local os arch
    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    arch="$(uname -m)"

    case "${os}" in
        darwin) os="macos" ;;
        linux)  os="linux" ;;
        *)      err "Unsupported OS: ${os}" ;;
    esac

    case "${arch}" in
        x86_64|amd64) arch="x86_64" ;;
        arm64|aarch64) arch="arm64" ;;
        *)            err "Unsupported architecture: ${arch}" ;;
    esac

    echo "${arch}-${os}"
}

ensure_wasi_sdk() {
    if [[ -n "${WASI_SDK_PATH:-}" ]] && [[ -x "${WASI_SDK_PATH}/bin/clang" ]]; then
        log "Using existing wasi-sdk at ${WASI_SDK_PATH}"
        return
    fi

    local platform sdk_name sdk_archive sdk_url
    platform="$(detect_platform)"
    sdk_name="wasi-sdk-${WASI_SDK_VERSION}-${platform}"
    sdk_archive="${sdk_name}.tar.gz"
    sdk_url="https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_SDK_VERSION%%.*}/${sdk_archive}"

    WASI_SDK_PATH="${CACHE_DIR}/${sdk_name}"

    if [[ -x "${WASI_SDK_PATH}/bin/clang" ]]; then
        log "Using cached wasi-sdk at ${WASI_SDK_PATH}"
        return
    fi

    log "Downloading wasi-sdk ${WASI_SDK_VERSION} for ${platform}..."
    mkdir -p "${CACHE_DIR}"
    curl -fSL "${sdk_url}" -o "${CACHE_DIR}/${sdk_archive}"
    tar xzf "${CACHE_DIR}/${sdk_archive}" -C "${CACHE_DIR}"
    rm -f "${CACHE_DIR}/${sdk_archive}"

    if [[ ! -x "${WASI_SDK_PATH}/bin/clang" ]]; then
        # Try alternate directory naming (some releases use different formats)
        local extracted
        extracted=$(ls -d "${CACHE_DIR}/wasi-sdk-"* 2>/dev/null | grep -v '\.tar' | head -1)
        if [[ -n "${extracted}" && -x "${extracted}/bin/clang" ]]; then
            mv "${extracted}" "${WASI_SDK_PATH}"
        else
            err "wasi-sdk download succeeded but clang not found at ${WASI_SDK_PATH}/bin/clang"
        fi
    fi

    log "wasi-sdk ${WASI_SDK_VERSION} installed at ${WASI_SDK_PATH}"
}

# ─── wasi-libc clone/checkout ────────────────────────────────────────────────

ensure_source() {
    local src_dir="${1}"
    local variant="${2}" # "stock" or "patched"

    if [[ -d "${src_dir}/.git" ]]; then
        local current_commit
        current_commit=$(git -C "${src_dir}" rev-parse HEAD)
        if [[ "${current_commit}" == "${UPSTREAM_COMMIT}" ]]; then
            log "Source at ${src_dir} already at ${UPSTREAM_COMMIT:0:12}"
            return
        fi
        log "Source at ${src_dir} is at wrong commit, re-cloning..."
        rm -rf "${src_dir}"
    fi

    log "Cloning wasi-libc at ${UPSTREAM_TAG} (${UPSTREAM_COMMIT:0:12})..."
    git clone --depth 1 --branch "${UPSTREAM_TAG}" \
        https://github.com/WebAssembly/wasi-libc.git "${src_dir}" 2>&1

    local actual_commit
    actual_commit=$(git -C "${src_dir}" rev-parse HEAD)
    if [[ "${actual_commit}" != "${UPSTREAM_COMMIT}" ]]; then
        err "Commit mismatch: expected ${UPSTREAM_COMMIT}, got ${actual_commit}"
    fi

    # Apply patches if this is the patched variant
    if [[ "${variant}" == "patched" ]]; then
        apply_patches "${src_dir}"
    fi
}

apply_patches() {
    local src_dir="${1}"
    local patches=()

    # Collect patch files in order
    while IFS= read -r -d '' patch; do
        patches+=("${patch}")
    done < <(find "${PATCHES_DIR}" -maxdepth 1 -name '*.patch' -print0 | sort -z)

    if [[ ${#patches[@]} -eq 0 ]]; then
        log "No patches found in ${PATCHES_DIR}/ — patched build is identical to stock"
        return
    fi

    log "Applying ${#patches[@]} patch(es)..."
    for patch in "${patches[@]}"; do
        local patch_name
        patch_name=$(basename "${patch}")
        log "  Applying ${patch_name}..."
        git -C "${src_dir}" am --3way "${patch}" || {
            err "Failed to apply patch ${patch_name}. Run 'scripts/rebase-libc.sh' to resolve conflicts."
        }
    done
    log "All patches applied successfully"
}

# ─── Build ───────────────────────────────────────────────────────────────────

build_libc() {
    local variant="${1}" # "stock" or "patched"
    local src_dir="${BUILD_DIR}/src-${variant}"
    local cmake_build_dir="${BUILD_DIR}/cmake-${variant}"
    local sysroot_dest="${BUILD_DIR}/sysroot-${variant}"

    log "Building wasi-libc (${variant}) for ${TARGET_TRIPLE}..."

    # Ensure source is available and at the right commit
    ensure_source "${src_dir}" "${variant}"

    # Configure with cmake
    mkdir -p "${cmake_build_dir}"
    cmake -S "${src_dir}" -B "${cmake_build_dir}" \
        -DCMAKE_C_COMPILER="${WASI_SDK_PATH}/bin/clang" \
        -DCMAKE_AR="${WASI_SDK_PATH}/bin/llvm-ar" \
        -DCMAKE_NM="${WASI_SDK_PATH}/bin/llvm-nm" \
        -DCMAKE_RANLIB="${WASI_SDK_PATH}/bin/llvm-ranlib" \
        -DCMAKE_C_COMPILER_TARGET="${TARGET_TRIPLE}" \
        -DTARGET_TRIPLE="${TARGET_TRIPLE}" \
        -DCMAKE_BUILD_TYPE=Release \
        -DBUILD_TESTS=OFF \
        -DBUILD_SHARED=OFF \
        2>&1

    # Build
    cmake --build "${cmake_build_dir}" --parallel "${JOBS}" 2>&1

    # Copy sysroot to final location
    rm -rf "${sysroot_dest}"
    cp -a "${cmake_build_dir}/sysroot" "${sysroot_dest}"

    # Verify libc.a exists
    local libc_a="${sysroot_dest}/lib/${TARGET_TRIPLE}/libc.a"
    if [[ ! -f "${libc_a}" ]]; then
        err "Build succeeded but libc.a not found at ${libc_a}"
    fi

    local libc_size
    libc_size=$(du -h "${libc_a}" | cut -f1)
    log "Build complete: ${libc_a} (${libc_size})"
    log "Sysroot installed at ${sysroot_dest}"
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
    local build_stock=false
    local build_patched=false
    local do_clean=false

    if [[ $# -eq 0 ]]; then
        usage
        exit 1
    fi

    while [[ $# -gt 0 ]]; do
        case "${1}" in
            --stock)   build_stock=true ;;
            --patched) build_patched=true ;;
            --both)    build_stock=true; build_patched=true ;;
            --clean)   do_clean=true ;;
            --help|-h) usage; exit 0 ;;
            *)         err "Unknown flag: ${1}. Use --help for usage." ;;
        esac
        shift
    done

    if ! ${build_stock} && ! ${build_patched}; then
        err "Must specify --stock, --patched, or --both"
    fi

    # Parse upstream ref
    parse_upstream_ref
    log "Upstream: ${UPSTREAM_TAG} (${UPSTREAM_COMMIT:0:12})"

    # Clean if requested
    if ${do_clean}; then
        log "Cleaning build artifacts..."
        rm -rf "${BUILD_DIR}/src-stock" "${BUILD_DIR}/src-patched"
        rm -rf "${BUILD_DIR}/cmake-stock" "${BUILD_DIR}/cmake-patched"
        rm -rf "${BUILD_DIR}/sysroot-stock" "${BUILD_DIR}/sysroot-patched"
    fi

    # Ensure wasi-sdk is available
    ensure_wasi_sdk

    # Check cmake
    if ! command -v cmake &>/dev/null; then
        err "cmake not found. Install cmake >= 3.26."
    fi

    # Build requested variants
    if ${build_stock}; then
        build_libc "stock"
    fi

    if ${build_patched}; then
        build_libc "patched"
    fi

    log "Done."
}

main "$@"
