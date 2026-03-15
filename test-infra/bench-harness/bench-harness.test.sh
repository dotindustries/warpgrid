#!/usr/bin/env bash
#
# bench-harness.test.sh — TDD test harness for US-710: Cross-domain performance baseline.
#
# Validates:
#   1. File existence and executability
#   2. Percentile computation correctness (lib/percentile.sh)
#   3. JSON output schema structure
#   4. Quality gate threshold logic (shim overhead < 10%)
#   5. Regression detection logic (>20% p95 increase)
#   6. Argument parsing (--apps, --sequential-count, --concurrent-count, --output, --dry-run, --help)
#   7. compare-perf.sh output format and exit codes
#
# Usage:
#   test-infra/bench-harness/bench-harness.test.sh              # Run all tests
#   test-infra/bench-harness/bench-harness.test.sh --unit        # Unit tests only
#   test-infra/bench-harness/bench-harness.test.sh --integration # Integration tests
#
# Exit codes:
#   0  All tests passed
#   1  One or more tests failed
#   2  Prerequisites missing

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
BENCH_HARNESS="${SCRIPT_DIR}/bench-harness.sh"
PERCENTILE_LIB="${SCRIPT_DIR}/lib/percentile.sh"
COMPARE_SCRIPT="${PROJECT_ROOT}/scripts/compare-perf.sh"

# Counters
TOTAL=0
PASSED=0
FAILED=0
SKIPPED=0

# Colors (if terminal supports it)
if [ -t 1 ]; then
  RED='\033[0;31m'
  GREEN='\033[0;32m'
  YELLOW='\033[0;33m'
  BLUE='\033[0;34m'
  NC='\033[0m'
else
  RED='' GREEN='' YELLOW='' BLUE='' NC=''
fi

# ─── Test helpers ──────────────────────────────────────────────────────────────

pass() {
  TOTAL=$((TOTAL + 1))
  PASSED=$((PASSED + 1))
  printf "  ${GREEN}✓${NC} %s\n" "$1"
}

fail() {
  TOTAL=$((TOTAL + 1))
  FAILED=$((FAILED + 1))
  printf "  ${RED}✗${NC} %s\n" "$1"
  if [ -n "${2:-}" ]; then
    printf "    ${RED}→ %s${NC}\n" "$2"
  fi
}

skip() {
  TOTAL=$((TOTAL + 1))
  SKIPPED=$((SKIPPED + 1))
  printf "  ${YELLOW}○${NC} %s (skipped: %s)\n" "$1" "$2"
}

section() {
  printf "\n${BLUE}── %s ──${NC}\n" "$1"
}

assert_eq() {
  local desc="$1" expected="$2" actual="$3"
  if [ "$expected" = "$actual" ]; then
    pass "$desc"
  else
    fail "$desc" "expected '$expected', got '$actual'"
  fi
}

assert_contains() {
  local desc="$1" haystack="$2" needle="$3"
  if echo "$haystack" | grep -q "$needle" 2>/dev/null; then
    pass "$desc"
  else
    fail "$desc" "output does not contain '$needle'"
  fi
}

assert_exit_code() {
  local desc="$1" expected="$2"
  shift 2
  local actual
  set +e
  "$@" >/dev/null 2>&1
  actual=$?
  set -e
  if [ "$expected" -eq "$actual" ]; then
    pass "$desc"
  else
    fail "$desc" "expected exit code $expected, got $actual"
  fi
}

# ─── Unit Tests ────────────────────────────────────────────────────────────────

test_unit() {
  section "File existence"

  # bench-harness.sh exists
  if [ -f "$BENCH_HARNESS" ]; then
    pass "bench-harness.sh exists"
  else
    fail "bench-harness.sh exists" "File not found at $BENCH_HARNESS"
  fi

  # bench-harness.sh is executable
  if [ -x "$BENCH_HARNESS" ]; then
    pass "bench-harness.sh is executable"
  else
    fail "bench-harness.sh is executable" "Missing execute permission"
  fi

  # lib/percentile.sh exists
  if [ -f "$PERCENTILE_LIB" ]; then
    pass "lib/percentile.sh exists"
  else
    fail "lib/percentile.sh exists" "File not found"
  fi

  # compare-perf.sh exists
  if [ -f "$COMPARE_SCRIPT" ]; then
    pass "scripts/compare-perf.sh exists"
  else
    fail "scripts/compare-perf.sh exists" "File not found at $COMPARE_SCRIPT"
  fi

  # compare-perf.sh is executable
  if [ -x "$COMPARE_SCRIPT" ]; then
    pass "scripts/compare-perf.sh is executable"
  else
    fail "scripts/compare-perf.sh is executable" "Missing execute permission"
  fi

  # ─── Percentile library tests ──────────────────────────────────────────────

  section "Percentile computation"

  if [ ! -f "$PERCENTILE_LIB" ]; then
    for t in "p50 of single value" "p50 of odd-count array" "p95 of 20 values" \
             "p99 of 100 values" "all-same values" "compute_stats JSON structure"; do
      fail "$t" "percentile.sh not found"
    done
    return
  fi

  # Source the library
  source "$PERCENTILE_LIB"

  # Single value
  local result
  result=$(compute_percentile 50 42.0)
  assert_eq "p50 of single value returns that value" "42.0" "$result"

  # Odd count: 1 2 3 4 5 → p50 = 3rd value = 3
  result=$(compute_percentile 50 1 2 3 4 5)
  assert_eq "p50 of [1,2,3,4,5] = 3" "3" "$result"

  # p95 of [1..20] → rank = ceil(0.95*20) = 19 → value at index 18 = 19
  local -a seq20
  for i in $(seq 1 20); do seq20+=("$i"); done
  result=$(compute_percentile 95 "${seq20[@]}")
  assert_eq "p95 of [1..20] = 19" "19" "$result"

  # p99 of [1..100] → rank = ceil(0.99*100) = 99 → value at index 98 = 99
  local -a seq100
  for i in $(seq 1 100); do seq100+=("$i"); done
  result=$(compute_percentile 99 "${seq100[@]}")
  assert_eq "p99 of [1..100] = 99" "99" "$result"

  # All same values: p50/p95/p99 all return the same value
  result=$(compute_percentile 50 7 7 7 7 7)
  assert_eq "p50 of all-same [7,7,7,7,7] = 7" "7" "$result"
  result=$(compute_percentile 95 7 7 7 7 7)
  assert_eq "p95 of all-same [7,7,7,7,7] = 7" "7" "$result"

  # Even count: [1,2,3,4] → p50: rank = ceil(0.5*4) = 2 → index 1 → value 2
  result=$(compute_percentile 50 1 2 3 4)
  assert_eq "p50 of [1,2,3,4] = 2" "2" "$result"

  # Empty array
  result=$(compute_percentile 50)
  assert_eq "p50 of empty array = 0" "0" "$result"

  # ─── compute_stats tests ─────────────────────────────────────────────────

  section "compute_stats"

  # Basic stats check
  local stats_json
  stats_json=$(compute_stats 10 20 30 40 50)

  if command -v jq &>/dev/null; then
    local count min_v max_v mean_v p50_v
    count=$(echo "$stats_json" | jq '.count')
    assert_eq "compute_stats count = 5" "5" "$count"

    min_v=$(echo "$stats_json" | jq '.min')
    assert_eq "compute_stats min = 10" "10" "$min_v"

    max_v=$(echo "$stats_json" | jq '.max')
    assert_eq "compute_stats max = 50" "50" "$max_v"

    mean_v=$(echo "$stats_json" | jq '.mean')
    assert_eq "compute_stats mean = 30" "30.000" "$mean_v"

    p50_v=$(echo "$stats_json" | jq '.p50')
    assert_eq "compute_stats p50 = 30" "30" "$p50_v"

    # Unsorted input should still work
    stats_json=$(compute_stats 50 10 40 20 30)
    min_v=$(echo "$stats_json" | jq '.min')
    assert_eq "compute_stats sorts input (min=10)" "10" "$min_v"
    max_v=$(echo "$stats_json" | jq '.max')
    assert_eq "compute_stats sorts input (max=50)" "50" "$max_v"

    # Empty input
    stats_json=$(compute_stats)
    count=$(echo "$stats_json" | jq '.count')
    assert_eq "compute_stats empty input count = 0" "0" "$count"

    # Single value
    stats_json=$(compute_stats 42.5)
    count=$(echo "$stats_json" | jq '.count')
    assert_eq "compute_stats single value count = 1" "1" "$count"
    min_v=$(echo "$stats_json" | jq '.min')
    assert_eq "compute_stats single value min = 42.5" "42.5" "$min_v"
  else
    skip "compute_stats JSON field tests" "jq not available"
  fi

  # ─── Harness argument parsing ─────────────────────────────────────────────

  section "Harness argument parsing"

  if [ ! -x "$BENCH_HARNESS" ]; then
    for t in "--help prints usage" "--dry-run exits 0" "--apps parses app list"; do
      fail "$t" "bench-harness.sh not executable"
    done
  else
    # --help flag
    local help_output
    help_output=$("$BENCH_HARNESS" --help 2>&1 || true)
    assert_contains "--help prints usage" "$help_output" "Usage"
    assert_contains "--help mentions --apps" "$help_output" "\-\-apps"
    assert_contains "--help mentions --output" "$help_output" "\-\-output"
    assert_contains "--help mentions --dry-run" "$help_output" "\-\-dry-run"

    # --dry-run should exit 0 without actually running benchmarks
    local dryrun_output
    set +e
    dryrun_output=$("$BENCH_HARNESS" --dry-run 2>&1)
    local dryrun_exit=$?
    set -e
    assert_eq "--dry-run exits 0" "0" "$dryrun_exit"
    assert_contains "--dry-run shows execution plan" "$dryrun_output" "dry.run\|DRY.RUN\|Dry run\|plan\|would"
  fi

  # ─── JSON schema validation ──────────────────────────────────────────────

  section "JSON output schema"

  if [ ! -x "$BENCH_HARNESS" ] || ! command -v jq &>/dev/null; then
    skip "JSON schema tests" "bench-harness.sh or jq not available"
  else
    # Generate sample output via --dry-run --generate-sample
    local sample_json
    local tmpfile
    tmpfile=$(mktemp /tmp/bench-sample-XXXXXX.json)
    trap "rm -f $tmpfile" EXIT

    set +e
    "$BENCH_HARNESS" --dry-run --generate-sample --output "$tmpfile" 2>/dev/null
    local gen_exit=$?
    set -e

    if [ "$gen_exit" -ne 0 ] || [ ! -s "$tmpfile" ]; then
      skip "JSON schema validation" "could not generate sample JSON"
    else
      # Validate top-level structure
      local has_metadata has_apps has_summary
      has_metadata=$(jq 'has("metadata")' "$tmpfile" 2>/dev/null || echo "false")
      assert_eq "JSON has 'metadata' key" "true" "$has_metadata"

      has_apps=$(jq 'has("apps")' "$tmpfile" 2>/dev/null || echo "false")
      assert_eq "JSON has 'apps' key" "true" "$has_apps"

      has_summary=$(jq 'has("summary")' "$tmpfile" 2>/dev/null || echo "false")
      assert_eq "JSON has 'summary' key" "true" "$has_summary"

      # Metadata fields
      local has_ts has_sha has_branch
      has_ts=$(jq '.metadata | has("timestamp")' "$tmpfile" 2>/dev/null || echo "false")
      assert_eq "metadata has 'timestamp'" "true" "$has_ts"

      has_sha=$(jq '.metadata | has("git_sha")' "$tmpfile" 2>/dev/null || echo "false")
      assert_eq "metadata has 'git_sha'" "true" "$has_sha"

      has_branch=$(jq '.metadata | has("branch")' "$tmpfile" 2>/dev/null || echo "false")
      assert_eq "metadata has 'branch'" "true" "$has_branch"

      # App structure — pick first app key
      local first_app
      first_app=$(jq -r '.apps | keys[0]' "$tmpfile" 2>/dev/null || echo "")
      if [ -n "$first_app" ] && [ "$first_app" != "null" ]; then
        local has_seq has_conc has_qg
        has_seq=$(jq ".apps[\"$first_app\"] | has(\"sequential\")" "$tmpfile" 2>/dev/null || echo "false")
        assert_eq "app has 'sequential' section" "true" "$has_seq"

        has_conc=$(jq ".apps[\"$first_app\"] | has(\"concurrent\")" "$tmpfile" 2>/dev/null || echo "false")
        assert_eq "app has 'concurrent' section" "true" "$has_conc"

        has_qg=$(jq ".apps[\"$first_app\"] | has(\"quality_gate\")" "$tmpfile" 2>/dev/null || echo "false")
        assert_eq "app has 'quality_gate' section" "true" "$has_qg"

        # Sequential latency fields
        local has_p50 has_p95 has_p99 has_mean
        has_p50=$(jq ".apps[\"$first_app\"].sequential.latency_ms | has(\"p50\")" "$tmpfile" 2>/dev/null || echo "false")
        assert_eq "sequential.latency_ms has p50" "true" "$has_p50"

        has_p95=$(jq ".apps[\"$first_app\"].sequential.latency_ms | has(\"p95\")" "$tmpfile" 2>/dev/null || echo "false")
        assert_eq "sequential.latency_ms has p95" "true" "$has_p95"

        has_p99=$(jq ".apps[\"$first_app\"].sequential.latency_ms | has(\"p99\")" "$tmpfile" 2>/dev/null || echo "false")
        assert_eq "sequential.latency_ms has p99" "true" "$has_p99"

        # Quality gate fields
        local has_overhead has_passed
        has_overhead=$(jq ".apps[\"$first_app\"].quality_gate | has(\"shim_overhead_pct\")" "$tmpfile" 2>/dev/null || echo "false")
        assert_eq "quality_gate has 'shim_overhead_pct'" "true" "$has_overhead"

        has_passed=$(jq ".apps[\"$first_app\"].quality_gate | has(\"passed\")" "$tmpfile" 2>/dev/null || echo "false")
        assert_eq "quality_gate has 'passed'" "true" "$has_passed"
      else
        fail "app structure validation" "no apps found in sample JSON"
      fi

      # Summary fields
      local has_total has_gp has_gf
      has_total=$(jq '.summary | has("total_apps")' "$tmpfile" 2>/dev/null || echo "false")
      assert_eq "summary has 'total_apps'" "true" "$has_total"

      has_gp=$(jq '.summary | has("quality_gates_passed")' "$tmpfile" 2>/dev/null || echo "false")
      assert_eq "summary has 'quality_gates_passed'" "true" "$has_gp"

      has_gf=$(jq '.summary | has("quality_gates_failed")' "$tmpfile" 2>/dev/null || echo "false")
      assert_eq "summary has 'quality_gates_failed'" "true" "$has_gf"
    fi
    rm -f "$tmpfile"
    trap - EXIT
  fi

  # ─── Quality gate logic ──────────────────────────────────────────────────

  section "Quality gate logic"

  if [ ! -x "$BENCH_HARNESS" ]; then
    skip "Quality gate tests" "bench-harness.sh not executable"
  else
    # Test that --check-overhead with <10% passes
    local overhead_result
    set +e
    overhead_result=$("$BENCH_HARNESS" --check-overhead 5.0 2>&1)
    local overhead_exit=$?
    set -e
    assert_eq "shim overhead 5% passes quality gate (exit 0)" "0" "$overhead_exit"

    set +e
    overhead_result=$("$BENCH_HARNESS" --check-overhead 15.0 2>&1)
    overhead_exit=$?
    set -e
    assert_eq "shim overhead 15% fails quality gate (exit 3)" "3" "$overhead_exit"

    # Boundary: exactly 10% should fail (< 10% means strictly less)
    set +e
    overhead_result=$("$BENCH_HARNESS" --check-overhead 10.0 2>&1)
    overhead_exit=$?
    set -e
    assert_eq "shim overhead 10% fails quality gate (exit 3)" "3" "$overhead_exit"

    # Just below threshold: 9.9% should pass
    set +e
    overhead_result=$("$BENCH_HARNESS" --check-overhead 9.9 2>&1)
    overhead_exit=$?
    set -e
    assert_eq "shim overhead 9.9% passes quality gate (exit 0)" "0" "$overhead_exit"

    # Unknown option should exit 1
    set +e
    "$BENCH_HARNESS" --bogus-flag 2>/dev/null
    local unknown_exit=$?
    set -e
    assert_eq "unknown option exits 1" "1" "$unknown_exit"
  fi

  # ─── compare-perf.sh tests ──────────────────────────────────────────────

  section "compare-perf.sh"

  if [ ! -x "$COMPARE_SCRIPT" ] || ! command -v jq &>/dev/null; then
    skip "compare-perf.sh tests" "script not executable or jq missing"
  else
    # --help
    local cmp_help
    cmp_help=$("$COMPARE_SCRIPT" --help 2>&1 || true)
    assert_contains "compare-perf.sh --help prints usage" "$cmp_help" "Usage"

    # Create two sample JSON files to test diffing
    local baseline_file current_file
    baseline_file=$(mktemp /tmp/bench-baseline-XXXXXX.json)
    current_file=$(mktemp /tmp/bench-current-XXXXXX.json)
    trap "rm -f $baseline_file $current_file" EXIT

    cat > "$baseline_file" <<'BASELINE'
{
  "metadata": {"timestamp":"2026-03-01T00:00:00Z","git_sha":"aaa","branch":"main","harness_version":"1.0.0"},
  "apps": {
    "t3-go-http-postgres": {
      "endpoint": "http://localhost:8080/users",
      "sequential": {
        "count": 100,
        "latency_ms": {"p50":10.0,"p95":20.0,"p99":30.0,"mean":12.0,"min":5.0,"max":50.0},
        "dns_ms": {"p50":0.1,"p95":0.2,"p99":0.3},
        "db_proxy_ms": {"p50":5.0,"p95":8.0,"p99":12.0},
        "response_size_bytes": {"p50":1024,"p95":1024,"p99":1024}
      },
      "concurrent": {
        "count": 100,
        "latency_ms": {"p50":15.0,"p95":25.0,"p99":35.0,"mean":17.0,"min":8.0,"max":60.0},
        "dns_ms": {"p50":0.1,"p95":0.3,"p99":0.5},
        "db_proxy_ms": {"p50":7.0,"p95":12.0,"p99":18.0},
        "response_size_bytes": {"p50":1024,"p95":1024,"p99":1024}
      },
      "quality_gate": {"shim_overhead_pct":4.0,"passed":true}
    }
  },
  "summary": {"total_apps":1,"quality_gates_passed":1,"quality_gates_failed":0}
}
BASELINE

    # Current with no regression (within 20%)
    cat > "$current_file" <<'CURRENT'
{
  "metadata": {"timestamp":"2026-03-15T00:00:00Z","git_sha":"bbb","branch":"feat/test","harness_version":"1.0.0"},
  "apps": {
    "t3-go-http-postgres": {
      "endpoint": "http://localhost:8080/users",
      "sequential": {
        "count": 100,
        "latency_ms": {"p50":11.0,"p95":22.0,"p99":32.0,"mean":13.0,"min":6.0,"max":52.0},
        "dns_ms": {"p50":0.1,"p95":0.2,"p99":0.3},
        "db_proxy_ms": {"p50":5.5,"p95":8.5,"p99":12.5},
        "response_size_bytes": {"p50":1024,"p95":1024,"p99":1024}
      },
      "concurrent": {
        "count": 100,
        "latency_ms": {"p50":16.0,"p95":27.0,"p99":37.0,"mean":18.0,"min":9.0,"max":62.0},
        "dns_ms": {"p50":0.1,"p95":0.3,"p99":0.5},
        "db_proxy_ms": {"p50":7.5,"p95":12.5,"p99":18.5},
        "response_size_bytes": {"p50":1024,"p95":1024,"p99":1024}
      },
      "quality_gate": {"shim_overhead_pct":4.5,"passed":true}
    }
  },
  "summary": {"total_apps":1,"quality_gates_passed":1,"quality_gates_failed":0}
}
CURRENT

    # No regression — should exit 0
    set +e
    local cmp_output
    cmp_output=$("$COMPARE_SCRIPT" --baseline "$baseline_file" --current "$current_file" 2>&1)
    local cmp_exit=$?
    set -e
    assert_eq "compare-perf.sh no regression exits 0" "0" "$cmp_exit"
    assert_contains "compare-perf.sh shows diff table" "$cmp_output" "p95"

    # Now create a current file with >20% p95 regression
    cat > "$current_file" <<'REGRESSED'
{
  "metadata": {"timestamp":"2026-03-15T00:00:00Z","git_sha":"ccc","branch":"feat/test","harness_version":"1.0.0"},
  "apps": {
    "t3-go-http-postgres": {
      "endpoint": "http://localhost:8080/users",
      "sequential": {
        "count": 100,
        "latency_ms": {"p50":15.0,"p95":30.0,"p99":45.0,"mean":18.0,"min":8.0,"max":70.0},
        "dns_ms": {"p50":0.1,"p95":0.2,"p99":0.3},
        "db_proxy_ms": {"p50":8.0,"p95":14.0,"p99":20.0},
        "response_size_bytes": {"p50":1024,"p95":1024,"p99":1024}
      },
      "concurrent": {
        "count": 100,
        "latency_ms": {"p50":20.0,"p95":40.0,"p99":55.0,"mean":25.0,"min":12.0,"max":85.0},
        "dns_ms": {"p50":0.1,"p95":0.3,"p99":0.5},
        "db_proxy_ms": {"p50":10.0,"p95":18.0,"p99":25.0},
        "response_size_bytes": {"p50":1024,"p95":1024,"p99":1024}
      },
      "quality_gate": {"shim_overhead_pct":6.0,"passed":true}
    }
  },
  "summary": {"total_apps":1,"quality_gates_passed":1,"quality_gates_failed":0}
}
REGRESSED

    # >20% p95 regression — should exit 1
    set +e
    cmp_output=$("$COMPARE_SCRIPT" --baseline "$baseline_file" --current "$current_file" 2>&1)
    cmp_exit=$?
    set -e
    assert_eq "compare-perf.sh >20% p95 regression exits 1" "1" "$cmp_exit"

    # Test --threshold override
    set +e
    cmp_output=$("$COMPARE_SCRIPT" --baseline "$baseline_file" --current "$current_file" --threshold 60 2>&1)
    cmp_exit=$?
    set -e
    assert_eq "compare-perf.sh --threshold 60 no regression exits 0" "0" "$cmp_exit"

    # Missing file — should exit 2
    set +e
    cmp_output=$("$COMPARE_SCRIPT" --baseline /nonexistent/file.json --current "$current_file" 2>&1)
    cmp_exit=$?
    set -e
    assert_eq "compare-perf.sh missing baseline exits 2" "2" "$cmp_exit"

    # Invalid JSON — should exit 2
    local invalid_json_file
    invalid_json_file=$(mktemp /tmp/bench-invalid-XXXXXX.json)
    echo "not valid json {{{" > "$invalid_json_file"
    set +e
    cmp_output=$("$COMPARE_SCRIPT" --baseline "$invalid_json_file" --current "$current_file" 2>&1)
    cmp_exit=$?
    set -e
    assert_eq "compare-perf.sh invalid JSON baseline exits 2" "2" "$cmp_exit"
    rm -f "$invalid_json_file"

    rm -f "$baseline_file" "$current_file"
    trap - EXIT
  fi
}

# ─── Integration Tests ────────────────────────────────────────────────────────

test_integration() {
  section "Integration: dry-run execution plan"

  if [ ! -x "$BENCH_HARNESS" ]; then
    skip "Integration tests" "bench-harness.sh not executable"
    return
  fi

  # Dry-run with specific app filter
  local dryrun_output
  set +e
  dryrun_output=$("$BENCH_HARNESS" --dry-run --apps t3,t5 2>&1)
  local exit_code=$?
  set -e
  assert_eq "--dry-run --apps t3,t5 exits 0" "0" "$exit_code"
  assert_contains "--apps t3 appears in plan" "$dryrun_output" "t3"
  assert_contains "--apps t5 appears in plan" "$dryrun_output" "t5"

  # Dry-run with custom counts
  set +e
  dryrun_output=$("$BENCH_HARNESS" --dry-run --sequential-count 50 --concurrent-count 25 2>&1)
  exit_code=$?
  set -e
  assert_eq "--dry-run with custom counts exits 0" "0" "$exit_code"
  assert_contains "shows sequential count" "$dryrun_output" "50"
  assert_contains "shows concurrent count" "$dryrun_output" "25"

  # Generate sample and validate end-to-end
  section "Integration: sample JSON generation"

  local tmpfile
  tmpfile=$(mktemp /tmp/bench-integration-XXXXXX.json)
  trap "rm -f $tmpfile" EXIT

  set +e
  "$BENCH_HARNESS" --dry-run --generate-sample --output "$tmpfile" 2>/dev/null
  exit_code=$?
  set -e

  if [ "$exit_code" -eq 0 ] && [ -s "$tmpfile" ]; then
    pass "sample JSON generated successfully"

    if command -v jq &>/dev/null; then
      set +e
      jq . "$tmpfile" >/dev/null 2>&1
      local jq_exit=$?
      set -e
      assert_eq "sample JSON is valid JSON" "0" "$jq_exit"
    fi
  else
    fail "sample JSON generated successfully" "exit code=$exit_code or empty file"
  fi

  rm -f "$tmpfile"
  trap - EXIT
}

# ─── Main ──────────────────────────────────────────────────────────────────────

main() {
  local mode="${1:---unit}"

  printf "${BLUE}US-710: Cross-Domain Performance Baseline${NC}\n"
  printf "═══════════════════════════════════════════════\n"

  case "$mode" in
    --unit)
      test_unit
      ;;
    --integration)
      test_unit
      test_integration
      ;;
    *)
      test_unit
      test_integration
      ;;
  esac

  # Summary
  printf "\n═══════════════════════════════════════════════\n"
  printf "Total: %d  " "$TOTAL"
  printf "${GREEN}Passed: %d${NC}  " "$PASSED"
  if [ "$FAILED" -gt 0 ]; then
    printf "${RED}Failed: %d${NC}  " "$FAILED"
  else
    printf "Failed: %d  " "$FAILED"
  fi
  if [ "$SKIPPED" -gt 0 ]; then
    printf "${YELLOW}Skipped: %d${NC}" "$SKIPPED"
  else
    printf "Skipped: %d" "$SKIPPED"
  fi
  printf "\n"

  if [ "$FAILED" -gt 0 ]; then
    exit 1
  fi
  exit 0
}

main "$@"
