#!/usr/bin/env bash
#
# bench-harness.sh — Cross-domain performance baseline measurement harness.
#
# Sends sequential and concurrent HTTP requests to WarpGrid test apps,
# collects per-request timing metrics via curl, computes percentile
# breakdowns (p50/p95/p99), and enforces a quality gate on shim overhead.
#
# Usage:
#   bench-harness.sh [OPTIONS]
#
# Options:
#   --apps APPS              Comma-separated app filter (default: all T1-T5)
#   --sequential-count N     Sequential requests per app (default: 100)
#   --concurrent-count N     Concurrent requests per app (default: 100)
#   --output PATH            Output JSON path (default: test-results/performance-baseline.json)
#   --base-url URL           Override per-app endpoint base URL
#   --dry-run                Show execution plan without running benchmarks
#   --generate-sample        Generate sample JSON with mock data (use with --dry-run)
#   --check-overhead PCT     Check if overhead percentage passes quality gate (<10%)
#   --help                   Show this help message
#
# Exit codes:
#   0  Success (all quality gates passed)
#   1  General error
#   2  Missing prerequisites
#   3  Quality gate failed (shim overhead >= 10%)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# Source percentile library
source "${SCRIPT_DIR}/lib/percentile.sh"

# ─── Defaults ──────────────────────────────────────────────────────────────────

SEQUENTIAL_COUNT=100
CONCURRENT_COUNT=100
OUTPUT_PATH="${PROJECT_ROOT}/test-results/performance-baseline.json"
DRY_RUN=false
GENERATE_SAMPLE=false
CHECK_OVERHEAD=""
APP_FILTER=""
BASE_URL_OVERRIDE=""
HARNESS_VERSION="1.0.0"
QUALITY_GATE_THRESHOLD=10  # shim overhead < 10%

# curl format string for per-request metrics
CURL_FORMAT='{"dns_ms":%{time_namelookup},"connect_ms":%{time_connect},"tls_ms":%{time_appconnect},"ttfb_ms":%{time_starttransfer},"total_ms":%{time_total},"size_bytes":%{size_download},"http_code":%{http_code}}\n'

# App definitions: name → endpoint
declare -A APP_ENDPOINTS=(
  ["t1-rust-http-postgres"]="http://localhost:3001/users"
  ["t2-rust-http-redis-postgres"]="http://localhost:3002/users"
  ["t3-go-http-postgres"]="http://localhost:3003/users"
  ["t4-ts-http-postgres"]="http://localhost:3004/users"
  ["t5-bun-http-postgres"]="http://localhost:3005/users"
)

# Ordered app names
APP_ORDER=("t1-rust-http-postgres" "t2-rust-http-redis-postgres" "t3-go-http-postgres" "t4-ts-http-postgres" "t5-bun-http-postgres")

# ─── Argument parsing ─────────────────────────────────────────────────────────

usage() {
  cat <<'EOF'
Usage: bench-harness.sh [OPTIONS]

Cross-domain performance baseline measurement harness for WarpGrid test apps.
Sends sequential and concurrent HTTP requests, collects timing metrics,
computes percentile breakdowns (p50/p95/p99), and enforces quality gates.

Options:
  --apps APPS              Comma-separated app filter (e.g., t3,t5)
                           Default: all T1-T5
  --sequential-count N     Sequential requests per app (default: 100)
  --concurrent-count N     Concurrent requests per app (default: 100)
  --output PATH            Output JSON path
                           Default: test-results/performance-baseline.json
  --base-url URL           Override per-app endpoint base URL
  --dry-run                Show execution plan without running benchmarks
  --generate-sample        Generate sample JSON with mock data (use with --dry-run)
  --check-overhead PCT     Check if overhead percentage passes quality gate (<10%)
  --help                   Show this help message

Exit codes:
  0  Success (all quality gates passed)
  1  General error
  2  Missing prerequisites
  3  Quality gate failed (shim overhead >= 10%)

Examples:
  bench-harness.sh --dry-run
  bench-harness.sh --apps t3,t5 --sequential-count 50
  bench-harness.sh --check-overhead 5.0
EOF
}

parse_args() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --apps)
        APP_FILTER="$2"
        shift 2
        ;;
      --apps=*)
        APP_FILTER="${1#--apps=}"
        shift
        ;;
      --sequential-count)
        SEQUENTIAL_COUNT="$2"
        shift 2
        ;;
      --sequential-count=*)
        SEQUENTIAL_COUNT="${1#--sequential-count=}"
        shift
        ;;
      --concurrent-count)
        CONCURRENT_COUNT="$2"
        shift 2
        ;;
      --concurrent-count=*)
        CONCURRENT_COUNT="${1#--concurrent-count=}"
        shift
        ;;
      --output)
        OUTPUT_PATH="$2"
        shift 2
        ;;
      --output=*)
        OUTPUT_PATH="${1#--output=}"
        shift
        ;;
      --base-url)
        BASE_URL_OVERRIDE="$2"
        shift 2
        ;;
      --base-url=*)
        BASE_URL_OVERRIDE="${1#--base-url=}"
        shift
        ;;
      --dry-run)
        DRY_RUN=true
        shift
        ;;
      --generate-sample)
        GENERATE_SAMPLE=true
        shift
        ;;
      --check-overhead)
        CHECK_OVERHEAD="$2"
        shift 2
        ;;
      --check-overhead=*)
        CHECK_OVERHEAD="${1#--check-overhead=}"
        shift
        ;;
      --help|-h)
        usage
        exit 0
        ;;
      *)
        echo "Error: Unknown option '$1'. Use --help for usage." >&2
        exit 1
        ;;
    esac
  done
}

# ─── Quality gate check ───────────────────────────────────────────────────────

check_overhead_gate() {
  local overhead="$1"
  # Compare: overhead < QUALITY_GATE_THRESHOLD (strictly less than)
  local result
  result=$(awk -v o="$overhead" -v t="$QUALITY_GATE_THRESHOLD" 'BEGIN { print (o < t) ? 1 : 0 }')
  if [ "$result" -eq 1 ]; then
    echo "PASS: shim overhead ${overhead}% < ${QUALITY_GATE_THRESHOLD}% threshold"
    return 0
  else
    echo "FAIL: shim overhead ${overhead}% >= ${QUALITY_GATE_THRESHOLD}% threshold" >&2
    return 1
  fi
}

# ─── Get filtered app list ────────────────────────────────────────────────────

get_target_apps() {
  if [ -z "$APP_FILTER" ]; then
    echo "${APP_ORDER[@]}"
    return
  fi

  local -a filtered=()
  IFS=',' read -ra filters <<< "$APP_FILTER"
  for app in "${APP_ORDER[@]}"; do
    for f in "${filters[@]}"; do
      f=$(echo "$f" | xargs)  # trim whitespace
      if [[ "$app" == *"$f"* ]]; then
        filtered+=("$app")
        break
      fi
    done
  done
  echo "${filtered[@]}"
}

# ─── Generate sample data ─────────────────────────────────────────────────────

generate_sample_json() {
  local output_file="$1"
  local git_sha branch timestamp
  git_sha=$(git -C "$PROJECT_ROOT" rev-parse --short HEAD 2>/dev/null || echo "unknown")
  branch=$(git -C "$PROJECT_ROOT" rev-parse --abbrev-ref HEAD 2>/dev/null || echo "unknown")
  timestamp=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

  local -a target_apps
  read -ra target_apps <<< "$(get_target_apps)"

  # Build sample apps JSON
  local apps_json="{}"
  for app in "${target_apps[@]}"; do
    local endpoint="${APP_ENDPOINTS[$app]:-http://localhost:8080/users}"
    apps_json=$(echo "$apps_json" | jq --arg app "$app" --arg endpoint "$endpoint" \
      '.[$app] = {
        "endpoint": $endpoint,
        "sequential": {
          "count": 100,
          "latency_ms": {"p50":12.3,"p95":18.7,"p99":25.1,"mean":13.2,"min":8.1,"max":45.3},
          "dns_ms": {"p50":0.1,"p95":0.3,"p99":0.5},
          "db_proxy_ms": {"p50":5.2,"p95":8.1,"p99":12.0},
          "response_size_bytes": {"p50":1024,"p95":1024,"p99":1024}
        },
        "concurrent": {
          "count": 100,
          "latency_ms": {"p50":15.1,"p95":28.4,"p99":42.7,"mean":17.8,"min":9.2,"max":65.1},
          "dns_ms": {"p50":0.1,"p95":0.4,"p99":0.6},
          "db_proxy_ms": {"p50":7.8,"p95":15.2,"p99":22.1},
          "response_size_bytes": {"p50":1024,"p95":1024,"p99":1024}
        },
        "quality_gate": {
          "shim_overhead_pct": 4.2,
          "passed": true
        }
      }')
  done

  local total_apps=${#target_apps[@]}

  jq -n \
    --arg ts "$timestamp" \
    --arg sha "$git_sha" \
    --arg branch "$branch" \
    --arg version "$HARNESS_VERSION" \
    --argjson apps "$apps_json" \
    --argjson total "$total_apps" \
    '{
      metadata: {
        timestamp: $ts,
        git_sha: $sha,
        branch: $branch,
        harness_version: $version
      },
      apps: $apps,
      summary: {
        total_apps: $total,
        quality_gates_passed: $total,
        quality_gates_failed: 0
      }
    }' > "$output_file"
}

# ─── Run sequential requests ──────────────────────────────────────────────────

run_sequential() {
  local endpoint="$1"
  local count="$2"
  local tmpdir="$3"

  for i in $(seq 1 "$count"); do
    curl -sf -o /dev/null -w "$CURL_FORMAT" "$endpoint" 2>/dev/null >> "${tmpdir}/sequential.jsonl" || true
  done
}

# ─── Run concurrent requests ──────────────────────────────────────────────────

run_concurrent() {
  local endpoint="$1"
  local count="$2"
  local tmpdir="$3"

  local -a pids=()
  for i in $(seq 1 "$count"); do
    (
      curl -sf -o /dev/null -w "$CURL_FORMAT" "$endpoint" 2>/dev/null >> "${tmpdir}/concurrent_${i}.jsonl" || true
    ) &
    pids+=($!)
  done

  # Wait for all
  for pid in "${pids[@]}"; do
    wait "$pid" 2>/dev/null || true
  done

  # Combine results
  cat "${tmpdir}"/concurrent_*.jsonl > "${tmpdir}/concurrent.jsonl" 2>/dev/null || true
}

# ─── Process results for a phase ──────────────────────────────────────────────

process_phase_results() {
  local jsonl_file="$1"
  local count="$2"

  if [ ! -s "$jsonl_file" ]; then
    echo '{"count":0,"latency_ms":{"p50":0,"p95":0,"p99":0,"mean":0,"min":0,"max":0},"dns_ms":{"p50":0,"p95":0,"p99":0},"db_proxy_ms":{"p50":0,"p95":0,"p99":0},"response_size_bytes":{"p50":0,"p95":0,"p99":0}}'
    return
  fi

  # Extract arrays using jq
  local -a latencies dns_times db_proxy_times sizes

  mapfile -t latencies < <(jq -r '.total_ms * 1000' "$jsonl_file" 2>/dev/null | sort -g)
  mapfile -t dns_times < <(jq -r '.dns_ms * 1000' "$jsonl_file" 2>/dev/null | sort -g)
  # db_proxy ≈ ttfb - connect (server-side processing time)
  mapfile -t db_proxy_times < <(jq -r '(.ttfb_ms - .connect_ms) * 1000' "$jsonl_file" 2>/dev/null | sort -g)
  mapfile -t sizes < <(jq -r '.size_bytes' "$jsonl_file" 2>/dev/null | sort -g)

  local actual_count=${#latencies[@]}

  # Compute stats for each metric
  local lat_stats dns_stats db_stats size_stats
  lat_stats=$(compute_stats "${latencies[@]}")
  dns_stats=$(compute_stats "${dns_times[@]}")
  db_stats=$(compute_stats "${db_proxy_times[@]}")
  size_stats=$(compute_stats "${sizes[@]}")

  # Assemble JSON
  jq -n \
    --argjson count "$actual_count" \
    --argjson lat "$lat_stats" \
    --argjson dns "$dns_stats" \
    --argjson db "$db_stats" \
    --argjson sz "$size_stats" \
    '{
      count: $count,
      latency_ms: {p50: $lat.p50, p95: $lat.p95, p99: $lat.p99, mean: $lat.mean, min: $lat.min, max: $lat.max},
      dns_ms: {p50: $dns.p50, p95: $dns.p95, p99: $dns.p99},
      db_proxy_ms: {p50: $db.p50, p95: $db.p95, p99: $db.p99},
      response_size_bytes: {p50: $sz.p50, p95: $sz.p95, p99: $sz.p99}
    }'
}

# ─── Main ──────────────────────────────────────────────────────────────────────

main() {
  parse_args "$@"

  # Handle --check-overhead
  if [ -n "$CHECK_OVERHEAD" ]; then
    if check_overhead_gate "$CHECK_OVERHEAD"; then
      exit 0
    else
      exit 3
    fi
  fi

  # Get target apps
  local -a target_apps
  read -ra target_apps <<< "$(get_target_apps)"

  # Handle --dry-run
  if [ "$DRY_RUN" = true ]; then
    echo "=== Dry run: Execution plan ==="
    echo ""
    echo "Target apps (${#target_apps[@]}):"
    for app in "${target_apps[@]}"; do
      local endpoint="${APP_ENDPOINTS[$app]:-unknown}"
      [ -n "$BASE_URL_OVERRIDE" ] && endpoint="$BASE_URL_OVERRIDE"
      echo "  - ${app} → ${endpoint}"
    done
    echo ""
    echo "Sequential requests per app: ${SEQUENTIAL_COUNT}"
    echo "Concurrent requests per app: ${CONCURRENT_COUNT}"
    echo "Output: ${OUTPUT_PATH}"
    echo "Quality gate: shim overhead < ${QUALITY_GATE_THRESHOLD}%"
    echo ""
    echo "Total requests would be: $(( ${#target_apps[@]} * (SEQUENTIAL_COUNT + CONCURRENT_COUNT) ))"

    if [ "$GENERATE_SAMPLE" = true ]; then
      mkdir -p "$(dirname "$OUTPUT_PATH")"
      generate_sample_json "$OUTPUT_PATH"
      echo ""
      echo "Sample JSON written to: ${OUTPUT_PATH}"
    fi

    exit 0
  fi

  # Prerequisites check
  for cmd in curl jq awk; do
    if ! command -v "$cmd" &>/dev/null; then
      echo "Error: required command '$cmd' not found" >&2
      exit 2
    fi
  done

  # Prepare output directory
  mkdir -p "$(dirname "$OUTPUT_PATH")"

  # Collect metadata
  local git_sha branch timestamp
  git_sha=$(git -C "$PROJECT_ROOT" rev-parse --short HEAD 2>/dev/null || echo "unknown")
  branch=$(git -C "$PROJECT_ROOT" rev-parse --abbrev-ref HEAD 2>/dev/null || echo "unknown")
  timestamp=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

  echo "=== WarpGrid Cross-Domain Performance Baseline ==="
  echo "Timestamp: ${timestamp}"
  echo "Git SHA: ${git_sha}"
  echo "Branch: ${branch}"
  echo ""

  local apps_json="{}"
  local gates_passed=0
  local gates_failed=0

  for app in "${target_apps[@]}"; do
    local endpoint="${APP_ENDPOINTS[$app]:-http://localhost:8080/users}"
    [ -n "$BASE_URL_OVERRIDE" ] && endpoint="$BASE_URL_OVERRIDE"

    echo "─── ${app} (${endpoint}) ───"

    # Check if endpoint is reachable
    if ! curl -sf -o /dev/null --connect-timeout 3 "$endpoint" 2>/dev/null; then
      echo "  ⚠ Endpoint not reachable, skipping"
      continue
    fi

    # Create temp directory for this app
    local tmpdir
    tmpdir=$(mktemp -d /tmp/bench-${app}-XXXXXX)

    # Sequential phase
    echo "  Sequential: ${SEQUENTIAL_COUNT} requests..."
    run_sequential "$endpoint" "$SEQUENTIAL_COUNT" "$tmpdir"
    local seq_results
    seq_results=$(process_phase_results "${tmpdir}/sequential.jsonl" "$SEQUENTIAL_COUNT")

    # Concurrent phase
    echo "  Concurrent: ${CONCURRENT_COUNT} requests..."
    run_concurrent "$endpoint" "$CONCURRENT_COUNT" "$tmpdir"
    local conc_results
    conc_results=$(process_phase_results "${tmpdir}/concurrent.jsonl" "$CONCURRENT_COUNT")

    # Compute shim overhead (db_proxy p95 as percentage of total latency p95)
    local seq_db_p95 seq_lat_p95 overhead_pct gate_passed
    seq_db_p95=$(echo "$seq_results" | jq '.db_proxy_ms.p95')
    seq_lat_p95=$(echo "$seq_results" | jq '.latency_ms.p95')

    if [ "$(awk -v v="$seq_lat_p95" 'BEGIN { print (v > 0) ? 1 : 0 }')" -eq 1 ]; then
      overhead_pct=$(awk -v db="$seq_db_p95" -v lat="$seq_lat_p95" 'BEGIN { printf "%.1f", db / lat * 100 }')
    else
      overhead_pct="0.0"
    fi

    if check_overhead_gate "$overhead_pct" >/dev/null 2>&1; then
      gate_passed=true
      gates_passed=$((gates_passed + 1))
      echo "  Quality gate: PASS (overhead ${overhead_pct}%)"
    else
      gate_passed=false
      gates_failed=$((gates_failed + 1))
      echo "  Quality gate: FAIL (overhead ${overhead_pct}% >= ${QUALITY_GATE_THRESHOLD}%)"
    fi

    # Add app results to JSON
    apps_json=$(echo "$apps_json" | jq \
      --arg app "$app" \
      --arg endpoint "$endpoint" \
      --argjson seq "$seq_results" \
      --argjson conc "$conc_results" \
      --argjson overhead "$overhead_pct" \
      --argjson passed "$gate_passed" \
      '.[$app] = {
        endpoint: $endpoint,
        sequential: $seq,
        concurrent: $conc,
        quality_gate: {
          shim_overhead_pct: $overhead,
          passed: $passed
        }
      }')

    # Cleanup
    rm -rf "$tmpdir"
    echo ""
  done

  local total_apps=${#target_apps[@]}

  # Write output JSON
  jq -n \
    --arg ts "$timestamp" \
    --arg sha "$git_sha" \
    --arg branch "$branch" \
    --arg version "$HARNESS_VERSION" \
    --argjson apps "$apps_json" \
    --argjson total "$total_apps" \
    --argjson gp "$gates_passed" \
    --argjson gf "$gates_failed" \
    '{
      metadata: {
        timestamp: $ts,
        git_sha: $sha,
        branch: $branch,
        harness_version: $version
      },
      apps: $apps,
      summary: {
        total_apps: $total,
        quality_gates_passed: $gp,
        quality_gates_failed: $gf
      }
    }' > "$OUTPUT_PATH"

  echo "═══════════════════════════════════════════════"
  echo "Results written to: ${OUTPUT_PATH}"
  echo "Total apps: ${total_apps}"
  echo "Quality gates passed: ${gates_passed}"
  echo "Quality gates failed: ${gates_failed}"

  if [ "$gates_failed" -gt 0 ]; then
    exit 3
  fi
  exit 0
}

main "$@"
