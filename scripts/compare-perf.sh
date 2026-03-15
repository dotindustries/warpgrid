#!/usr/bin/env bash
#
# compare-perf.sh — Regression detection for WarpGrid performance baselines.
#
# Compares two performance-baseline.json files and detects regressions
# in p95 latency values. Outputs a formatted diff table.
#
# Usage:
#   compare-perf.sh [OPTIONS]
#
# Options:
#   --baseline PATH    Baseline JSON file (default: git show main:test-results/performance-baseline.json)
#   --current PATH     Current JSON file (default: test-results/performance-baseline.json)
#   --threshold N      Regression threshold percentage (default: 20)
#   --help             Show this help message
#
# Exit codes:
#   0  No regression detected
#   1  >threshold% p95 regression detected
#   2  Error / missing data

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Defaults
BASELINE_PATH=""
CURRENT_PATH="${PROJECT_ROOT}/test-results/performance-baseline.json"
THRESHOLD=20

# Colors
if [ -t 1 ]; then
  RED='\033[0;31m'
  GREEN='\033[0;32m'
  YELLOW='\033[0;33m'
  BLUE='\033[0;34m'
  NC='\033[0m'
else
  RED='' GREEN='' YELLOW='' BLUE='' NC=''
fi

# ─── Argument parsing ─────────────────────────────────────────────────────────

usage() {
  cat <<'EOF'
Usage: compare-perf.sh [OPTIONS]

Compare two WarpGrid performance baseline JSON files and detect regressions.
Outputs a formatted diff table showing per-app p50/p95/p99 changes.

Options:
  --baseline PATH    Baseline JSON file
                     Default: fetched from main branch via git
  --current PATH     Current JSON file
                     Default: test-results/performance-baseline.json
  --threshold N      Regression threshold percentage (default: 20)
                     A >N% increase in p95 triggers a warning exit
  --help             Show this help message

Exit codes:
  0  No regression detected
  1  Regression detected (p95 increase > threshold)
  2  Error or missing data

Examples:
  compare-perf.sh --baseline old.json --current new.json
  compare-perf.sh --threshold 30
EOF
}

parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --baseline)
        BASELINE_PATH="$2"
        shift 2
        ;;
      --baseline=*)
        BASELINE_PATH="${1#--baseline=}"
        shift
        ;;
      --current)
        CURRENT_PATH="$2"
        shift 2
        ;;
      --current=*)
        CURRENT_PATH="${1#--current=}"
        shift
        ;;
      --threshold)
        THRESHOLD="$2"
        shift 2
        ;;
      --threshold=*)
        THRESHOLD="${1#--threshold=}"
        shift
        ;;
      --help|-h)
        usage
        exit 0
        ;;
      *)
        echo "Error: Unknown option '$1'. Use --help for usage." >&2
        exit 2
        ;;
    esac
  done
}

# ─── Fetch baseline from main branch ──────────────────────────────────────────

fetch_main_baseline() {
  local tmpfile
  tmpfile=$(mktemp /tmp/perf-baseline-main-XXXXXX.json)

  if git -C "$PROJECT_ROOT" show main:test-results/performance-baseline.json > "$tmpfile" 2>/dev/null; then
    echo "$tmpfile"
  else
    rm -f "$tmpfile"
    echo ""
  fi
}

# ─── Compute delta percentage ─────────────────────────────────────────────────

compute_delta() {
  local baseline="$1" current="$2"
  awk -v b="$baseline" -v c="$current" 'BEGIN {
    if (b == 0) { printf "0.0"; exit }
    printf "%.1f", (c - b) / b * 100
  }'
}

# ─── Main ──────────────────────────────────────────────────────────────────────

main() {
  parse_args "$@"

  # Validate prerequisites
  if ! command -v jq &>/dev/null; then
    echo "Error: jq is required" >&2
    exit 2
  fi

  # Resolve baseline
  local baseline_tmpfile=""
  if [ -z "$BASELINE_PATH" ]; then
    baseline_tmpfile=$(fetch_main_baseline)
    if [ -z "$baseline_tmpfile" ]; then
      echo "Error: could not fetch baseline from main branch" >&2
      echo "Use --baseline PATH to specify a baseline file" >&2
      exit 2
    fi
    BASELINE_PATH="$baseline_tmpfile"
  fi

  # Validate files exist
  if [ ! -f "$BASELINE_PATH" ]; then
    echo "Error: baseline file not found: ${BASELINE_PATH}" >&2
    exit 2
  fi

  if [ ! -f "$CURRENT_PATH" ]; then
    echo "Error: current file not found: ${CURRENT_PATH}" >&2
    exit 2
  fi

  # Validate JSON
  if ! jq . "$BASELINE_PATH" >/dev/null 2>&1; then
    echo "Error: invalid JSON in baseline file" >&2
    exit 2
  fi
  if ! jq . "$CURRENT_PATH" >/dev/null 2>&1; then
    echo "Error: invalid JSON in current file" >&2
    exit 2
  fi

  # Header
  local baseline_sha baseline_branch current_sha current_branch
  baseline_sha=$(jq -r '.metadata.git_sha // "unknown"' "$BASELINE_PATH")
  baseline_branch=$(jq -r '.metadata.branch // "unknown"' "$BASELINE_PATH")
  current_sha=$(jq -r '.metadata.git_sha // "unknown"' "$CURRENT_PATH")
  current_branch=$(jq -r '.metadata.branch // "unknown"' "$CURRENT_PATH")

  printf "${BLUE}=== Performance Comparison ===${NC}\n"
  printf "Baseline: %s (%s)\n" "$baseline_sha" "$baseline_branch"
  printf "Current:  %s (%s)\n" "$current_sha" "$current_branch"
  printf "Threshold: >%d%% p95 regression\n\n" "$THRESHOLD"

  # Get all app keys from both files
  local -a all_apps
  mapfile -t all_apps < <(jq -r '.apps | keys[]' "$BASELINE_PATH" "$CURRENT_PATH" 2>/dev/null | sort -u)

  if [ ${#all_apps[@]} -eq 0 ]; then
    echo "No apps found in either file"
    [ -n "$baseline_tmpfile" ] && rm -f "$baseline_tmpfile"
    exit 0
  fi

  # Print table header
  printf "%-30s  %-8s  %-10s  %-10s  %-10s  %s\n" "APP / METRIC" "PHASE" "BASELINE" "CURRENT" "DELTA" "STATUS"
  printf "%-30s  %-8s  %-10s  %-10s  %-10s  %s\n" "──────────────────────────────" "────────" "──────────" "──────────" "──────────" "──────"

  local regression_found=false

  for app in "${all_apps[@]}"; do
    printf "\n${BLUE}%s${NC}\n" "$app"

    for phase in sequential concurrent; do
      for metric in p50 p95 p99; do
        local base_val cur_val
        base_val=$(jq -r ".apps[\"$app\"].$phase.latency_ms.$metric // 0" "$BASELINE_PATH" 2>/dev/null)
        cur_val=$(jq -r ".apps[\"$app\"].$phase.latency_ms.$metric // 0" "$CURRENT_PATH" 2>/dev/null)

        local delta
        delta=$(compute_delta "$base_val" "$cur_val")

        local status=""
        local color=""
        # Check if delta exceeds threshold (only flag p95)
        if [ "$metric" = "p95" ]; then
          local exceeds
          exceeds=$(awk -v d="$delta" -v t="$THRESHOLD" 'BEGIN { print (d > t) ? 1 : 0 }')
          if [ "$exceeds" -eq 1 ]; then
            status="⚠ REGRESSION"
            color="$RED"
            regression_found=true
          else
            status="OK"
            color="$GREEN"
          fi
        fi

        printf "  %-28s  %-8s  %8s ms  %8s ms  %+8s%%  ${color}%s${NC}\n" \
          "latency_ms.$metric" "$phase" "$base_val" "$cur_val" "$delta" "$status"
      done
    done
  done

  printf "\n"

  # Cleanup temp file
  [ -n "$baseline_tmpfile" ] && rm -f "$baseline_tmpfile"

  if [ "$regression_found" = true ]; then
    printf "${RED}⚠ Regression detected: p95 latency increased >%d%%${NC}\n" "$THRESHOLD"
    exit 1
  else
    printf "${GREEN}✓ No regression detected (within %d%% threshold)${NC}\n" "$THRESHOLD"
    exit 0
  fi
}

main "$@"
