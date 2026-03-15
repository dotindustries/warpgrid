#!/usr/bin/env bash
#
# rebase-libc.test.sh — TDD tests for rebase-libc.sh --validate mode.
#
# Tests validation of patch ordering, dependency constraints, structured
# headers (WarpGrid-Domain/Deps/WIT), and subset filtering. Uses synthetic
# patch files in a temp directory — no wasi-sdk, wasmtime, or network needed.
#
# Tests:
#   1.  --validate exits 0 on a well-ordered patch set
#   2.  --validate detects missing numeric prefix
#   3.  --validate detects out-of-order patches
#   4.  --validate detects missing dependency
#   5.  --validate reads WarpGrid-Deps: headers from patch files
#   6.  --validate reads WarpGrid-Domain: and WarpGrid-WIT: headers
#   7.  --validate --subset dns validates DNS-only subset
#   8.  --validate --subset filesystem validates filesystem-only subset
#   9.  --validate reports clear error for socket patches without filesystem
#  10.  --help includes --validate and --subset documentation
#
# Usage:
#   ./rebase-libc.test.sh          Run all tests
#   ./rebase-libc.test.sh --quick  Same (all tests are fast)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
REBASE_LIBC="${SCRIPT_DIR}/rebase-libc.sh"

PASS=0
FAIL=0
SKIP=0

# ── Helpers ──────────────────────────────────────────────────────────

log()  { echo "==> $*" >&2; }
pass() { PASS=$((PASS + 1)); echo "  PASS: $*" >&2; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL: $*" >&2; }
skip() { SKIP=$((SKIP + 1)); echo "  SKIP: $*" >&2; }

_tmpdir=""
cleanup() {
    if [ -n "${_tmpdir}" ]; then
        rm -rf "${_tmpdir}"
        _tmpdir=""
    fi
}
trap cleanup EXIT

_tmpdir="$(mktemp -d)"

# ── Parse args ───────────────────────────────────────────────────────

while [ $# -gt 0 ]; do
    case "$1" in
        --quick) ;; # all tests are fast
        --help|-h)
            echo "Usage: rebase-libc.test.sh [--quick]"
            exit 0
            ;;
        *) echo "Unknown flag: $1" >&2; exit 1 ;;
    esac
    shift
done

# ── Prerequisite ─────────────────────────────────────────────────────

if [ ! -f "${REBASE_LIBC}" ]; then
    echo "ERROR: rebase-libc.sh not found at ${REBASE_LIBC}" >&2
    exit 1
fi

if [ ! -x "${REBASE_LIBC}" ]; then
    echo "ERROR: rebase-libc.sh is not executable" >&2
    exit 1
fi

# ── Helpers: create synthetic patch files ────────────────────────────

# Creates a minimal valid git patch file with optional WarpGrid headers.
# Usage: make_patch <dir> <filename> [domain] [deps] [wit]
make_patch() {
    local dir="$1"
    local name="$2"
    local domain="${3:-}"
    local deps="${4:-}"
    local wit="${5:-}"

    local patch_file="${dir}/${name}"
    cat > "${patch_file}" <<PATCH
From aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa Mon Sep 17 00:00:00 2001
From: Test <test@test.local>
Date: Mon, 1 Jan 2024 00:00:00 +0000
Subject: [PATCH] test patch ${name}

Test patch body.
PATCH

    # Add structured headers if provided
    if [ -n "${domain}" ]; then
        echo "WarpGrid-Domain: ${domain}" >> "${patch_file}"
    fi
    if [ -n "${deps}" ]; then
        echo "WarpGrid-Deps: ${deps}" >> "${patch_file}"
    fi
    if [ -n "${wit}" ]; then
        echo "WarpGrid-WIT: ${wit}" >> "${patch_file}"
    fi

    # Add a minimal diff so the patch looks valid
    cat >> "${patch_file}" <<'DIFF'
---
 dummy.c | 1 +
 1 file changed, 1 insertion(+)

diff --git a/dummy.c b/dummy.c
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/dummy.c
@@ -0,0 +1 @@
+// test
DIFF
}

# Create a full valid patch set (all 8 patches with proper headers)
# in the given directory. Mimics the real libc-patches/ layout.
make_valid_patch_set() {
    local dir="$1"
    make_patch "$dir" "0001-dns-getaddrinfo.patch" "dns" "none" "warpgrid:shim/dns"
    make_patch "$dir" "0002-fs-fopen.patch" "filesystem" "none" "warpgrid:shim/filesystem"
    make_patch "$dir" "0003-socket-connect.patch" "socket" "0001,0002" "warpgrid:shim/database-proxy.connect"
    make_patch "$dir" "0004-socket-send-recv.patch" "socket" "0003" "warpgrid:shim/database-proxy.send,warpgrid:shim/database-proxy.recv"
    make_patch "$dir" "0005-socket-close.patch" "socket" "0004" "warpgrid:shim/database-proxy.close"
    make_patch "$dir" "0006-dns-gethostbyname.patch" "dns" "0001" "warpgrid:shim/dns"
    make_patch "$dir" "0007-dns-getnameinfo.patch" "dns" "0001" "warpgrid:shim/dns"
    make_patch "$dir" "0008-fs-timezone.patch" "filesystem" "0002" "warpgrid:shim/filesystem"
}

# Run rebase-libc.sh --validate with a custom patches dir.
# We override PATCHES_DIR by pointing the script at a temp project root
# that has a libc-patches/ directory.
run_validate() {
    local patches_dir="$1"
    shift
    # Create a temporary project structure
    local fake_root="${_tmpdir}/project-$$-${RANDOM}"
    mkdir -p "${fake_root}/libc-patches"
    mkdir -p "${fake_root}/scripts"

    # Copy patches into fake project
    if [ -d "${patches_dir}" ]; then
        cp "${patches_dir}"/*.patch "${fake_root}/libc-patches/" 2>/dev/null || true
    fi

    # Create a minimal UPSTREAM_REF so the script doesn't complain
    cat > "${fake_root}/libc-patches/UPSTREAM_REF" <<'REF'
TAG=test-tag
COMMIT=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
REF

    # Copy the actual script
    cp "${REBASE_LIBC}" "${fake_root}/scripts/rebase-libc.sh"
    chmod +x "${fake_root}/scripts/rebase-libc.sh"

    # Run validate from the fake project root
    local exit_code=0
    local output
    output=$("${fake_root}/scripts/rebase-libc.sh" --validate "$@" 2>&1) || exit_code=$?

    # Clean up fake root
    rm -rf "${fake_root}"

    echo "${output}"
    return ${exit_code}
}

# ── Test 1: --validate exits 0 on well-ordered patch set ─────────────

log "Test 1: --validate exits 0 on well-ordered patch set"

PATCHES="${_tmpdir}/test1-patches"
mkdir -p "${PATCHES}"
make_valid_patch_set "${PATCHES}"

EXIT_CODE=0
OUTPUT=$(run_validate "${PATCHES}" 2>&1) || EXIT_CODE=$?

if [ ${EXIT_CODE} -eq 0 ]; then
    pass "--validate exits 0 on well-ordered patch set"
else
    fail "--validate exited ${EXIT_CODE} on valid patches. Output: ${OUTPUT}"
fi

# ── Test 2: --validate detects missing numeric prefix ─────────────────

log "Test 2: --validate detects missing numeric prefix"

PATCHES="${_tmpdir}/test2-patches"
mkdir -p "${PATCHES}"
make_patch "${PATCHES}" "0001-good.patch" "dns" "none" "warpgrid:shim/dns"
make_patch "${PATCHES}" "no-number-bad.patch" "dns" "none" "warpgrid:shim/dns"

EXIT_CODE=0
OUTPUT=$(run_validate "${PATCHES}" 2>&1) || EXIT_CODE=$?

if [ ${EXIT_CODE} -ne 0 ] && echo "${OUTPUT}" | grep -qi "missing numeric prefix\|numeric prefix"; then
    pass "--validate detects missing numeric prefix"
else
    fail "--validate should detect missing numeric prefix (exit=${EXIT_CODE}). Output: ${OUTPUT}"
fi

# ── Test 3: --validate detects out-of-order patches ───────────────────

log "Test 3: --validate detects out-of-order patches"

PATCHES="${_tmpdir}/test3-patches"
mkdir -p "${PATCHES}"
make_patch "${PATCHES}" "0002-second.patch" "filesystem" "none" "warpgrid:shim/filesystem"
make_patch "${PATCHES}" "0001-first.patch" "dns" "none" "warpgrid:shim/dns"
# Note: files sort lexically, so 0001 comes before 0002 — this is actually in order.
# To test out-of-order we need a patch with a lower number appearing after a higher one.
# Since find|sort is lexical, we need to create patches where numbering goes backwards:
rm -rf "${PATCHES}"
mkdir -p "${PATCHES}"
make_patch "${PATCHES}" "0003-third.patch" "socket" "0001,0002" "warpgrid:shim/database-proxy.connect"
make_patch "${PATCHES}" "0003-duplicate.patch" "socket" "0001,0002" "warpgrid:shim/database-proxy.connect"

EXIT_CODE=0
OUTPUT=$(run_validate "${PATCHES}" 2>&1) || EXIT_CODE=$?

if [ ${EXIT_CODE} -ne 0 ] && echo "${OUTPUT}" | grep -qi "out of order\|duplicate"; then
    pass "--validate detects out-of-order/duplicate patches"
else
    fail "--validate should detect out-of-order patches (exit=${EXIT_CODE}). Output: ${OUTPUT}"
fi

# ── Test 4: --validate detects missing dependency ─────────────────────

log "Test 4: --validate detects missing dependency"

PATCHES="${_tmpdir}/test4-patches"
mkdir -p "${PATCHES}"
# Socket-connect (0003) requires 0001 and 0002, but we only provide 0001
make_patch "${PATCHES}" "0001-dns.patch" "dns" "none" "warpgrid:shim/dns"
make_patch "${PATCHES}" "0003-socket.patch" "socket" "0001,0002" "warpgrid:shim/database-proxy.connect"

EXIT_CODE=0
OUTPUT=$(run_validate "${PATCHES}" 2>&1) || EXIT_CODE=$?

if [ ${EXIT_CODE} -ne 0 ] && echo "${OUTPUT}" | grep -qi "requires\|depend\|missing.*0002"; then
    pass "--validate detects missing dependency (0003 needs 0002)"
else
    fail "--validate should detect missing dep 0002 (exit=${EXIT_CODE}). Output: ${OUTPUT}"
fi

# ── Test 5: --validate reads WarpGrid-Deps: headers ──────────────────

log "Test 5: --validate reads WarpGrid-Deps: headers from patch files"

PATCHES="${_tmpdir}/test5-patches"
mkdir -p "${PATCHES}"
# Create patches without headers — validation should warn about missing headers
make_patch "${PATCHES}" "0001-dns.patch"
make_patch "${PATCHES}" "0002-fs.patch"

EXIT_CODE=0
OUTPUT=$(run_validate "${PATCHES}" 2>&1) || EXIT_CODE=$?

if echo "${OUTPUT}" | grep -qi "WarpGrid-Deps\|missing.*header\|header.*missing"; then
    pass "--validate checks for WarpGrid-Deps: headers"
else
    fail "--validate should check for WarpGrid-Deps headers (exit=${EXIT_CODE}). Output: ${OUTPUT}"
fi

# ── Test 6: --validate reads WarpGrid-Domain: and WarpGrid-WIT: ──────

log "Test 6: --validate reads WarpGrid-Domain: and WarpGrid-WIT: headers"

PATCHES="${_tmpdir}/test6-patches"
mkdir -p "${PATCHES}"
# Create a patch with Deps but missing Domain and WIT
make_patch "${PATCHES}" "0001-dns.patch" "" "" ""
# Add only deps header manually
echo "WarpGrid-Deps: none" >> "${PATCHES}/0001-dns.patch"

EXIT_CODE=0
OUTPUT=$(run_validate "${PATCHES}" 2>&1) || EXIT_CODE=$?

if echo "${OUTPUT}" | grep -qi "WarpGrid-Domain\|WarpGrid-WIT\|missing.*header\|header.*missing"; then
    pass "--validate checks for WarpGrid-Domain and WarpGrid-WIT headers"
else
    fail "--validate should check for Domain/WIT headers (exit=${EXIT_CODE}). Output: ${OUTPUT}"
fi

# ── Test 7: --validate --subset dns validates DNS-only subset ─────────

log "Test 7: --validate --subset dns validates DNS-only subset"

PATCHES="${_tmpdir}/test7-patches"
mkdir -p "${PATCHES}"
# DNS-only subset: 0001, 0006, 0007 — should be self-consistent
make_patch "${PATCHES}" "0001-dns-getaddrinfo.patch" "dns" "none" "warpgrid:shim/dns"
make_patch "${PATCHES}" "0006-dns-gethostbyname.patch" "dns" "0001" "warpgrid:shim/dns"
make_patch "${PATCHES}" "0007-dns-getnameinfo.patch" "dns" "0001" "warpgrid:shim/dns"

EXIT_CODE=0
OUTPUT=$(run_validate "${PATCHES}" --subset dns 2>&1) || EXIT_CODE=$?

if [ ${EXIT_CODE} -eq 0 ]; then
    pass "--validate --subset dns passes on DNS-only patches"
else
    fail "--validate --subset dns should pass (exit=${EXIT_CODE}). Output: ${OUTPUT}"
fi

# ── Test 8: --validate --subset filesystem validates FS-only subset ───

log "Test 8: --validate --subset filesystem validates filesystem-only subset"

PATCHES="${_tmpdir}/test8-patches"
mkdir -p "${PATCHES}"
# Filesystem-only subset: 0002, 0008 — should be self-consistent
make_patch "${PATCHES}" "0002-fs-fopen.patch" "filesystem" "none" "warpgrid:shim/filesystem"
make_patch "${PATCHES}" "0008-fs-timezone.patch" "filesystem" "0002" "warpgrid:shim/filesystem"

EXIT_CODE=0
OUTPUT=$(run_validate "${PATCHES}" --subset filesystem 2>&1) || EXIT_CODE=$?

if [ ${EXIT_CODE} -eq 0 ]; then
    pass "--validate --subset filesystem passes on FS-only patches"
else
    fail "--validate --subset filesystem should pass (exit=${EXIT_CODE}). Output: ${OUTPUT}"
fi

# ── Test 9: socket patches without filesystem produces clear error ────

log "Test 9: --validate reports clear error for socket without filesystem"

PATCHES="${_tmpdir}/test9-patches"
mkdir -p "${PATCHES}"
# Provide DNS (0001) and socket (0003) but NOT filesystem (0002)
# 0003 depends on 0001,0002 — should fail with clear message about filesystem
make_patch "${PATCHES}" "0001-dns.patch" "dns" "none" "warpgrid:shim/dns"
make_patch "${PATCHES}" "0003-socket.patch" "socket" "0001,0002" "warpgrid:shim/database-proxy.connect"
make_patch "${PATCHES}" "0004-socket.patch" "socket" "0003" "warpgrid:shim/database-proxy.send,warpgrid:shim/database-proxy.recv"
make_patch "${PATCHES}" "0005-socket.patch" "socket" "0004" "warpgrid:shim/database-proxy.close"

EXIT_CODE=0
OUTPUT=$(run_validate "${PATCHES}" 2>&1) || EXIT_CODE=$?

if [ ${EXIT_CODE} -ne 0 ] && echo "${OUTPUT}" | grep -qi "filesystem\|proxy.*config\|virtual.*FS\|0002"; then
    pass "--validate reports clear error for socket without filesystem"
else
    fail "--validate should report socket-needs-filesystem error (exit=${EXIT_CODE}). Output: ${OUTPUT}"
fi

# ── Test 10: --help includes --validate and --subset documentation ────

log "Test 10: --help includes --validate and --subset documentation"

EXIT_CODE=0
OUTPUT=$("${REBASE_LIBC}" --help 2>&1) || EXIT_CODE=$?

HAS_VALIDATE=false
HAS_SUBSET=false

if echo "${OUTPUT}" | grep -q -- "--validate"; then
    HAS_VALIDATE=true
fi
if echo "${OUTPUT}" | grep -q -- "--subset"; then
    HAS_SUBSET=true
fi

if ${HAS_VALIDATE} && ${HAS_SUBSET}; then
    pass "--help documents --validate and --subset flags"
elif ${HAS_VALIDATE}; then
    fail "--help has --validate but missing --subset"
else
    fail "--help missing --validate and/or --subset. Output: ${OUTPUT}"
fi

# ── Summary ──────────────────────────────────────────────────────────

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"

if [ "${FAIL}" -gt 0 ]; then
    exit 1
fi
exit 0
