#!/usr/bin/env bash
#
# rebase-libc.sh — Apply, export, and rebase WarpGrid patches against upstream wasi-libc.
#
# Maintains the WarpGrid wasi-libc patch series as numbered `git format-patch` files
# in libc-patches/. Patches are applied to a clean upstream checkout using `git am --3way`
# and can be exported back after modification.
#
# Modes:
#   --apply     Apply all patches from libc-patches/*.patch onto the pinned upstream ref
#   --export    Regenerate libc-patches/*.patch files from the warpgrid branch
#   --update    Fetch a new upstream ref, attempt rebase, and report results
#   --validate  Check patch ordering and dependency constraints
#   --help      Show this help message
#
# Usage:
#   scripts/rebase-libc.sh --apply                     # Apply patches to fresh checkout
#   scripts/rebase-libc.sh --export                    # Export current branch as patches
#   scripts/rebase-libc.sh --update <tag>              # Rebase onto new upstream tag
#   scripts/rebase-libc.sh --validate                  # Check patch ordering/deps
#   scripts/rebase-libc.sh --help                      # Show this help
#
# Environment variables:
#   WARPGRID_LIBC_SRC   Path to wasi-libc source checkout (default: build/src-patched)
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
UPSTREAM_REF_FILE="${PROJECT_ROOT}/libc-patches/UPSTREAM_REF"
PATCHES_DIR="${PROJECT_ROOT}/libc-patches"
BUILD_DIR="${PROJECT_ROOT}/build"
DEFAULT_SRC_DIR="${BUILD_DIR}/src-patched"

# ─── Helpers ─────────────────────────────────────────────────────────────────

log() { echo "==> $*" >&2; }
warn() { echo "WARNING: $*" >&2; }
err() { echo "ERROR: $*" >&2; exit 1; }

usage() {
    cat <<'USAGE'
rebase-libc.sh — Apply, export, and rebase WarpGrid patches against upstream wasi-libc.

Modes:
  --apply         Apply all patches from libc-patches/*.patch onto a fresh
                  upstream checkout. Reports conflicting file names on failure.
  --export        Regenerate libc-patches/*.patch files from the current
                  warpgrid branch in the source checkout.
  --update <tag>  Fetch a new upstream tag, attempt to rebase patches onto it,
                  and report which patches applied/conflicted.
  --validate      Check patch ordering, numbering, and dependency constraints.
  --help          Show this help message.

Options:
  --src <path>    Path to wasi-libc source checkout
                  (default: build/src-patched)

Environment:
  WARPGRID_LIBC_SRC   Override default source checkout path

Examples:
  scripts/rebase-libc.sh --apply
  scripts/rebase-libc.sh --export
  scripts/rebase-libc.sh --update wasi-sdk-31
  scripts/rebase-libc.sh --validate
USAGE
}

# Parse UPSTREAM_REF file — sets UPSTREAM_TAG and UPSTREAM_COMMIT
parse_upstream_ref() {
    if [[ ! -f "${UPSTREAM_REF_FILE}" ]]; then
        err "UPSTREAM_REF file not found at ${UPSTREAM_REF_FILE}"
    fi

    UPSTREAM_TAG=$(grep '^TAG=' "${UPSTREAM_REF_FILE}" | cut -d= -f2)
    UPSTREAM_COMMIT=$(grep '^COMMIT=' "${UPSTREAM_REF_FILE}" | cut -d= -f2)

    if [[ -z "${UPSTREAM_TAG}" || -z "${UPSTREAM_COMMIT}" ]]; then
        err "UPSTREAM_REF must contain TAG=<tag> and COMMIT=<hash>"
    fi
}

# Collect patch files sorted by name
collect_patches() {
    local patches=()
    while IFS= read -r -d '' patch; do
        patches+=("${patch}")
    done < <(find "${PATCHES_DIR}" -maxdepth 1 -name '*.patch' -print0 | sort -z)
    PATCH_FILES=("${patches[@]+"${patches[@]}"}")
}

# Ensure a clean source checkout at the pinned upstream commit
ensure_clean_checkout() {
    local src_dir="${1}"

    if [[ -d "${src_dir}/.git" ]]; then
        local current_commit
        current_commit=$(git -C "${src_dir}" rev-parse HEAD 2>/dev/null || echo "unknown")

        # Check if the repo is clean and at the right base
        local base_commit
        base_commit=$(git -C "${src_dir}" log --format='%H' --reverse | head -1)

        if [[ "${base_commit}" == "${UPSTREAM_COMMIT}" ]]; then
            # Reset to upstream commit for a clean apply
            log "Resetting ${src_dir} to upstream ${UPSTREAM_COMMIT:0:12}..."
            git -C "${src_dir}" reset --hard "${UPSTREAM_COMMIT}" 2>/dev/null
            git -C "${src_dir}" clean -fd 2>/dev/null
            return
        fi

        log "Source at ${src_dir} is not based on upstream, re-cloning..."
        rm -rf "${src_dir}"
    fi

    log "Cloning wasi-libc at ${UPSTREAM_TAG} (${UPSTREAM_COMMIT:0:12})..."
    mkdir -p "$(dirname "${src_dir}")"
    git clone --depth 50 --branch "${UPSTREAM_TAG}" \
        https://github.com/WebAssembly/wasi-libc.git "${src_dir}" 2>&1

    local actual_commit
    actual_commit=$(git -C "${src_dir}" rev-parse HEAD)
    if [[ "${actual_commit}" != "${UPSTREAM_COMMIT}" ]]; then
        err "Commit mismatch: expected ${UPSTREAM_COMMIT}, got ${actual_commit}"
    fi
}

# ─── --apply ──────────────────────────────────────────────────────────────────

do_apply() {
    local src_dir="${1}"

    parse_upstream_ref
    collect_patches

    if [[ ${#PATCH_FILES[@]} -eq 0 ]]; then
        log "No patches found in ${PATCHES_DIR}/ — nothing to apply"
        return 0
    fi

    ensure_clean_checkout "${src_dir}"

    log "Applying ${#PATCH_FILES[@]} patch(es) to ${src_dir}..."

    local applied=0
    local failed=0
    local failed_patches=()

    for patch in "${PATCH_FILES[@]}"; do
        local patch_name
        patch_name=$(basename "${patch}")

        if git -C "${src_dir}" am --3way "${patch}" 2>/dev/null; then
            log "  OK  ${patch_name}"
            applied=$((applied + 1))
        else
            # Capture conflicting files
            local conflict_files
            conflict_files=$(git -C "${src_dir}" diff --name-only --diff-filter=U 2>/dev/null || true)
            if [[ -z "${conflict_files}" ]]; then
                # git am failure without merge conflicts (e.g., patch doesn't apply at all)
                conflict_files=$(git -C "${src_dir}" am --show-current-patch 2>/dev/null | grep '^diff --git' | sed 's|diff --git a/\(.*\) b/.*|\1|' || echo "(unknown)")
            fi

            warn "  FAIL ${patch_name}"
            if [[ -n "${conflict_files}" ]]; then
                echo "${conflict_files}" | while IFS= read -r f; do
                    warn "    conflict: ${f}"
                done
            fi

            failed=$((failed + 1))
            failed_patches+=("${patch_name}")

            # Abort the failed am to clean up
            git -C "${src_dir}" am --abort 2>/dev/null || true
            break  # Stop at first failure (patches are sequential)
        fi
    done

    echo
    log "Apply summary: ${applied} applied, ${failed} failed out of ${#PATCH_FILES[@]} total"

    if [[ ${failed} -gt 0 ]]; then
        warn "Failed patches: ${failed_patches[*]}"
        warn "Run 'scripts/rebase-libc.sh --update <tag>' to rebase onto a new upstream."
        return 1
    fi

    log "All patches applied successfully"
    return 0
}

# ─── --export ────────────────────────────────────────────────────────────────

do_export() {
    local src_dir="${1}"

    parse_upstream_ref

    if [[ ! -d "${src_dir}/.git" ]]; then
        err "Source checkout not found at ${src_dir}. Run --apply first or specify --src."
    fi

    # Count commits on top of the upstream base
    local patch_count
    patch_count=$(git -C "${src_dir}" rev-list "${UPSTREAM_COMMIT}..HEAD" --count 2>/dev/null || echo "0")

    if [[ "${patch_count}" -eq 0 ]]; then
        log "No commits on top of upstream — nothing to export"
        return 0
    fi

    log "Exporting ${patch_count} patch(es) from ${src_dir}..."

    # Remove old patch files
    local old_count=0
    while IFS= read -r -d '' old_patch; do
        rm -f "${old_patch}"
        old_count=$((old_count + 1))
    done < <(find "${PATCHES_DIR}" -maxdepth 1 -name '*.patch' -print0)

    if [[ ${old_count} -gt 0 ]]; then
        log "Removed ${old_count} old patch file(s)"
    fi

    # Generate new patches
    git -C "${src_dir}" format-patch \
        --output-directory "${PATCHES_DIR}" \
        --numbered \
        "${UPSTREAM_COMMIT}..HEAD" 2>/dev/null

    # List exported patches
    collect_patches
    for patch in "${PATCH_FILES[@]}"; do
        log "  $(basename "${patch}")"
    done

    log "Exported ${#PATCH_FILES[@]} patch(es) to ${PATCHES_DIR}/"
    return 0
}

# ─── --update ────────────────────────────────────────────────────────────────

do_update() {
    local new_tag="${1}"
    local src_dir="${2}"

    if [[ -z "${new_tag}" ]]; then
        err "--update requires a tag argument (e.g., --update wasi-sdk-31)"
    fi

    parse_upstream_ref
    collect_patches

    if [[ ${#PATCH_FILES[@]} -eq 0 ]]; then
        log "No patches to rebase — just updating UPSTREAM_REF"
        update_upstream_ref "${new_tag}" "${src_dir}"
        return 0
    fi

    log "Rebasing ${#PATCH_FILES[@]} patch(es) from ${UPSTREAM_TAG} onto ${new_tag}..."

    # Clone fresh at the new tag
    local rebase_dir="${BUILD_DIR}/src-rebase-${new_tag}"
    rm -rf "${rebase_dir}"
    mkdir -p "$(dirname "${rebase_dir}")"

    log "Cloning wasi-libc at ${new_tag}..."
    if ! git clone --depth 50 --branch "${new_tag}" \
        https://github.com/WebAssembly/wasi-libc.git "${rebase_dir}" 2>&1; then
        err "Failed to clone wasi-libc at tag ${new_tag}. Is the tag valid?"
    fi

    local new_commit
    new_commit=$(git -C "${rebase_dir}" rev-parse HEAD)
    log "New upstream commit: ${new_commit:0:12}"

    # Try applying patches onto new base
    local applied=0
    local failed=0
    local results=()

    for patch in "${PATCH_FILES[@]}"; do
        local patch_name
        patch_name=$(basename "${patch}")

        if git -C "${rebase_dir}" am --3way "${patch}" 2>/dev/null; then
            results+=("  OK       ${patch_name}")
            applied=$((applied + 1))
        else
            local conflict_files
            conflict_files=$(git -C "${rebase_dir}" diff --name-only --diff-filter=U 2>/dev/null || true)
            results+=("  CONFLICT ${patch_name}")
            if [[ -n "${conflict_files}" ]]; then
                while IFS= read -r f; do
                    results+=("             → ${f}")
                done <<< "${conflict_files}"
            fi

            failed=$((failed + 1))
            git -C "${rebase_dir}" am --abort 2>/dev/null || true
            break
        fi
    done

    echo
    log "Rebase summary for ${UPSTREAM_TAG} → ${new_tag}:"
    for line in "${results[@]}"; do
        echo "  ${line}" >&2
    done
    echo
    log "${applied} applied, ${failed} failed out of ${#PATCH_FILES[@]} total"

    if [[ ${failed} -gt 0 ]]; then
        warn "Rebase has conflicts. Resolve manually in ${rebase_dir}/ then run --export."
        rm -rf "${rebase_dir}"
        return 1
    fi

    # Success — update UPSTREAM_REF and export new patches
    log "All patches apply cleanly onto ${new_tag}!"

    # Update UPSTREAM_REF
    local today
    today=$(date +%Y-%m-%d)
    cat > "${UPSTREAM_REF_FILE}" <<EOF
# wasi-libc upstream pin
# This file records the exact upstream commit that the WarpGrid patch series applies to.
# Update this file when rebasing patches onto a new upstream version.
#
# Repository: https://github.com/WebAssembly/wasi-libc
# Last updated: ${today}
TAG=${new_tag}
COMMIT=${new_commit}
EOF
    log "Updated UPSTREAM_REF to ${new_tag} (${new_commit:0:12})"

    # Export rebased patches
    local old_count=0
    while IFS= read -r -d '' old_patch; do
        rm -f "${old_patch}"
        old_count=$((old_count + 1))
    done < <(find "${PATCHES_DIR}" -maxdepth 1 -name '*.patch' -print0)

    git -C "${rebase_dir}" format-patch \
        --output-directory "${PATCHES_DIR}" \
        --numbered \
        "${new_commit}..HEAD" 2>/dev/null

    # Move rebased source into place
    rm -rf "${src_dir}"
    mv "${rebase_dir}" "${src_dir}"

    log "Rebase complete. Run 'scripts/build-libc.sh --patched' to verify."
    return 0
}

update_upstream_ref() {
    local new_tag="${1}"
    local src_dir="${2}"

    local rebase_dir="${BUILD_DIR}/src-rebase-${new_tag}"
    rm -rf "${rebase_dir}"

    log "Cloning wasi-libc at ${new_tag}..."
    if ! git clone --depth 50 --branch "${new_tag}" \
        https://github.com/WebAssembly/wasi-libc.git "${rebase_dir}" 2>&1; then
        err "Failed to clone wasi-libc at tag ${new_tag}. Is the tag valid?"
    fi

    local new_commit
    new_commit=$(git -C "${rebase_dir}" rev-parse HEAD)

    local today
    today=$(date +%Y-%m-%d)
    cat > "${UPSTREAM_REF_FILE}" <<EOF
# wasi-libc upstream pin
# This file records the exact upstream commit that the WarpGrid patch series applies to.
# Update this file when rebasing patches onto a new upstream version.
#
# Repository: https://github.com/WebAssembly/wasi-libc
# Last updated: ${today}
TAG=${new_tag}
COMMIT=${new_commit}
EOF

    rm -rf "${src_dir}"
    mv "${rebase_dir}" "${src_dir}"

    log "Updated UPSTREAM_REF to ${new_tag} (${new_commit:0:12})"
}

# ─── --validate ──────────────────────────────────────────────────────────────

do_validate() {
    collect_patches

    if [[ ${#PATCH_FILES[@]} -eq 0 ]]; then
        log "No patches found — nothing to validate"
        return 0
    fi

    log "Validating ${#PATCH_FILES[@]} patch(es)..."

    local errors=0
    local prev_num=0
    local seen_numbers=""

    for patch in "${PATCH_FILES[@]}"; do
        local patch_name
        patch_name=$(basename "${patch}")

        # Check numbering: must start with numeric prefix
        local num_str
        num_str=$(echo "${patch_name}" | grep -oE '^[0-9]+' || true)
        if [[ -z "${num_str}" ]]; then
            warn "  ${patch_name}: missing numeric prefix"
            errors=$((errors + 1))
            continue
        fi

        # Check sequential ordering
        local num=$((10#${num_str}))
        if [[ ${num} -le ${prev_num} ]]; then
            warn "  ${patch_name}: out of order (${num} <= ${prev_num})"
            errors=$((errors + 1))
        fi
        prev_num=${num}
        seen_numbers="${seen_numbers} ${num_str}"

        # Check that patch file is valid (has diff content)
        if ! grep -q '^diff --git' "${patch}" 2>/dev/null; then
            warn "  ${patch_name}: does not appear to be a valid git patch"
            errors=$((errors + 1))
        fi

        # Check dependency constraints (portable — no associative arrays)
        # Known deps:
        #   0003→0001 (socket-connect depends on dns-getaddrinfo for proxy config via FS)
        #   0004→0003 (socket-send-recv depends on socket-connect)
        #   0005→0004 (socket-close depends on socket-send-recv)
        #   0006→0001 (gethostbyname depends on dns-getaddrinfo shim)
        #   0007→0001 (getnameinfo depends on dns-getaddrinfo shim)
        local dep=""
        case "${num_str}" in
            0003)      dep="0001" ;;
            0004)      dep="0003" ;;
            0005)      dep="0004" ;;
            0006|0007) dep="0001" ;;
        esac

        if [[ -n "${dep}" ]]; then
            if ! echo "${seen_numbers}" | grep -qw "${dep}"; then
                warn "  ${patch_name}: requires patch ${dep} which is not present or comes later"
                errors=$((errors + 1))
            fi
        fi

        log "  OK  ${patch_name}"
    done

    echo
    if [[ ${errors} -gt 0 ]]; then
        warn "Validation found ${errors} issue(s)"
        return 1
    fi

    log "All patches valid"
    return 0
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
    local mode=""
    local update_tag=""
    local src_dir="${WARPGRID_LIBC_SRC:-${DEFAULT_SRC_DIR}}"

    if [[ $# -eq 0 ]]; then
        usage
        exit 1
    fi

    while [[ $# -gt 0 ]]; do
        case "${1}" in
            --apply)    mode="apply" ;;
            --export)   mode="export" ;;
            --update)
                mode="update"
                if [[ $# -lt 2 ]]; then
                    err "--update requires a tag argument (e.g., --update wasi-sdk-31)"
                fi
                shift
                update_tag="${1}"
                ;;
            --validate) mode="validate" ;;
            --src)
                if [[ $# -lt 2 ]]; then
                    err "--src requires a path argument"
                fi
                shift
                src_dir="${1}"
                ;;
            --help|-h)  usage; exit 0 ;;
            *)          err "Unknown flag: ${1}. Use --help for usage." ;;
        esac
        shift
    done

    if [[ -z "${mode}" ]]; then
        err "Must specify a mode: --apply, --export, --update, or --validate"
    fi

    case "${mode}" in
        apply)    do_apply "${src_dir}" ;;
        export)   do_export "${src_dir}" ;;
        update)   do_update "${update_tag}" "${src_dir}" ;;
        validate) do_validate ;;
    esac
}

main "$@"
