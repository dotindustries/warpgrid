#!/usr/bin/env bash
# Verify that doc examples stay in sync with fixture source code.
#
# Compares extracted examples in docs/examples/ against the canonical
# fixture templates in tests/fixtures/. Exits non-zero if any drift
# is detected.
#
# Usage: scripts/check-doc-examples.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EXIT_CODE=0

check_diff() {
    local label="$1"
    local doc_file="$2"
    local fixture_file="$3"

    if [ ! -f "$doc_file" ]; then
        echo "FAIL: $label — doc example missing: $doc_file"
        EXIT_CODE=1
        return
    fi

    if [ ! -f "$fixture_file" ]; then
        echo "FAIL: $label — fixture missing: $fixture_file"
        EXIT_CODE=1
        return
    fi

    if ! diff -q "$doc_file" "$fixture_file" > /dev/null 2>&1; then
        echo "FAIL: $label — doc example has drifted from fixture"
        echo "  doc:     $doc_file"
        echo "  fixture: $fixture_file"
        diff -u "$fixture_file" "$doc_file" || true
        EXIT_CODE=1
    else
        echo "OK:   $label"
    fi
}

echo "Checking doc examples against fixture sources..."
echo

check_diff "Rust async handler" \
    "$REPO_ROOT/docs/examples/async-rust/lib.rs" \
    "$REPO_ROOT/tests/fixtures/async-rust-template/src/lib.rs"

check_diff "TypeScript async handler" \
    "$REPO_ROOT/docs/examples/async-ts/handler.ts" \
    "$REPO_ROOT/tests/fixtures/async-ts-template/src/handler.ts"

check_diff "Go async handler" \
    "$REPO_ROOT/docs/examples/async-go/main.go" \
    "$REPO_ROOT/tests/fixtures/async-go-template/main.go"

echo
if [ "$EXIT_CODE" -eq 0 ]; then
    echo "All doc examples are in sync with fixtures."
else
    echo "Some doc examples have drifted. Update them to match the fixtures."
fi

exit "$EXIT_CODE"
