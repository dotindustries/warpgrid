#!/usr/bin/env bash
#
# test-tinygo-patches.sh — Test harness for TinyGo patch rebase tooling.
#
# Validates the patch management infrastructure: UPSTREAM_REF format,
# rebase-tinygo.sh modes (validate, apply, export), and conflict detection.
#
# Why: ensures the patch tooling produces deterministic, reproducible results
# and that the WarpGrid TinyGo patch series can be maintained across upstream
# TinyGo releases without manual intervention.
#
# Usage:
#   scripts/test-tinygo-patches.sh              # Run all tests
#   scripts/test-tinygo-patches.sh --quick       # Skip network-dependent tests
#   scripts/test-tinygo-patches.sh --help        # Show this help
#
# Exit codes:
#   0  All tests passed
#   1  One or more tests failed
#   2  Configuration error

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
REBASE_SCRIPT="${SCRIPT_DIR}/rebase-tinygo.sh"
UPSTREAM_REF_FILE="${PROJECT_ROOT}/patches/tinygo/UPSTREAM_REF"
PATCHES_DIR="${PROJECT_ROOT}/patches/tinygo"

# Test counters
TOTAL=0
PASSED=0
FAILED=0
SKIPPED=0

# Mode
QUICK_MODE=false

# ─── Helpers ─────────────────────────────────────────────────────────────────

log() { echo "==> $*" >&2; }
err() { echo "ERROR: $*" >&2; exit 2; }

pass() {
    TOTAL=$((TOTAL + 1))
    PASSED=$((PASSED + 1))
    echo "  PASS  $1"
}

fail() {
    TOTAL=$((TOTAL + 1))
    FAILED=$((FAILED + 1))
    echo "  FAIL  $1"
    if [[ -n "${2:-}" ]]; then
        echo "        $2"
    fi
}

skip() {
    TOTAL=$((TOTAL + 1))
    SKIPPED=$((SKIPPED + 1))
    echo "  SKIP  $1 ($2)"
}

usage() {
    cat <<'USAGE'
test-tinygo-patches.sh — Test harness for TinyGo patch rebase tooling.

Usage:
  scripts/test-tinygo-patches.sh              # Run all tests
  scripts/test-tinygo-patches.sh --quick       # Skip network-dependent tests
  scripts/test-tinygo-patches.sh --help        # Show this help

Tests:
  - UPSTREAM_REF file format validation
  - rebase-tinygo.sh --validate on valid and invalid patches
  - rebase-tinygo.sh --apply applies patches cleanly (requires network)
  - rebase-tinygo.sh --export round-trip idempotency (requires network)
  - Conflict detection with fabricated bad patch (requires network)
USAGE
}

# ─── Test: UPSTREAM_REF format ───────────────────────────────────────────────

test_upstream_ref_exists() {
    if [[ -f "${UPSTREAM_REF_FILE}" ]]; then
        pass "UPSTREAM_REF file exists"
    else
        fail "UPSTREAM_REF file exists" "not found at ${UPSTREAM_REF_FILE}"
    fi
}

test_upstream_ref_has_tag() {
    local tag
    tag=$(grep '^TAG=' "${UPSTREAM_REF_FILE}" 2>/dev/null | cut -d= -f2 || true)
    if [[ -n "${tag}" ]]; then
        pass "UPSTREAM_REF contains TAG field (${tag})"
    else
        fail "UPSTREAM_REF contains TAG field" "TAG= line missing or empty"
    fi
}

test_upstream_ref_has_commit() {
    local commit
    commit=$(grep '^COMMIT=' "${UPSTREAM_REF_FILE}" 2>/dev/null | cut -d= -f2 || true)
    if [[ -n "${commit}" && ${#commit} -ge 40 ]]; then
        pass "UPSTREAM_REF contains COMMIT field (${commit:0:12}...)"
    else
        fail "UPSTREAM_REF contains COMMIT field" "COMMIT= line missing or not a full SHA"
    fi
}

# ─── Test: --validate mode ───────────────────────────────────────────────────

test_validate_succeeds() {
    if "${REBASE_SCRIPT}" --validate >/dev/null 2>&1; then
        pass "--validate succeeds with existing patches"
    else
        fail "--validate succeeds with existing patches" "exit code $?"
    fi
}

test_validate_detects_bad_numbering() {
    # Create a temporary patch with bad numbering (duplicate 0001)
    local bad_patch="${PATCHES_DIR}/0001-bad-duplicate-number.patch"
    cat > "${bad_patch}" <<'BADPATCH'
From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001
From: Test <test@test.com>
Date: Mon, 1 Jan 2024 00:00:00 +0000
Subject: [PATCH] bad patch

---
diff --git a/test.txt b/test.txt
new file mode 100644
--- /dev/null
+++ b/test.txt
@@ -0,0 +1 @@
+test
BADPATCH

    if "${REBASE_SCRIPT}" --validate >/dev/null 2>&1; then
        rm -f "${bad_patch}"
        fail "--validate detects bad numbering" "should have failed with duplicate 0001"
    else
        rm -f "${bad_patch}"
        pass "--validate detects bad numbering (duplicate 0001)"
    fi
}

test_validate_detects_non_patch() {
    # Create a file with .patch extension but no diff content
    local bad_patch="${PATCHES_DIR}/0099-not-a-real-patch.patch"
    echo "This is not a patch file" > "${bad_patch}"

    if "${REBASE_SCRIPT}" --validate >/dev/null 2>&1; then
        rm -f "${bad_patch}"
        fail "--validate detects non-patch file" "should have failed on file without diff --git"
    else
        rm -f "${bad_patch}"
        pass "--validate detects non-patch file"
    fi
}

# ─── Test: --apply mode ─────────────────────────────────────────────────────

test_apply_succeeds() {
    if [[ "${QUICK_MODE}" == "true" ]]; then
        skip "--apply applies patches cleanly" "requires network (use without --quick)"
        return
    fi

    local test_src="${PROJECT_ROOT}/build/test-tinygo-apply"
    rm -rf "${test_src}"

    if WARPGRID_TINYGO_SRC="${test_src}" "${REBASE_SCRIPT}" --apply >/dev/null 2>&1; then
        # Verify the patched file exists
        if [[ -f "${test_src}/src/internal/warpgrid/dns_wasip2.go" ]]; then
            pass "--apply applies patches cleanly"
        else
            fail "--apply applies patches cleanly" "patched file not found after apply"
        fi
    else
        fail "--apply applies patches cleanly" "rebase-tinygo.sh --apply failed"
    fi

    rm -rf "${test_src}"
}

test_apply_creates_expected_files() {
    if [[ "${QUICK_MODE}" == "true" ]]; then
        skip "--apply creates expected files" "requires network (use without --quick)"
        return
    fi

    local test_src="${PROJECT_ROOT}/build/test-tinygo-files"
    rm -rf "${test_src}"

    WARPGRID_TINYGO_SRC="${test_src}" "${REBASE_SCRIPT}" --apply >/dev/null 2>&1 || true

    if [[ -f "${test_src}/src/internal/warpgrid/dns_wasip2.go" ]]; then
        # Verify the file contains the wasmimport directive
        if grep -q 'go:wasmimport warpgrid_shim dns_resolve' "${test_src}/src/internal/warpgrid/dns_wasip2.go" 2>/dev/null; then
            pass "--apply creates files with expected content (wasmimport directive)"
        else
            fail "--apply creates files with expected content" "wasmimport directive not found"
        fi
    else
        fail "--apply creates expected files" "dns_wasip2.go not found"
    fi

    rm -rf "${test_src}"
}

# ─── Test: --export round-trip ───────────────────────────────────────────────

test_export_roundtrip() {
    if [[ "${QUICK_MODE}" == "true" ]]; then
        skip "--export round-trip idempotent" "requires network (use without --quick)"
        return
    fi

    local test_src="${PROJECT_ROOT}/build/test-tinygo-roundtrip"
    rm -rf "${test_src}"

    # Save original patches
    local orig_dir
    orig_dir=$(mktemp -d)
    cp "${PATCHES_DIR}"/*.patch "${orig_dir}/" 2>/dev/null || true

    # Apply then export
    WARPGRID_TINYGO_SRC="${test_src}" "${REBASE_SCRIPT}" --apply >/dev/null 2>&1
    WARPGRID_TINYGO_SRC="${test_src}" "${REBASE_SCRIPT}" --export >/dev/null 2>&1

    # Compare: same number of patches, same filenames
    local orig_count new_count
    orig_count=$(find "${orig_dir}" -name '*.patch' | wc -l)
    new_count=$(find "${PATCHES_DIR}" -maxdepth 1 -name '*.patch' | wc -l)

    if [[ "${orig_count}" -eq "${new_count}" && "${new_count}" -gt 0 ]]; then
        # Verify patch content is semantically equivalent (same diff hunks)
        local all_match=true
        for orig_patch in "${orig_dir}"/*.patch; do
            local patch_name
            patch_name=$(basename "${orig_patch}")
            local new_patch="${PATCHES_DIR}/${patch_name}"

            if [[ ! -f "${new_patch}" ]]; then
                all_match=false
                break
            fi

            # Compare the diff content (skip headers which may have different commit hashes)
            local orig_diff new_diff
            orig_diff=$(sed -n '/^diff --git/,$ p' "${orig_patch}")
            new_diff=$(sed -n '/^diff --git/,$ p' "${new_patch}")

            if [[ "${orig_diff}" != "${new_diff}" ]]; then
                all_match=false
                break
            fi
        done

        if [[ "${all_match}" == "true" ]]; then
            pass "--export round-trip produces identical patches"
        else
            fail "--export round-trip produces identical patches" "diff content changed"
        fi
    else
        fail "--export round-trip produces identical patches" "count mismatch: orig=${orig_count} new=${new_count}"
    fi

    # Restore original patches
    rm -f "${PATCHES_DIR}"/*.patch
    cp "${orig_dir}"/*.patch "${PATCHES_DIR}/" 2>/dev/null || true
    rm -rf "${orig_dir}" "${test_src}"
}

# ─── Test: conflict detection ────────────────────────────────────────────────

test_conflict_detection() {
    if [[ "${QUICK_MODE}" == "true" ]]; then
        skip "conflict detection reports clearly" "requires network (use without --quick)"
        return
    fi

    local test_src="${PROJECT_ROOT}/build/test-tinygo-conflict"
    rm -rf "${test_src}"

    # Save original patches
    local orig_dir
    orig_dir=$(mktemp -d)
    cp "${PATCHES_DIR}"/*.patch "${orig_dir}/" 2>/dev/null || true

    # Create a conflicting patch that tries to modify a file that doesn't exist
    # in a way that would conflict
    local conflict_patch="${PATCHES_DIR}/0002-deliberate-conflict.patch"
    cat > "${conflict_patch}" <<'CONFLICTPATCH'
From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001
From: Test <test@test.com>
Date: Mon, 1 Jan 2024 00:00:00 +0000
Subject: [PATCH 2/2] deliberate conflict

---
diff --git a/src/runtime/runtime_wasip2.go b/src/runtime/runtime_wasip2.go
--- a/src/runtime/runtime_wasip2.go
+++ b/src/runtime/runtime_wasip2.go
@@ -1,3 +1,4 @@
+// THIS LINE CONFLICTS WITH EVERYTHING
 this line does not exist in the real file
 neither does this one
 or this one
CONFLICTPATCH

    # Apply should fail on the conflicting patch
    local output
    output=$(WARPGRID_TINYGO_SRC="${test_src}" "${REBASE_SCRIPT}" --apply 2>&1 || true)

    if echo "${output}" | grep -qi "fail\|conflict\|error" 2>/dev/null; then
        pass "conflict detection reports clearly on bad patch"
    else
        fail "conflict detection reports clearly on bad patch" "no failure/conflict message in output"
    fi

    # Restore original patches
    rm -f "${PATCHES_DIR}"/*.patch
    cp "${orig_dir}"/*.patch "${PATCHES_DIR}/" 2>/dev/null || true
    rm -rf "${orig_dir}" "${test_src}"
}

# ─── Test: script interface ──────────────────────────────────────────────────

test_help_flag() {
    if "${REBASE_SCRIPT}" --help >/dev/null 2>&1; then
        pass "--help flag works"
    else
        fail "--help flag works" "non-zero exit code"
    fi
}

test_no_args_shows_usage() {
    local output
    output=$("${REBASE_SCRIPT}" 2>&1 || true)
    if echo "${output}" | grep -q "Modes:" 2>/dev/null; then
        pass "no-args shows usage"
    else
        fail "no-args shows usage" "usage text not found in output"
    fi
}

test_unknown_flag_errors() {
    if "${REBASE_SCRIPT}" --bogus >/dev/null 2>&1; then
        fail "unknown flag errors" "should have failed on --bogus"
    else
        pass "unknown flag errors correctly"
    fi
}

test_update_without_tag_errors() {
    if "${REBASE_SCRIPT}" --update 2>/dev/null; then
        fail "--update without tag errors" "should have failed without tag argument"
    else
        pass "--update without tag errors correctly"
    fi
}

test_src_flag_accepted() {
    # --src with --validate (no network needed) should work
    if "${REBASE_SCRIPT}" --validate --src /tmp >/dev/null 2>&1; then
        pass "--src flag is accepted"
    else
        fail "--src flag is accepted" "unexpected error with --src flag"
    fi
}

# ─── Test: --validate edge cases ─────────────────────────────────────────────

test_validate_detects_non_numeric_prefix() {
    local bad_patch="${PATCHES_DIR}/no-number-prefix.patch"
    cat > "${bad_patch}" <<'BADPATCH'
From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001
From: Test <test@test.com>
Date: Mon, 1 Jan 2024 00:00:00 +0000
Subject: [PATCH] patch without numeric prefix

---
diff --git a/test.txt b/test.txt
new file mode 100644
--- /dev/null
+++ b/test.txt
@@ -0,0 +1 @@
+test
BADPATCH

    if "${REBASE_SCRIPT}" --validate >/dev/null 2>&1; then
        rm -f "${bad_patch}"
        fail "--validate detects non-numeric prefix" "should have failed on patch without number"
    else
        rm -f "${bad_patch}"
        pass "--validate detects non-numeric prefix"
    fi
}

# ─── Test: --export without source checkout ──────────────────────────────────

test_export_without_checkout_errors() {
    local nonexistent="/tmp/test-tinygo-nonexistent-$$"
    rm -rf "${nonexistent}"

    local output
    output=$(WARPGRID_TINYGO_SRC="${nonexistent}" "${REBASE_SCRIPT}" --export 2>&1 || true)

    if echo "${output}" | grep -qi "not found\|error" 2>/dev/null; then
        pass "--export without source checkout errors clearly"
    else
        fail "--export without source checkout errors clearly" "no clear error message"
    fi
}

# ─── Test: malformed UPSTREAM_REF ────────────────────────────────────────────

test_malformed_upstream_ref_missing_tag() {
    # Temporarily replace UPSTREAM_REF with one missing TAG
    local backup
    backup=$(mktemp)
    cp "${UPSTREAM_REF_FILE}" "${backup}"

    echo "COMMIT=abc123" > "${UPSTREAM_REF_FILE}"

    if "${REBASE_SCRIPT}" --validate >/dev/null 2>&1; then
        # --validate doesn't call parse_upstream_ref, so try --apply instead
        local output
        output=$(WARPGRID_TINYGO_SRC="/tmp/test-$$" "${REBASE_SCRIPT}" --apply 2>&1 || true)
        cp "${backup}" "${UPSTREAM_REF_FILE}"
        rm -f "${backup}"

        if echo "${output}" | grep -qi "error\|must contain" 2>/dev/null; then
            pass "malformed UPSTREAM_REF (missing TAG) detected"
        else
            fail "malformed UPSTREAM_REF (missing TAG) detected" "no error on missing TAG"
        fi
    else
        cp "${backup}" "${UPSTREAM_REF_FILE}"
        rm -f "${backup}"
        pass "malformed UPSTREAM_REF (missing TAG) detected"
    fi
}

test_malformed_upstream_ref_missing_commit() {
    local backup
    backup=$(mktemp)
    cp "${UPSTREAM_REF_FILE}" "${backup}"

    echo "TAG=v0.40.0" > "${UPSTREAM_REF_FILE}"

    local output
    output=$(WARPGRID_TINYGO_SRC="/tmp/test-$$" "${REBASE_SCRIPT}" --apply 2>&1 || true)

    cp "${backup}" "${UPSTREAM_REF_FILE}"
    rm -f "${backup}"

    if echo "${output}" | grep -qi "error\|must contain" 2>/dev/null; then
        pass "malformed UPSTREAM_REF (missing COMMIT) detected"
    else
        fail "malformed UPSTREAM_REF (missing COMMIT) detected" "no error on missing COMMIT"
    fi
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
    while [[ $# -gt 0 ]]; do
        case "${1}" in
            --quick)    QUICK_MODE=true ;;
            --help|-h)  usage; exit 0 ;;
            *)          err "Unknown flag: ${1}. Use --help for usage." ;;
        esac
        shift
    done

    # Preconditions
    if [[ ! -x "${REBASE_SCRIPT}" ]]; then
        err "rebase-tinygo.sh not found at ${REBASE_SCRIPT}"
    fi

    echo
    log "Running TinyGo patch tooling tests..."
    echo

    # UPSTREAM_REF tests
    echo "--- UPSTREAM_REF validation ---"
    test_upstream_ref_exists
    test_upstream_ref_has_tag
    test_upstream_ref_has_commit
    echo

    # Script interface tests
    echo "--- Script interface ---"
    test_help_flag
    test_no_args_shows_usage
    test_unknown_flag_errors
    test_update_without_tag_errors
    test_src_flag_accepted
    echo

    # Validate mode tests
    echo "--- Validate mode ---"
    test_validate_succeeds
    test_validate_detects_bad_numbering
    test_validate_detects_non_patch
    test_validate_detects_non_numeric_prefix
    echo

    # Error handling tests
    echo "--- Error handling ---"
    test_export_without_checkout_errors
    test_malformed_upstream_ref_missing_tag
    test_malformed_upstream_ref_missing_commit
    echo

    # Apply mode tests (require network for git clone)
    echo "--- Apply mode ---"
    test_apply_succeeds
    test_apply_creates_expected_files
    echo

    # Export round-trip test
    echo "--- Export round-trip ---"
    test_export_roundtrip
    echo

    # Conflict detection
    echo "--- Conflict detection ---"
    test_conflict_detection
    echo

    # Summary
    echo "─────────────────────────────────────"
    echo "Total: ${TOTAL}  Passed: ${PASSED}  Failed: ${FAILED}  Skipped: ${SKIPPED}"
    echo "─────────────────────────────────────"

    if [[ ${FAILED} -gt 0 ]]; then
        echo "RESULT: FAIL"
        return 1
    fi

    echo "RESULT: PASS"
    return 0
}

main "$@"
