#!/usr/bin/env bash
# audit-componentize-js.sh — Validate and document baseline npm package + core API compatibility
#
# Audits stock ComponentizeJS (StarlingMonkey) runtime compatibility
# for npm packages and core Node.js APIs, without WarpGrid shims.
#
# Usage:
#   scripts/audit-componentize-js.sh [--table|--json|--test|--all]
#
# Modes:
#   --table    Generate human-readable markdown compatibility table
#   --json     Print baseline compatibility results as JSON
#   --test     Run the compatibility audit test suite
#   --all      Run tests + generate table (default)
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

BASELINE_JSON="$PROJECT_ROOT/compat-db/componentize-js-baseline.json"
WARPGRID_JSON="$PROJECT_ROOT/compat-db/componentize-js-warpgrid.json"
TABLE_OUTPUT="$PROJECT_ROOT/compat-db/componentize-js-compat-table.md"

MODE="${1:---all}"

validate_json() {
  local errors=0

  if [ ! -f "$BASELINE_JSON" ]; then
    echo "ERROR: Baseline JSON not found: $BASELINE_JSON"
    errors=1
  fi

  if [ "$errors" -ne 0 ]; then
    echo "Missing required JSON files. Run the full audit first."
    exit 1
  fi

  # Validate JSON is parseable, has 20 packages and 11 core APIs
  local pkg_count core_api_count
  pkg_count=$(node -e "const d=JSON.parse(require('fs').readFileSync('$BASELINE_JSON','utf-8'));console.log(d.packages.length)")
  core_api_count=$(node -e "const d=JSON.parse(require('fs').readFileSync('$BASELINE_JSON','utf-8'));console.log((d.coreApis||[]).length)")

  if [ "$pkg_count" != "20" ]; then
    echo "ERROR: Baseline has $pkg_count packages (expected 20)"
    exit 1
  fi
  if [ "$core_api_count" != "11" ]; then
    echo "ERROR: Baseline has $core_api_count core APIs (expected 11)"
    exit 1
  fi

  echo "Validation passed: baseline=$pkg_count packages, $core_api_count core APIs"
}

run_tests() {
  echo "Running compatibility audit tests..."
  cd "$PROJECT_ROOT/packages/warpgrid-js"
  npx vitest run --reporter=verbose \
    src/__tests__/compat-audit.test.ts \
    src/__tests__/compat-integration.test.ts
  echo "All tests passed."
}

generate_table() {
  validate_json

  echo "Generating compatibility table..."

  # Use the audit.ts generateCompatTable for the authoritative table output
  node --input-type=module -e "
import { readFileSync, writeFileSync } from 'node:fs';
import { loadCompatResults, compareWithBaseline, compareCoreApis, generateCompatTable } from '$PROJECT_ROOT/packages/warpgrid-js/src/compat/audit.ts';

const baseline = loadCompatResults(readFileSync('$BASELINE_JSON', 'utf-8'));

// If warpgrid JSON exists, generate a comparison table
let warpgrid, improved, improvedCoreApis;
try {
  warpgrid = loadCompatResults(readFileSync('$WARPGRID_JSON', 'utf-8'));
  improved = compareWithBaseline(baseline, warpgrid);
  improvedCoreApis = compareCoreApis(baseline, warpgrid);
} catch {
  // Warpgrid JSON not available, generate baseline-only table
  warpgrid = baseline;
  improved = [];
  improvedCoreApis = [];
}

const table = generateCompatTable(warpgrid, improved, improvedCoreApis);
writeFileSync('$TABLE_OUTPUT', table + '\n');

const pkgCount = warpgrid.packages.length;
const fullyCompat = warpgrid.packages.filter(p => {
  const statuses = [p.importStatus, p.functionCallStatus, p.realisticUsageStatus];
  return statuses.every(s => s === 'pass');
}).length;
const coreApiCount = (warpgrid.coreApis || []).length;

console.log('Generated: $TABLE_OUTPUT');
console.log('Summary: ' + fullyCompat + '/' + pkgCount + ' packages fully compatible, ' + improved.length + ' improved by shims, ' + coreApiCount + ' core APIs documented');
"
}

show_json() {
  validate_json
  cat "$BASELINE_JSON"
}

case "$MODE" in
  --table)  generate_table ;;
  --json)   show_json ;;
  --test)   run_tests ;;
  --all)    run_tests; generate_table ;;
  --help|-h)
    echo "Usage: $0 [--table|--json|--test|--all]"
    echo ""
    echo "Modes:"
    echo "  --table    Generate human-readable markdown compatibility table"
    echo "  --json     Print baseline compatibility results as JSON"
    echo "  --test     Run the compatibility audit test suite"
    echo "  --all      Run tests + generate table (default)"
    ;;
  *)
    echo "Unknown option: $MODE"
    echo "Run $0 --help for usage"
    exit 1
    ;;
esac
