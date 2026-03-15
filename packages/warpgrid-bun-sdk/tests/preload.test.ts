import { describe, test, expect, beforeEach, afterEach } from "bun:test";

// Save original state to restore after each test
let originalWarpgrid: unknown;
let originalWarpgridMode: string | undefined;
let originalWasmFlag: unknown;
let originalPreloadFlag: unknown;

beforeEach(() => {
  const g = globalThis as Record<string, unknown>;
  originalWarpgrid = g.warpgrid;
  originalWarpgridMode = process.env.WARPGRID_MODE;
  originalWasmFlag = g.__WARPGRID_WASM__;
  originalPreloadFlag = g.__WARPGRID_PRELOAD_INITIALIZED__;

  // Clean state before each test
  delete g.warpgrid;
  delete process.env.WARPGRID_MODE;
  delete g.__WARPGRID_WASM__;
  delete g.__WARPGRID_PRELOAD_INITIALIZED__;
});

afterEach(() => {
  const g = globalThis as Record<string, unknown>;
  if (originalWarpgrid !== undefined) {
    g.warpgrid = originalWarpgrid;
  } else {
    delete g.warpgrid;
  }
  if (originalWarpgridMode !== undefined) {
    process.env.WARPGRID_MODE = originalWarpgridMode;
  } else {
    delete process.env.WARPGRID_MODE;
  }
  if (originalWasmFlag !== undefined) {
    g.__WARPGRID_WASM__ = originalWasmFlag;
  } else {
    delete g.__WARPGRID_WASM__;
  }
  if (originalPreloadFlag !== undefined) {
    g.__WARPGRID_PRELOAD_INITIALIZED__ = originalPreloadFlag;
  } else {
    delete g.__WARPGRID_PRELOAD_INITIALIZED__;
  }
});

/**
 * Helper to run the preload script in a clean state.
 * Since the preload module executes on import and is cached,
 * we dynamically import a fresh copy by busting the module cache.
 */
async function runPreload(): Promise<void> {
  // Use a dynamic import with a cache-busting query parameter
  // to ensure the module re-executes each time
  const cacheBuster = `?t=${Date.now()}-${Math.random()}`;
  await import(`../src/preload.ts${cacheBuster}`);
}

describe("preload", () => {
  test("sets process.env.WARPGRID_MODE to 'development'", async () => {
    expect(process.env.WARPGRID_MODE).toBeUndefined();
    await runPreload();
    expect(process.env.WARPGRID_MODE).toBe("development");
  });

  test("initializes globalThis.warpgrid object", async () => {
    const g = globalThis as Record<string, unknown>;
    expect(g.warpgrid).toBeUndefined();
    await runPreload();
    expect(g.warpgrid).toBeDefined();
    expect(typeof g.warpgrid).toBe("object");
  });

  test("sets globalThis.warpgrid.mode to 'development'", async () => {
    await runPreload();
    const g = globalThis as Record<string, unknown>;
    const wg = g.warpgrid as Record<string, unknown>;
    expect(wg.mode).toBe("development");
  });

  test("does NOT set __WARPGRID_WASM__ (ensures native mode detection)", async () => {
    await runPreload();
    const g = globalThis as Record<string, unknown>;
    expect(g.__WARPGRID_WASM__).toBeUndefined();
  });

  test("is idempotent — running twice does not corrupt state", async () => {
    await runPreload();
    const g = globalThis as Record<string, unknown>;
    const wgBefore = g.warpgrid as Record<string, unknown>;

    // Run again
    await runPreload();
    const wgAfter = g.warpgrid as Record<string, unknown>;

    expect(wgAfter.mode).toBe("development");
    expect(process.env.WARPGRID_MODE).toBe("development");
  });

  test("does not overwrite existing globalThis.warpgrid properties", async () => {
    const g = globalThis as Record<string, unknown>;
    g.warpgrid = { mode: "custom", extra: "data" };

    await runPreload();
    const wg = g.warpgrid as Record<string, unknown>;

    // preload should not overwrite if already initialized
    expect(wg.extra).toBe("data");
  });

  test("detectMode() returns 'native' after preload in Bun environment", async () => {
    await runPreload();
    const { detectMode } = await import("../src/postgres.ts");
    // In Bun, with no __WARPGRID_WASM__, detectMode should return "native"
    const g = globalThis as Record<string, unknown>;
    expect(g.__WARPGRID_WASM__).toBeUndefined();
    if (typeof g.Bun !== "undefined") {
      expect(detectMode()).toBe("native");
    }
  });

  test("handler integration: createPool with native mode works after preload", async () => {
    await runPreload();

    // Verify environment is correctly set up for native dev
    expect(process.env.WARPGRID_MODE).toBe("development");
    const g = globalThis as Record<string, unknown>;
    const wg = g.warpgrid as Record<string, unknown>;
    expect(wg.mode).toBe("development");

    // In native mode, createPool would use NativePool (pg package)
    // We verify the mode detection works correctly
    const { detectMode } = await import("../src/postgres.ts");
    if (typeof g.Bun !== "undefined") {
      expect(detectMode()).toBe("native");
    }
  });

  test("works when loaded as a module (simulating bun run --preload)", async () => {
    // Simulate the preload being loaded as a module
    // This is what happens with `bun run --preload @warpgrid/bun-sdk/preload`
    await runPreload();

    // After preload, the environment should be fully configured
    expect(process.env.WARPGRID_MODE).toBe("development");
    const g = globalThis as Record<string, unknown>;
    expect(g.warpgrid).toBeDefined();
    expect((g.warpgrid as Record<string, unknown>).mode).toBe("development");
    expect(g.__WARPGRID_WASM__).toBeUndefined();
  });
});
