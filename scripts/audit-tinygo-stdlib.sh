#!/usr/bin/env bash
# audit-tinygo-stdlib.sh — Audit Go stdlib package compatibility with TinyGo wasip2
#
# US-302: Audit Go stdlib compatibility for wasip2 target
#
# Usage:
#   scripts/audit-tinygo-stdlib.sh [--build|--table|--json|--test|--all]
#
# Modes:
#   --build    Run TinyGo wasip2 builds for all 20 stdlib packages
#   --table    Generate human-readable markdown compatibility table
#   --json     Print compatibility results as JSON
#   --test     Run Go test suite (validates file structure + JSON schema)
#   --all      Run build + table + test (default)
#
# Exit codes:
#   0 — Success (individual package failures are expected and recorded)
#   1 — Script error or prerequisites missing
#   2 — Go tests failed

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
FIXTURE_DIR="${PROJECT_ROOT}/tests/fixtures/go-stdlib-compat"
COMPAT_DB="${PROJECT_ROOT}/compat-db"
JSON_OUTPUT="${COMPAT_DB}/tinygo-stdlib.json"
TABLE_OUTPUT="${COMPAT_DB}/tinygo-stdlib-compat-table.md"
TINYGO_BIN="${PROJECT_ROOT}/build/tinygo/bin/tinygo"
TMP_DIR="/tmp/tinygo-stdlib-audit"

MODE="${1:---all}"

log() { echo "==> $*" >&2; }
err() { echo "ERROR: $*" >&2; }

# Map directory name to Go import path
dir_to_import_path() {
  local dir="$1"
  case "${dir}" in
    pkg_fmt)             echo "fmt" ;;
    pkg_strings)         echo "strings" ;;
    pkg_strconv)         echo "strconv" ;;
    pkg_encoding_json)   echo "encoding/json" ;;
    pkg_encoding_base64) echo "encoding/base64" ;;
    pkg_crypto_sha256)   echo "crypto/sha256" ;;
    pkg_crypto_tls)      echo "crypto/tls" ;;
    pkg_math)            echo "math" ;;
    pkg_sort)            echo "sort" ;;
    pkg_bytes)           echo "bytes" ;;
    pkg_io)              echo "io" ;;
    pkg_os)              echo "os" ;;
    pkg_net)             echo "net" ;;
    pkg_net_http)        echo "net/http" ;;
    pkg_database_sql)    echo "database/sql" ;;
    pkg_context)         echo "context" ;;
    pkg_sync)            echo "sync" ;;
    pkg_time)            echo "time" ;;
    pkg_regexp)          echo "regexp" ;;
    pkg_log)             echo "log" ;;
    *) echo "${dir}" ;;
  esac
}

# Map directory name to human-friendly package name
dir_to_name() {
  local dir="$1"
  dir_to_import_path "${dir}"
}

find_tinygo() {
  if [ -x "${TINYGO_BIN}" ]; then
    echo "${TINYGO_BIN}"
  elif command -v tinygo &>/dev/null; then
    command -v tinygo
  else
    echo ""
  fi
}

get_tinygo_version() {
  local bin="$1"
  "${bin}" version 2>&1 | head -1 || echo "unknown"
}

# Escape a string for safe JSON embedding
json_escape() {
  local s="$1"
  # Use python if available, otherwise sed
  if command -v python3 &>/dev/null; then
    python3 -c "import json,sys; print(json.dumps(sys.stdin.read()), end='')" <<< "${s}"
  else
    # Basic escaping: backslash, quotes, newlines
    echo -n "\"$(echo "${s}" | sed 's/\\/\\\\/g; s/"/\\"/g' | tr '\n' '|' | sed 's/|/\\n/g')\""
  fi
}

run_build() {
  local tinygo_bin
  tinygo_bin="$(find_tinygo)"

  if [ -z "${tinygo_bin}" ]; then
    err "TinyGo not found. Install via: scripts/build-tinygo.sh"
    err "Looked in: ${TINYGO_BIN} and system PATH"
    exit 1
  fi

  local tinygo_version
  tinygo_version="$(get_tinygo_version "${tinygo_bin}")"
  local go_version
  go_version="$(go version 2>&1 | sed 's/go version go//' | cut -d' ' -f1)"
  local tested_at
  tested_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

  log "TinyGo: ${tinygo_version}"
  log "Go: ${go_version}"
  log "Target: wasip2"
  log "Fixtures: ${FIXTURE_DIR}"
  echo ""

  mkdir -p "${TMP_DIR}" "${COMPAT_DB}"

  # Collect results as JSON array entries
  local packages_json=""
  local pkg_count=0
  local pass_count=0
  local fail_count=0
  local partial_count=0

  # Sort directories for deterministic output
  for pkg_dir in $(find "${FIXTURE_DIR}" -maxdepth 1 -type d -name 'pkg_*' | sort); do
    local dir_name
    dir_name="$(basename "${pkg_dir}")"
    local import_path
    import_path="$(dir_to_import_path "${dir_name}")"
    local pkg_name
    pkg_name="$(dir_to_name "${dir_name}")"

    pkg_count=$((pkg_count + 1))
    local wasm_out="${TMP_DIR}/${dir_name}.wasm"

    printf "  [%2d/20] %-20s " "${pkg_count}" "${pkg_name}"

    # Run TinyGo build, capture stderr
    local compile_output=""
    local compile_status=""
    local exit_code=0

    compile_output=$("${tinygo_bin}" build -target=wasip2 -o "${wasm_out}" "${pkg_dir}/main.go" 2>&1) || exit_code=$?

    if [ ${exit_code} -eq 0 ]; then
      compile_status="pass"
      pass_count=$((pass_count + 1))
      echo "PASS"
    else
      # Check if it's a partial failure (compiled but with warnings)
      if [ -f "${wasm_out}" ]; then
        compile_status="partial"
        partial_count=$((partial_count + 1))
        echo "PARTIAL"
      else
        compile_status="fail"
        fail_count=$((fail_count + 1))
        echo "FAIL"
      fi
    fi

    # Build JSON entry for this package using jq for safe escaping
    local entry
    if [ "${compile_status}" != "pass" ] && [ -n "${compile_output}" ]; then
      local error_count
      error_count=$(echo "${compile_output}" | grep -c 'error:\|Error\|cannot\|undefined\|not declared\|missing' || true)
      error_count=${error_count:-0}

      # Use jq to safely construct JSON with proper escaping
      local errors_array
      errors_array=$(echo "${compile_output}" | head -20 | jq -R -s 'split("\n") | map(select(length > 0))')

      local missing_array
      missing_array=$(echo "${compile_output}" | grep -oP 'undefined: \K[a-zA-Z0-9_.]+' 2>/dev/null | jq -R -s 'split("\n") | map(select(length > 0))' || echo "[]")

      local notes="TinyGo wasip2 compilation failed with ${error_count} error(s)"

      entry=$(jq -n \
        --arg name "${pkg_name}" \
        --arg importPath "${import_path}" \
        --arg status "${compile_status}" \
        --argjson errors "${errors_array}" \
        --argjson errorCount "${error_count}" \
        --argjson missing "${missing_array}" \
        --arg notes "${notes}" \
        '{name: $name, importPath: $importPath, compileStatus: $status, errors: $errors, errorCount: $errorCount, missingSymbols: $missing, notes: $notes}')
    else
      entry=$(jq -n \
        --arg name "${pkg_name}" \
        --arg importPath "${import_path}" \
        --arg status "${compile_status}" \
        '{name: $name, importPath: $importPath, compileStatus: $status, errors: [], errorCount: 0, missingSymbols: [], notes: "Compiles successfully with TinyGo wasip2"}')
    fi

    # Accumulate entries into a temp file
    echo "${entry}" >> "${TMP_DIR}/results.jsonl"
  done

  # Extract compiler version number
  local compiler_version
  compiler_version=$(echo "${tinygo_version}" | grep -oP '\d+\.\d+\.\d+' | head -1 || echo "unknown")

  # Assemble final JSON from collected entries using jq
  jq -n \
    --arg compiler "tinygo" \
    --arg compilerVersion "${compiler_version}" \
    --arg target "wasip2" \
    --arg testedAt "${tested_at}" \
    --arg goVersion "${go_version}" \
    --arg userStory "US-302" \
    --argjson packageCount "${pkg_count}" \
    --slurpfile packages "${TMP_DIR}/results.jsonl" \
    '{compiler: $compiler, compilerVersion: $compilerVersion, target: $target, testedAt: $testedAt, goVersion: $goVersion, userStory: $userStory, packageCount: $packageCount, packages: $packages}' \
    > "${JSON_OUTPUT}"

  echo ""
  log "Results written to ${JSON_OUTPUT}"
  log "Summary: ${pass_count} pass, ${fail_count} fail, ${partial_count} partial (${pkg_count} total)"

  # Validate count
  if [ "${pkg_count}" -ne 20 ]; then
    err "Expected 20 packages, found ${pkg_count}"
    exit 1
  fi

  # Clean up temp files
  rm -rf "${TMP_DIR}"
}

generate_table() {
  if [ ! -f "${JSON_OUTPUT}" ]; then
    err "JSON results not found: ${JSON_OUTPUT}"
    err "Run with --build first to generate results."
    exit 1
  fi

  log "Generating compatibility table..."

  # Use jq to generate the markdown table
  if ! command -v jq &>/dev/null; then
    err "jq not found. Install jq to generate markdown tables."
    exit 1
  fi

  local compiler_version target tested_at go_version pkg_count
  compiler_version=$(jq -r '.compilerVersion' "${JSON_OUTPUT}")
  target=$(jq -r '.target' "${JSON_OUTPUT}")
  tested_at=$(jq -r '.testedAt' "${JSON_OUTPUT}")
  go_version=$(jq -r '.goVersion' "${JSON_OUTPUT}")
  pkg_count=$(jq -r '.packageCount' "${JSON_OUTPUT}")

  local pass_count fail_count partial_count
  pass_count=$(jq '[.packages[] | select(.compileStatus == "pass")] | length' "${JSON_OUTPUT}")
  fail_count=$(jq '[.packages[] | select(.compileStatus == "fail")] | length' "${JSON_OUTPUT}")
  partial_count=$(jq '[.packages[] | select(.compileStatus == "partial")] | length' "${JSON_OUTPUT}")

  {
    echo "# TinyGo wasip2 Standard Library Compatibility"
    echo ""
    echo "**Compiler**: TinyGo ${compiler_version}"
    echo "**Target**: ${target}"
    echo "**Go Version**: ${go_version}"
    echo "**Tested**: ${tested_at}"
    echo "**User Story**: US-302"
    echo ""
    echo "## Summary"
    echo ""
    echo "- **Pass**: ${pass_count}/${pkg_count} packages compile successfully"
    echo "- **Fail**: ${fail_count}/${pkg_count} packages fail to compile"
    echo "- **Partial**: ${partial_count}/${pkg_count} packages compile with warnings"
    echo ""
    echo "## Compatibility Table"
    echo ""
    echo "| Package | Import Path | Status | Errors | Notes |"
    echo "|---------|-------------|--------|--------|-------|"

    # Generate table rows using jq
    jq -r '.packages[] | "| \(.name) | \(.importPath) | \(.compileStatus) | \(.errorCount) | \(.notes) |"' "${JSON_OUTPUT}"

    echo ""
    echo "---"
    echo ""
    echo "*Generated by \`scripts/audit-tinygo-stdlib.sh --table\`*"
  } > "${TABLE_OUTPUT}"

  log "Table written to ${TABLE_OUTPUT}"
}

show_json() {
  if [ ! -f "${JSON_OUTPUT}" ]; then
    err "JSON results not found: ${JSON_OUTPUT}"
    err "Run with --build first to generate results."
    exit 1
  fi

  if command -v jq &>/dev/null; then
    jq '.' "${JSON_OUTPUT}"
  else
    cat "${JSON_OUTPUT}"
  fi
}

run_tests() {
  log "Running Go test suite..."

  cd "${FIXTURE_DIR}"

  if ! go vet ./... 2>&1; then
    err "go vet failed"
    exit 2
  fi

  if ! go test -v -count=1 -timeout=120s . 2>&1; then
    err "Go tests failed"
    exit 2
  fi

  log "All Go tests passed."
}

case "${MODE}" in
  --build)  run_build ;;
  --table)  generate_table ;;
  --json)   show_json ;;
  --test)   run_tests ;;
  --all)    run_build; generate_table; run_tests ;;
  --help|-h)
    echo "Usage: $0 [--build|--table|--json|--test|--all]"
    echo ""
    echo "Modes:"
    echo "  --build    Run TinyGo wasip2 builds for all 20 stdlib packages"
    echo "  --table    Generate human-readable markdown compatibility table"
    echo "  --json     Print compatibility results as JSON"
    echo "  --test     Run Go test suite (validates file structure + JSON schema)"
    echo "  --all      Run build + table + test (default)"
    echo ""
    echo "Prerequisites:"
    echo "  - Go 1.22+ (go command in PATH)"
    echo "  - TinyGo 0.40+ (build/tinygo/bin/tinygo or system PATH)"
    echo "  - jq (for --table mode)"
    ;;
  *)
    echo "Unknown option: ${MODE}"
    echo "Run $0 --help for usage"
    exit 1
    ;;
esac
