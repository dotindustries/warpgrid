#!/usr/bin/env bash
# Build the wastebin demo as a wasm32-wasip2 component.
#
# Prerequisites:
#   - Rust toolchain with wasm32-wasip2 target
#   - wasm-tools CLI (for component wrapping)
#   - Build libpq-wasm first: scripts/build-libpq.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

PROFILE="${1:---debug}"
if [[ "$PROFILE" == "--release" ]]; then
    CARGO_FLAG="--release"
    TARGET_DIR="release"
else
    CARGO_FLAG=""
    TARGET_DIR="debug"
fi

echo "=== Building wastebin demo (${TARGET_DIR}) ==="

# Check prerequisites
if ! rustup target list --installed | grep -q wasm32-wasip2; then
    echo "Error: wasm32-wasip2 target not installed."
    echo "Run: rustup target add wasm32-wasip2"
    exit 1
fi

if ! command -v wasm-tools &>/dev/null; then
    echo "Error: wasm-tools not found."
    echo "Run: cargo install wasm-tools"
    exit 1
fi

LIBPQ_DIR="$PROJECT_ROOT/build/libpq-wasm"
if [[ ! -f "$LIBPQ_DIR/lib/libpq.a" ]]; then
    echo "Error: libpq-wasm not built. Run scripts/build-libpq.sh first."
    exit 1
fi

# Build
cd "$SCRIPT_DIR"
cargo build --target wasm32-wasip2 $CARGO_FLAG

WASM="$SCRIPT_DIR/target/wasm32-wasip2/${TARGET_DIR}/wastebin-demo.wasm"
OUTPUT="$SCRIPT_DIR/wastebin-demo.wasm"

# Wrap as component
echo "=== Wrapping as component ==="
wasm-tools component new "$WASM" -o "$OUTPUT"

SIZE=$(wc -c < "$OUTPUT" | tr -d ' ')
echo "=== Done: $OUTPUT (${SIZE} bytes) ==="
