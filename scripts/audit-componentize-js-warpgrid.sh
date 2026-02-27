#!/usr/bin/env bash
# audit-componentize-js-warpgrid.sh — Validate and document npm package compatibility
#
# Usage:
#   scripts/audit-componentize-js-warpgrid.sh [--table|--json|--diff|--test|--all]
#
# Modes:
#   --table    Generate human-readable markdown compatibility table
#   --json     Print WarpGrid compatibility results as JSON
#   --diff     Show packages improved by WarpGrid shims (before/after)
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

validate_json_files() {
  local errors=0

  if [ ! -f "$BASELINE_JSON" ]; then
    echo "ERROR: Baseline JSON not found: $BASELINE_JSON"
    errors=1
  fi
  if [ ! -f "$WARPGRID_JSON" ]; then
    echo "ERROR: WarpGrid JSON not found: $WARPGRID_JSON"
    errors=1
  fi

  if [ "$errors" -ne 0 ]; then
    echo "Missing required JSON files. Run the full audit first."
    exit 1
  fi

  # Validate JSON is parseable and has 20 packages
  local baseline_count warpgrid_count
  baseline_count=$(node -e "const d=JSON.parse(require('fs').readFileSync('$BASELINE_JSON','utf-8'));console.log(d.packages.length)")
  warpgrid_count=$(node -e "const d=JSON.parse(require('fs').readFileSync('$WARPGRID_JSON','utf-8'));console.log(d.packages.length)")

  if [ "$baseline_count" != "20" ]; then
    echo "ERROR: Baseline has $baseline_count packages (expected 20)"
    exit 1
  fi
  if [ "$warpgrid_count" != "20" ]; then
    echo "ERROR: WarpGrid results have $warpgrid_count packages (expected 20)"
    exit 1
  fi

  echo "Validation passed: baseline=$baseline_count packages, warpgrid=$warpgrid_count packages"
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
  validate_json_files

  echo "Generating compatibility table..."

  node --input-type=module -e "
import { readFileSync, writeFileSync } from 'node:fs';

const baseline = JSON.parse(readFileSync('$BASELINE_JSON', 'utf-8'));
const warpgrid = JSON.parse(readFileSync('$WARPGRID_JSON', 'utf-8'));

function statusRank(s) {
  if (s === 'pass') return 3;
  if (s === 'partial') return 2;
  return 1;
}

function overall(p) {
  const min = Math.min(statusRank(p.importStatus), statusRank(p.functionCallStatus), statusRank(p.realisticUsageStatus));
  if (min <= 1) return 'fail';
  if (min === 2) return 'partial';
  return 'pass';
}

const baseMap = new Map(baseline.packages.map(p => [p.name, p]));
const improved = [];
for (const pkg of warpgrid.packages) {
  const base = baseMap.get(pkg.name);
  if (base && statusRank(overall(pkg)) > statusRank(overall(base))) {
    improved.push({ name: pkg.name, before: overall(base), after: overall(pkg) });
  }
}
const improvedNames = new Set(improved.map(p => p.name));

const fullyCompat = warpgrid.packages.filter(p => overall(p) === 'pass').length;
const total = warpgrid.packages.length;

const lines = [];
lines.push('# ComponentizeJS npm Package Compatibility');
lines.push('');
lines.push('**Runtime**: ' + warpgrid.runtime + ' ' + warpgrid.runtimeVersion);
lines.push('**Shims**: ' + warpgrid.shimVersion);
lines.push('**Tested**: ' + warpgrid.testedAt);
lines.push('');
lines.push('## Summary');
lines.push('');
lines.push('- **Fully compatible**: ' + fullyCompat + '/' + total + ' packages');
lines.push('- **Improved by WarpGrid shims**: ' + improved.length + ' packages');
lines.push('- **Permanently incompatible**: ' + warpgrid.packages.filter(p => overall(p) === 'fail').length + ' packages');
lines.push('');
lines.push('## Compatibility Table');
lines.push('');
lines.push('| Package | Version | Import | Function | Realistic | Status | Notes |');
lines.push('|---------|---------|--------|----------|-----------|--------|-------|');

for (const pkg of warpgrid.packages) {
  const status = overall(pkg);
  const label = improvedNames.has(pkg.name) ? status + ' IMPROVED' : status;
  const notes = pkg.notes.length > 60 ? pkg.notes.slice(0, 57) + '...' : pkg.notes;
  lines.push('| ' + [pkg.name, pkg.version, pkg.importStatus, pkg.functionCallStatus, pkg.realisticUsageStatus, label, notes].join(' | ') + ' |');
}

if (improved.length > 0) {
  lines.push('');
  lines.push('## Packages Improved by WarpGrid Shims');
  lines.push('');
  for (const imp of improved) {
    const pkg = warpgrid.packages.find(p => p.name === imp.name);
    lines.push('### ' + imp.name);
    lines.push('');
    lines.push('- **Before** (baseline): ' + imp.before);
    lines.push('- **After** (WarpGrid): ' + imp.after);
    if (pkg) lines.push('- **Details**: ' + pkg.notes);
    lines.push('');
  }
}

const content = lines.join('\n') + '\n';
writeFileSync('$TABLE_OUTPUT', content);
console.log('Generated: $TABLE_OUTPUT');
console.log('Summary: ' + fullyCompat + '/' + total + ' fully compatible, ' + improved.length + ' improved by shims');
"
}

show_diff() {
  validate_json_files

  echo "Packages improved by WarpGrid shims:"
  echo ""

  node --input-type=module -e "
import { readFileSync } from 'node:fs';

const baseline = JSON.parse(readFileSync('$BASELINE_JSON', 'utf-8'));
const warpgrid = JSON.parse(readFileSync('$WARPGRID_JSON', 'utf-8'));

function statusRank(s) {
  if (s === 'pass') return 3;
  if (s === 'partial') return 2;
  return 1;
}

const baseMap = new Map(baseline.packages.map(p => [p.name, p]));
for (const pkg of warpgrid.packages) {
  const base = baseMap.get(pkg.name);
  if (!base) continue;
  const changes = [];
  if (statusRank(pkg.importStatus) > statusRank(base.importStatus))
    changes.push('import: ' + base.importStatus + ' → ' + pkg.importStatus);
  if (statusRank(pkg.functionCallStatus) > statusRank(base.functionCallStatus))
    changes.push('function: ' + base.functionCallStatus + ' → ' + pkg.functionCallStatus);
  if (statusRank(pkg.realisticUsageStatus) > statusRank(base.realisticUsageStatus))
    changes.push('realistic: ' + base.realisticUsageStatus + ' → ' + pkg.realisticUsageStatus);
  if (changes.length > 0) {
    console.log(pkg.name + ':');
    changes.forEach(c => console.log('  ' + c));
  }
}
"
}

show_json() {
  validate_json_files
  cat "$WARPGRID_JSON"
}

case "$MODE" in
  --table)  generate_table ;;
  --json)   show_json ;;
  --diff)   show_diff ;;
  --test)   run_tests ;;
  --all)    run_tests; generate_table ;;
  --help|-h)
    echo "Usage: $0 [--table|--json|--diff|--test|--all]"
    echo ""
    echo "Modes:"
    echo "  --table    Generate human-readable markdown compatibility table"
    echo "  --json     Print WarpGrid compatibility results as JSON"
    echo "  --diff     Show packages improved by WarpGrid shims (before/after)"
    echo "  --test     Run the compatibility audit test suite"
    echo "  --all      Run tests + generate table (default)"
    ;;
  *)
    echo "Unknown option: $MODE"
    echo "Run $0 --help for usage"
    exit 1
    ;;
esac
