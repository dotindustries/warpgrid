/**
 * WarpGrid Bun preload script for native development mode.
 *
 * Usage:
 *   bun run --preload @warpgrid/bun-sdk/preload ./your-handler.ts
 *
 * Or configure in bunfig.toml for convenience:
 *   ```toml
 *   # bunfig.toml
 *   preload = ["@warpgrid/bun-sdk/preload"]
 *   ```
 *
 * This preload script:
 * - Sets WARPGRID_MODE=development environment variable
 * - Initializes globalThis.warpgrid namespace with { mode: "development" }
 * - Does NOT set __WARPGRID_WASM__, so detectMode() returns "native"
 * - Is idempotent — safe to load multiple times
 */

const g = globalThis as Record<string, unknown>;

// Guard against double-initialization
if (g.__WARPGRID_PRELOAD_INITIALIZED__) {
  // Already initialized, nothing to do
} else {
  // Mark as initialized
  g.__WARPGRID_PRELOAD_INITIALIZED__ = true;

  // Set environment variable for development mode
  process.env.WARPGRID_MODE = "development";

  // Initialize globalThis.warpgrid namespace if not already set
  if (!g.warpgrid) {
    g.warpgrid = { mode: "development" };
  }

  // Log startup message to stderr (convention for preload scripts)
  console.error("[warpgrid] Development mode active (native Bun)");
}
